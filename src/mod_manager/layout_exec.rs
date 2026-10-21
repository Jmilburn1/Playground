//! Library Layout (Roadmap #2) — Phase 2: the migration
//! executor, its verification, and the rollback.
//!
//! Phase 1 (see `layout.rs`) plans; this module acts.
//! The verbs are `cli layout-apply` and
//! `cli layout-rollback`, wired in `cli.rs`; everything
//! here is driven by a [`LayoutPlan`] and recorded in a
//! [`LayoutState`] as it lands, so an interrupted run is
//! always recoverable and a finished one can be undone
//! until the owner closes the window by hand.
//!
//! The work list: when a saved `layout-plan.json` exists
//! for these roots it is the contract Phase 1 wrote for
//! this phase, and it is what gets executed — with the
//! fresh plan cross-checked against it (roots must match
//! or the run refuses; any other drift is warned about
//! loudly before anything moves). With no saved plan,
//! the fresh plan is the work list. That contract is
//! also what makes a vanished source a failure rather
//! than a quietly smaller migration: the saved plan's
//! rename hits the missing file, the run halts, and
//! everything already done is reversed from the
//! operation log.
//!
//! The safety shape is Ruling C's, verbatim: archive
//! sort by same-volume renames; the projection farm
//! built in a staging sibling and verified link by link
//! BEFORE any swap; the game folder renamed whole to a
//! timestamped backup; the backup deleted by NO ONE but
//! the owner — neither verb ever removes it. A state
//! file whose migration is still open (in progress, or
//! swapped with its backup still present) blocks a new
//! apply: roll back first. A swapped state whose backup
//! is GONE means the owner closed the window by hand;
//! apply retires that stale record and proceeds.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::layout::{self, LayoutPlan, LinkKind};

/// Where this module's records live: the app config
/// dir, beside the layout plan and the library database.
fn store_dir() -> Option<PathBuf> {
    crate::paths::data_dir()
}

/// The migration state file: present while a migration
/// is in progress or its rollback window is open.
pub fn state_path() -> Option<PathBuf> {
    Some(store_dir()?.join("layout-state.json"))
}

/// The on-disk paths a run writes to. Production uses
/// the real config dir; tests point these at a temp dir.
#[derive(Debug, Clone)]
pub struct StorePaths {
    pub state: PathBuf,
    pub log: PathBuf,
}

fn prod_store_paths() -> Result<StorePaths> {
    let dir = store_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    Ok(StorePaths {
        state: dir.join("layout-state.json"),
        log: dir.join("layout-runs").join(format!("layout-{}.log", ts)),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationStatus {
    /// Operations are landing, or a crash left the run
    /// between operations (the swap may or may not have
    /// happened — the op log says).
    InProgress,
    /// The swap completed; the rollback window is open
    /// (the backup still exists, or did at last record).
    Swapped,
    /// Verification or execution failed and everything
    /// was reversed. The world is back where it started;
    /// the record stays until the next apply or a
    /// rollback call clears it.
    FailedReversed,
}

/// One completed, durable operation — the reversal
/// script, in landing order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletedOp {
    /// A category folder created in the library.
    CreateDir { path: PathBuf },
    /// An archive rename (file or folder mod).
    Move { from: PathBuf, to: PathBuf },
    /// A projection created inside the staging folder
    /// (`link_path` is the staging path, not the final
    /// game-folder path).
    StageLink {
        link_path: PathBuf,
        target: PathBuf,
        kind: LinkKind,
    },
    /// The game folder was renamed to the backup.
    SwapOut,
    /// The staging folder was renamed to the game folder.
    SwapIn,
}

/// The migration record. Written BEFORE the first
/// filesystem operation and re-written after every
/// completed one (temp file + rename), so the op log on
/// disk always reflects what has actually landed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutState {
    pub format: u32,
    pub status: MigrationStatus,
    pub started_at: String,
    pub library_root: PathBuf,
    pub game_root: PathBuf,
    pub staging_root: PathBuf,
    pub backup_root: PathBuf,
    pub log_file: PathBuf,
    pub plan: LayoutPlan,
    pub ops: Vec<CompletedOp>,
}

fn save_state(path: &Path, state: &LayoutState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&tmp, json)?;
    if path.exists() {
        // std::fs::rename will not replace an existing
        // file on Windows; remove-then-rename is the
        // portable shape.
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_state(path: &Path) -> Result<Option<LayoutState>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read layout state {}", path.display()))?;
    let state: LayoutState = serde_json::from_str(&text)
        .with_context(|| format!("layout state {} is not readable JSON", path.display()))?;
    Ok(Some(state))
}

/// The per-run text log: one line per landed operation,
/// flushed as it lands. Best-effort — the state file is
/// the authoritative record; a log write failure never
/// fails a migration.
struct RunLog {
    file: Option<std::fs::File>,
}

impl RunLog {
    fn open(path: &Path) -> RunLog {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        RunLog {
            file: std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok(),
        }
    }

    fn line(&mut self, msg: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{} {}", chrono::Utc::now().format("%H:%M:%S"), msg);
            let _ = f.flush();
        }
    }
}

/// Print a line to the user and mirror it into the log.
fn say(log: &mut RunLog, msg: &str) {
    println!("{}", msg);
    log.line(msg);
}

/// Can the two roots be proven to sit on one volume?
/// Some(true/false) when determinable, None when the
/// platform gives no answer. The migration requires
/// Some(true): a hardlink farm cannot cross volumes
/// (Ruling F).
pub fn same_volume(a: &Path, b: &Path) -> Option<bool> {
    let a = std::fs::canonicalize(a).ok()?;
    let b = std::fs::canonicalize(b).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let da = std::fs::metadata(&a).ok()?.dev();
        let db = std::fs::metadata(&b).ok()?.dev();
        Some(da == db)
    }
    #[cfg(windows)]
    {
        let va = volume_of(&a)?;
        let vb = volume_of(&b)?;
        Some(va == vb)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a, b);
        None
    }
}

/// The volume portion of a canonicalized Windows path:
/// the drive or share a `\\?\` verbatim prefix names,
/// compared case-insensitively with the prefix stripped.
#[cfg(windows)]
fn volume_of(p: &Path) -> Option<String> {
    use std::path::Component;
    match p.components().next()? {
        Component::Prefix(prefix) => {
            let raw = prefix.as_os_str().to_string_lossy().to_uppercase();
            Some(raw.trim_start_matches("\\\\?\\").to_string())
        }
        _ => None,
    }
}

/// Build one projection of `target` at `link_path` —
/// THE function every projection in the app goes
/// through, so the migration farm (this phase), steady
/// state (Phase 3), and update installs (Ruling E) can
/// never drift apart on what a link is. File mods get a
/// hard link; folder mods get a directory junction
/// (a symlink is the Unix stand-in — same semantics for
/// the game: follow it and you are in the target).
pub fn create_projection(target: &Path, link_path: &Path, kind: LinkKind) -> Result<()> {
    match kind {
        LinkKind::Hardlink => std::fs::hard_link(target, link_path).with_context(|| {
            format!(
                "could not hard-link {} -> {}",
                link_path.display(),
                target.display()
            )
        }),
        LinkKind::Junction => create_junction(target, link_path),
    }
}

#[cfg(unix)]
fn create_junction(target: &Path, link_path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link_path).with_context(|| {
        format!(
            "could not link {} -> {}",
            link_path.display(),
            target.display()
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn create_junction(_target: &Path, _link_path: &Path) -> Result<()> {
    anyhow::bail!("directory junctions are not supported on this platform")
}

/// Windows junction creation: make the directory, then
/// stamp it with a mount-point reparse point naming the
/// target. Hand-rolled FFI over kernel32 (CreateFileW /
/// DeviceIoControl with FSCTL_SET_REPARSE_POINT) rather
/// than a new windows-sys dependency: four stable ABI
/// entry points, and the reparse payload itself is built
/// by [`junction_reparse_buffer`], a pure function the
/// unit tests pin byte for byte on any platform.
#[cfg(windows)]
fn create_junction(target: &Path, link_path: &Path) -> Result<()> {
    let target_abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::fs::canonicalize(target)
            .with_context(|| format!("junction target {} not found", target.display()))?
    };
    // A canonicalized target carries the \\?\ verbatim
    // prefix; the reparse substitute name wants the
    // plain drive path.
    let display = target_abs.to_string_lossy().replace("\\\\?\\", "");
    std::fs::create_dir(link_path)
        .with_context(|| format!("could not create junction dir {}", link_path.display()))?;
    match win_junction::set_mount_point(link_path, &display) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_dir(link_path);
            Err(e)
        }
    }
}

/// The REPARSE_DATA_BUFFER payload for a junction to
/// the target named by `target_display` (an absolute
/// path in backslash form, no `\\?\` prefix).
///
/// The documented layout (all integers little-endian):
/// a fixed header — u32 reparse tag
/// (IO_REPARSE_TAG_MOUNT_POINT), u16 payload length,
/// u16 reserved — then the mount-point path block:
/// u16 substitute-name offset, u16 substitute-name
/// length in bytes (terminator not counted), u16
/// print-name offset, u16 print-name length, then the
/// UTF-16LE substitute name (`\??\` + target) and print
/// name (the target), each followed by a NUL wide char.
/// The payload length covers the path block and both
/// terminators, but not the 8-byte fixed header.
pub fn junction_reparse_buffer(target_display: &str) -> Vec<u8> {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let substitute: Vec<u16> = format!("\\??\\{}", target_display).encode_utf16().collect();
    let print: Vec<u16> = target_display.encode_utf16().collect();
    let sub_bytes = (substitute.len() * 2) as u16;
    let print_bytes = (print.len() * 2) as u16;
    let print_offset = sub_bytes + 2; // substitute + its NUL
    let data_len = 8 + print_offset + print_bytes + 2;

    let mut buf = Vec::with_capacity(8 + data_len as usize);
    buf.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // Reserved
    buf.extend_from_slice(&0u16.to_le_bytes()); // SubstituteNameOffset
    buf.extend_from_slice(&sub_bytes.to_le_bytes());
    buf.extend_from_slice(&print_offset.to_le_bytes());
    buf.extend_from_slice(&print_bytes.to_le_bytes());
    for u in &substitute {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf.extend_from_slice(&0u16.to_le_bytes()); // substitute NUL
    for u in &print {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf.extend_from_slice(&0u16.to_le_bytes()); // print NUL
    buf
}

#[cfg(windows)]
mod win_junction {
    use anyhow::Result;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const INVALID_HANDLE_VALUE: isize = -1;

    #[allow(non_snake_case)]
    extern "system" {
        fn CreateFileW(
            lp_file_name: *const u16,
            dw_desired_access: u32,
            dw_share_mode: u32,
            lp_security_attributes: *mut c_void,
            dw_creation_disposition: u32,
            dw_flags_and_attributes: u32,
            h_template_file: *mut c_void,
        ) -> isize;
        fn DeviceIoControl(
            h_device: isize,
            dw_io_control_code: u32,
            lp_in_buffer: *const c_void,
            n_in_buffer_size: u32,
            lp_out_buffer: *mut c_void,
            n_out_buffer_size: u32,
            lp_bytes_returned: *mut u32,
            lp_overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(h_object: isize) -> i32;
        fn GetLastError() -> u32;
    }

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Stamp the (already created) directory at `dir`
    /// as a junction to `target_display`.
    pub fn set_mount_point(dir: &Path, target_display: &str) -> Result<()> {
        let buffer = super::junction_reparse_buffer(target_display);
        let name = wide(dir);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_ALL,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let code = unsafe { GetLastError() };
            anyhow::bail!(
                "could not open {} to set its junction (Windows error {})",
                dir.display(),
                code
            );
        }
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr() as *const c_void,
                buffer.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        let code = if ok == 0 { unsafe { GetLastError() } } else { 0 };
        unsafe {
            CloseHandle(handle);
        }
        if ok == 0 {
            anyhow::bail!(
                "FSCTL_SET_REPARSE_POINT failed for {} (Windows error {})",
                dir.display(),
                code
            );
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Verification — every claim the migration makes, checked
// before the swap and again (as an audit) before rollback
// deletes anything.
// ---------------------------------------------------------------------

/// Read up to `n` bytes from the front of a file.
fn read_prefix(p: &Path, n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)
        .with_context(|| format!("could not open {}", p.display()))?;
    let mut buf = vec![0u8; n];
    let mut read = 0;
    while read < n {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(k) => read += k,
            Err(e) => return Err(e.into()),
        }
    }
    buf.truncate(read);
    Ok(buf)
}

/// Prove a staged file link: same size as its target,
/// same leading bytes, and (on Unix) literally the same
/// inode — the hardlink property itself.
pub fn verify_file_link(target: &Path, link_path: &Path) -> Result<()> {
    let tm = std::fs::metadata(target)
        .with_context(|| format!("link target {} is missing", target.display()))?;
    let lm = std::fs::metadata(link_path)
        .with_context(|| format!("staged link {} is missing", link_path.display()))?;
    if lm.len() != tm.len() {
        anyhow::bail!(
            "staged link {} has {} bytes, its target has {}",
            link_path.display(),
            lm.len(),
            tm.len()
        );
    }
    let n = (tm.len() as usize).min(64 * 1024);
    if read_prefix(target, n)? != read_prefix(link_path, n)? {
        anyhow::bail!(
            "staged link {} does not read the same first bytes as {}",
            link_path.display(),
            target.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if tm.dev() != lm.dev() || tm.ino() != lm.ino() {
            anyhow::bail!(
                "staged link {} is not the same file as {} (not a hardlink)",
                link_path.display(),
                target.display()
            );
        }
    }
    Ok(())
}

/// Prove a staged folder link: the mod's modDesc.xml
/// reads identically through the link and through the
/// target (the file the game itself will open first).
/// If a target somehow has no modDesc.xml, fall back to
/// comparing the top-level name sets.
pub fn verify_folder_link(target: &Path, link_path: &Path) -> Result<()> {
    let desc = "modDesc.xml";
    let through_target = std::fs::read(target.join(desc));
    let through_link = std::fs::read(link_path.join(desc));
    match (through_target, through_link) {
        (Ok(a), Ok(b)) => {
            if a != b {
                anyhow::bail!(
                    "modDesc.xml read through {} differs from {}'s",
                    link_path.display(),
                    target.display()
                );
            }
            Ok(())
        }
        (Err(_), Err(_)) => {
            let names = |root: &Path| -> Result<Vec<String>> {
                let mut v: Vec<String> = std::fs::read_dir(root)
                    .with_context(|| format!("could not list {}", root.display()))?
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                v.sort();
                Ok(v)
            };
            if names(target)? != names(link_path)? {
                anyhow::bail!(
                    "folder link {} does not list the same entries as {}",
                    link_path.display(),
                    target.display()
                );
            }
            Ok(())
        }
        _ => anyhow::bail!(
            "modDesc.xml is readable on only one side of {} -> {}",
            link_path.display(),
            target.display()
        ),
    }
}

/// Is this entry a directory junction (reparse point)?
/// The Windows half of the rollback audit: std reports
/// a junction inconsistently across versions, so the
/// audit asks the filesystem attribute directly.
#[cfg(windows)]
fn is_reparse_point(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    std::fs::symlink_metadata(p)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

/// Audit the game folder before rollback removes it:
/// EVERY top-level entry must be a projection of the
/// planned library target it is named for. Returns the
/// list of offending entries (empty = the folder is
/// purely the farm). A real file the owner dropped in
/// after the migration — anything whose twin cannot be
/// proven — is an offender, and rollback halts rather
/// than deleting it.
pub fn audit_projection_folder(game_root: &Path, plan: &LayoutPlan) -> Vec<String> {
    let mut offenders: Vec<String> = Vec::new();
    let by_name: HashMap<&str, &layout::PlannedLink> = plan
        .links
        .iter()
        .map(|l| (l.file_name.as_str(), l))
        .collect();
    let entries = match std::fs::read_dir(game_root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(e) => return vec![format!("{}: cannot be listed ({})", game_root.display(), e)],
    };
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(link) = by_name.get(name.as_str()) else {
            offenders.push(format!(
                "{}: not a planned projection (not in the migration plan)",
                path.display()
            ));
            continue;
        };
        if !projection_proven(&link.target, &path, link.kind) {
            offenders.push(format!(
                "{}: cannot be proven to project {}",
                path.display(),
                link.target.display()
            ));
        }
    }
    offenders
}

/// The platform-honest proof that `link_path` projects
/// `target`. Unix: the engine's own link inspection
/// (inode equality for hardlinks, resolution for
/// symlinks). Windows: a symlink is checked by
/// resolution; a regular file by content — same size
/// and same SHA-256 as its library twin, so deleting it
/// loses nothing that does not survive in the library;
/// a directory must be a reparse point whose modDesc
/// reads through identically.
///
/// THE satisfaction check for the whole app: the
/// rollback audit proves the farm with it, and steady
/// state (Phase 3) proves a game-folder entry with it
/// before treating the entry as a link rather than a
/// file — one definition of "this is a projection",
/// shared, never re-derived.
pub fn projection_proven(target: &Path, link_path: &Path, kind: LinkKind) -> bool {
    #[cfg(unix)]
    {
        layout::link_satisfied(target, link_path, kind)
    }
    #[cfg(windows)]
    {
        let Ok(md) = std::fs::symlink_metadata(link_path) else {
            return false;
        };
        if md.file_type().is_symlink() {
            return layout::link_satisfied(target, link_path, kind);
        }
        match kind {
            LinkKind::Hardlink => files_byte_identical(target, link_path),
            LinkKind::Junction => {
                is_reparse_point(link_path) && verify_folder_link(target, link_path).is_ok()
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link_path, kind);
        false
    }
}

/// Byte-compare two files in chunks (the Windows
/// hardlink proof — see `projection_proven`). Sizes
/// first, then an 8 MiB walking compare that stops at
/// the first difference; neither file is held in memory.
#[cfg(windows)]
fn files_byte_identical(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let mut ba = vec![0u8; 8 * 1024 * 1024];
    let mut bb = vec![0u8; 8 * 1024 * 1024];
    loop {
        let (Ok(ra), Ok(rb)) = (fa.read(&mut ba), fb.read(&mut bb)) else {
            return false;
        };
        if ra != rb || ba[..ra] != bb[..rb] {
            return false;
        }
        if ra == 0 {
            return true;
        }
    }
}

/// Remove one game-folder entry, having already proven
/// it is a projection: files and file links unlink;
/// directory links come off as links, not as trees; a
/// real directory (only reachable if the audit was
/// bypassed) is removed recursively. The rollback's farm
/// teardown uses it, and so does steady-state
/// deactivation (Phase 3) — removing a proven projection
/// is the same operation in both worlds.
pub fn remove_projected_entry(path: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not stat {}", path.display()))?;
    let ft = md.file_type();
    if ft.is_symlink() {
        #[cfg(windows)]
        {
            // A junction / directory symlink: Windows
            // removes it as a directory link, not a file.
            let target_is_dir =
                std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
            if target_is_dir {
                return std::fs::remove_dir(path)
                    .or_else(|_| std::fs::remove_file(path))
                    .with_context(|| format!("could not remove link {}", path.display()));
            }
        }
        return std::fs::remove_file(path)
            .with_context(|| format!("could not remove link {}", path.display()));
    }
    if md.is_dir() {
        #[cfg(windows)]
        {
            if is_reparse_point(path) {
                return std::fs::remove_dir(path)
                    .with_context(|| format!("could not remove junction {}", path.display()));
            }
        }
        return std::fs::remove_dir_all(path)
            .with_context(|| format!("could not remove {}", path.display()));
    }
    std::fs::remove_file(path).with_context(|| format!("could not remove {}", path.display()))
}

/// Remove a whole folder of projections (the farm):
/// entry by entry, then the folder itself.
fn remove_projection_tree(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("could not list {}", root.display()))?
        .filter_map(|e| e.ok())
    {
        remove_projected_entry(&entry.path())?;
    }
    std::fs::remove_dir(root)
        .with_context(|| format!("could not remove {}", root.display()))
}

/// Reverse the archive half of a migration from its op
/// log: moves renamed back (newest first), category
/// folders removed when left empty. Returns the errors
/// met along the way — reversal is best-effort and
/// reports rather than hiding a stuck step.
fn reverse_moves_and_dirs(ops: &[CompletedOp]) -> Vec<String> {
    let mut errors = Vec::new();
    for op in ops.iter().rev() {
        match op {
            CompletedOp::Move { from, to } => {
                if to.exists() && !from.exists() {
                    if let Err(e) = std::fs::rename(to, from) {
                        errors.push(format!(
                            "could not rename {} back to {}: {}",
                            to.display(),
                            from.display(),
                            e
                        ));
                    }
                } else if !to.exists() {
                    errors.push(format!(
                        "cannot reverse move: {} no longer exists",
                        to.display()
                    ));
                }
            }
            CompletedOp::CreateDir { path } => {
                // Only succeeds when empty — exactly the
                // cleanup wanted; a non-empty folder is a
                // fact to keep, not an error.
                let _ = std::fs::remove_dir(path);
            }
            _ => {}
        }
    }
    errors
}

// ---------------------------------------------------------------------
// Apply — the migration itself.
// ---------------------------------------------------------------------

#[derive(Debug)]
pub enum ApplyOutcome {
    /// The fresh plan has no work: the tree is already
    /// in layout form (or a hand migration matches it).
    AlreadyLaidOut,
    Migrated {
        moves: usize,
        links: usize,
        backup: PathBuf,
    },
}

/// Do two plans describe the same work? Compared
/// through serialization — every field is derived from
/// listings and records, so an unchanged tree produces
/// an identical plan.
fn plans_equivalent(a: &LayoutPlan, b: &LayoutPlan) -> bool {
    serde_json::to_string(a).ok() == serde_json::to_string(b).ok()
}

/// Append an op to the record: in-memory first (the
/// run's own source of truth), then the state file,
/// then the log line.
fn record_op(
    store: &StorePaths,
    state: &mut LayoutState,
    log: &mut RunLog,
    op: CompletedOp,
    msg: &str,
) -> Result<()> {
    state.ops.push(op);
    save_state(&store.state, state)?;
    say(log, msg);
    Ok(())
}

/// Undo everything a run has landed, newest first: the
/// farm (if it was swapped in), the backup rename, the
/// staging folder, then the archive moves. Best-effort,
/// collecting errors rather than stopping at the first
/// stuck step.
fn reverse_all(state: &LayoutState, log: &mut RunLog) -> Vec<String> {
    let mut errors = Vec::new();
    let swapped_in = state
        .ops
        .iter()
        .any(|o| matches!(o, CompletedOp::SwapIn));
    let swapped_out = state
        .ops
        .iter()
        .any(|o| matches!(o, CompletedOp::SwapOut));
    if swapped_in {
        say(log, "[reverse] removing the projection farm from the game folder");
        if let Err(e) = remove_projection_tree(&state.game_root) {
            errors.push(format!("could not remove the farm: {}", e));
        }
    }
    if swapped_out {
        if state.game_root.exists() {
            if swapped_in {
                // The farm was just removed above (or
                // its removal failed — reported already).
            } else {
                errors.push(format!(
                    "the game folder {} exists but the swap never completed; not touching it",
                    state.game_root.display()
                ));
            }
        }
        if !state.game_root.exists() {
            say(
                log,
                &format!(
                    "[reverse] renaming the backup {} back to {}",
                    state.backup_root.display(),
                    state.game_root.display()
                ),
            );
            if let Err(e) = std::fs::rename(&state.backup_root, &state.game_root) {
                errors.push(format!("could not restore the backup: {}", e));
            }
        }
    }
    if !swapped_in && state.staging_root.exists() {
        say(log, "[reverse] removing the staging folder");
        if let Err(e) = std::fs::remove_dir_all(&state.staging_root) {
            errors.push(format!("could not remove staging: {}", e));
        }
    }
    for e in reverse_moves_and_dirs(&state.ops) {
        errors.push(e);
    }
    if errors.is_empty() {
        say(log, "[reverse] archive moves renamed back");
    }
    errors
}

/// The verification pass, run on the staged farm BEFORE
/// the swap: every archive target exists at its planned
/// size, every staged projection reads through to its
/// target, and the farm holds exactly the planned
/// entries. Any failure aborts into reversal.
fn verify_migration(
    state: &LayoutState,
    plan: &LayoutPlan,
    pre_sizes: &HashMap<PathBuf, u64>,
    log: &mut RunLog,
) -> Result<()> {
    let collision_names: HashSet<&str> = plan
        .warnings
        .iter()
        .filter(|w| w.kind == layout::WarningKind::TargetCollision)
        .map(|w| w.file_name.as_str())
        .collect();
    let mut verified_targets = 0usize;
    for e in &plan.entries {
        if e.library_path.is_none() || collision_names.contains(e.file_name.as_str()) {
            continue;
        }
        let expected = pre_sizes.get(&e.target_path).copied().unwrap_or(e.size);
        if !e.target_path.exists() {
            anyhow::bail!(
                "verification: archive target {} is missing after the sort",
                e.target_path.display()
            );
        }
        let now = layout::size_on_disk(&e.target_path);
        if now != expected {
            anyhow::bail!(
                "verification: {} measures {} bytes after the sort, {} before",
                e.target_path.display(),
                now,
                expected
            );
        }
        verified_targets += 1;
    }
    for link in &plan.links {
        let staged = state.staging_root.join(&link.file_name);
        match link.kind {
            LinkKind::Hardlink => verify_file_link(&link.target, &staged)?,
            LinkKind::Junction => verify_folder_link(&link.target, &staged)?,
        }
    }
    let staged_count = std::fs::read_dir(&state.staging_root)
        .map(|rd| rd.count())
        .unwrap_or(usize::MAX);
    if staged_count != plan.links.len() {
        anyhow::bail!(
            "verification: the staging folder holds {} entries, the plan calls for {}",
            staged_count,
            plan.links.len()
        );
    }
    say(
        log,
        &format!(
            "[apply] verified: {} archive targets in place, {} projections read through, census reconciles",
            verified_targets,
            plan.links.len()
        ),
    );
    Ok(())
}

/// Execute the plan's operations in order: category
/// folders, archive renames, the staging farm, the
/// verification, the swap. Every landed op is recorded
/// before the next begins. `progress`, when given, is
/// called after every landed op with (done, total) —
/// the total counts every op this run will record:
/// category folders to create, moves, staged links, and
/// the two swap renames.
fn execute(
    store: &StorePaths,
    state: &mut LayoutState,
    log: &mut RunLog,
    plan: &LayoutPlan,
    progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<()> {
    // Pre-move sizes, measured off the copies about to
    // move (records can be stale; the files cannot).
    let mut pre_sizes: HashMap<PathBuf, u64> = HashMap::new();
    for e in &plan.entries {
        if let Some(lp) = &e.library_path {
            pre_sizes.insert(e.target_path.clone(), layout::size_on_disk(lp));
        }
    }

    let mut dirs: Vec<PathBuf> = plan
        .moves
        .iter()
        .filter_map(|m| m.to.parent().map(|p| p.to_path_buf()))
        .collect();
    dirs.sort();
    dirs.dedup();
    // The progress total: every op this run will record
    // (see the fn docs). Counted before the first op
    // lands, from the same facts execute acts on.
    let total_ops = dirs.iter().filter(|d| !d.exists()).count()
        + plan.moves.len()
        + plan.links.len()
        + 2;
    let mut done_ops = 0usize;
    let tick = |done_ops: &mut usize| {
        *done_ops += 1;
        if let Some(p) = progress {
            p(*done_ops, total_ops);
        }
    };
    for d in dirs {
        if !d.exists() {
            std::fs::create_dir_all(&d)
                .with_context(|| format!("could not create category folder {}", d.display()))?;
            record_op(
                store,
                state,
                log,
                CompletedOp::CreateDir { path: d.clone() },
                &format!("[apply] created category folder {}", d.display()),
            )?;
            tick(&mut done_ops);
        }
    }

    let total_moves = plan.moves.len();
    for (i, mv) in plan.moves.iter().enumerate() {
        std::fs::rename(&mv.from, &mv.to).with_context(|| {
            format!(
                "could not move {} -> {}",
                mv.from.display(),
                mv.to.display()
            )
        })?;
        record_op(
            store,
            state,
            log,
            CompletedOp::Move {
                from: mv.from.clone(),
                to: mv.to.clone(),
            },
            &format!(
                "[apply] move {}/{}: {} -> {}",
                i + 1,
                total_moves,
                mv.from.display(),
                mv.to.display()
            ),
        )?;
        tick(&mut done_ops);
    }

    std::fs::create_dir(&state.staging_root).with_context(|| {
        format!(
            "could not create staging folder {}",
            state.staging_root.display()
        )
    })?;
    let total_links = plan.links.len();
    for (i, link) in plan.links.iter().enumerate() {
        let dest = state.staging_root.join(&link.file_name);
        create_projection(&link.target, &dest, link.kind)?;
        record_op(
            store,
            state,
            log,
            CompletedOp::StageLink {
                link_path: dest.clone(),
                target: link.target.clone(),
                kind: link.kind,
            },
            &format!(
                "[apply] link {}/{}: {} -> {}",
                i + 1,
                total_links,
                dest.display(),
                link.target.display()
            ),
        )?;
        tick(&mut done_ops);
    }

    verify_migration(state, plan, &pre_sizes, log)?;

    std::fs::rename(&state.game_root, &state.backup_root).with_context(|| {
        format!(
            "could not rename the game folder {} to its backup {}",
            state.game_root.display(),
            state.backup_root.display()
        )
    })?;
    record_op(
        store,
        state,
        log,
        CompletedOp::SwapOut,
        &format!(
            "[apply] game folder renamed to backup {}",
            state.backup_root.display()
        ),
    )?;
    tick(&mut done_ops);
    if let Err(e) = std::fs::rename(&state.staging_root, &state.game_root) {
        // Put the original straight back before reporting.
        return match std::fs::rename(&state.backup_root, &state.game_root) {
            Ok(()) => {
                state.ops.pop(); // the SwapOut is undone
                Err(e).context(
                    "the staging folder could not be renamed into place; \
                     the original game folder was put straight back",
                )
            }
            Err(e2) => Err(e).context(format!(
                "the staging folder could not be renamed into place, and putting \
                 the original back failed too ({}); run `cli layout-rollback`",
                e2
            )),
        };
    }
    record_op(
        store,
        state,
        log,
        CompletedOp::SwapIn,
        "[apply] staging folder is now the game folder",
    )?;
    tick(&mut done_ops);
    Ok(())
}

/// Run a migration. `fresh_plan` is the engine's plan
/// for the current tree; `saved_plan` is the on-disk
/// `layout-plan.json`, when one exists (see the module
/// docs for the contract between them).
pub fn apply(
    store: &StorePaths,
    library_root: &Path,
    game_root: &Path,
    fresh_plan: &LayoutPlan,
    saved_plan: Option<LayoutPlan>,
) -> Result<ApplyOutcome> {
    apply_with_progress(
        store,
        library_root,
        game_root,
        fresh_plan,
        saved_plan,
        None,
    )
}

/// [`apply`], with an optional progress sink: called
/// after every landed operation with (done, total) op
/// counts. The CLI passes None; the GUI Layout page
/// passes its channel sender so the page can tick.
pub fn apply_with_progress(
    store: &StorePaths,
    library_root: &Path,
    game_root: &Path,
    fresh_plan: &LayoutPlan,
    saved_plan: Option<LayoutPlan>,
    progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<ApplyOutcome> {
    if !library_root.is_dir() {
        anyhow::bail!("library folder {} does not exist", library_root.display());
    }
    if !game_root.is_dir() {
        anyhow::bail!("game folder {} does not exist", game_root.display());
    }

    // The work list and the drift cross-check.
    let mut drift: Vec<String> = Vec::new();
    let mut work: LayoutPlan = match &saved_plan {
        Some(saved) => {
            if saved.library_root != library_root || saved.game_root != game_root {
                anyhow::bail!(
                    "the saved layout-plan.json belongs to different folders (library {}, game {}) \
                     than the configured ones (library {}, game {}). Run `cli layout-plan` again \
                     for the current folders, or delete the saved plan.",
                    saved.library_root.display(),
                    saved.game_root.display(),
                    library_root.display(),
                    game_root.display()
                );
            }
            if !plans_equivalent(saved, fresh_plan) {
                drift.push(
                    "the saved plan differs from a fresh census (mods added, removed, or \
                     changed since it was written). The SAVED plan is what will be executed; \
                     re-run `cli layout-plan` first if that is not what you want."
                        .to_string(),
                );
            }
            saved.clone()
        }
        None => fresh_plan.clone(),
    };

    let plan_has_work = |p: &LayoutPlan| {
        !p.moves.is_empty() || p.links.iter().any(|l| !l.satisfied)
    };
    if !plan_has_work(fresh_plan) {
        println!("Already in layout form — the library is sorted and the game folder is projected. Nothing to do.");
        return Ok(ApplyOutcome::AlreadyLaidOut);
    }
    if !plan_has_work(&work) {
        drift.push(
            "the saved plan itself holds no work, so the fresh plan is the work list instead."
                .to_string(),
        );
        work = fresh_plan.clone();
    }

    // An open migration blocks a new one.
    let existing = load_state(&store.state)?;
    if let Some(s) = &existing {
        match s.status {
            MigrationStatus::InProgress => {
                anyhow::bail!(
                    "a layout migration started {} is recorded as still in progress. \
                     Roll it back first: `cli layout-rollback`.",
                    s.started_at
                );
            }
            MigrationStatus::Swapped => {
                if s.backup_root.exists() {
                    anyhow::bail!(
                        "a layout migration from {} is complete but its rollback window is \
                         still open (backup at {}). Roll it back first: `cli layout-rollback` — \
                         or, if the layout is final, delete the backup yourself and run this again.",
                        s.started_at,
                        s.backup_root.display()
                    );
                }
                println!(
                    "Note: the earlier migration's backup {} is gone, so its rollback window \
                     was closed by hand. Retiring that record.",
                    s.backup_root.display()
                );
            }
            MigrationStatus::FailedReversed => {
                println!(
                    "Note: replacing the record of the failed, already-reversed migration from {}.",
                    s.started_at
                );
            }
        }
    }

    // Ruling F: the farm requires one volume, proven.
    match same_volume(library_root, game_root) {
        Some(true) => {}
        Some(false) => anyhow::bail!(
            "the library ({}) and the game folder ({}) are on different volumes. Projections \
             cannot cross volumes (Ruling F), so the migration refuses; the copy-based \
             behavior stays in effect.",
            library_root.display(),
            game_root.display()
        ),
        None => anyhow::bail!(
            "could not prove the library and the game folder share a volume — refusing to migrate."
        ),
    }

    let game_name = game_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("the game folder {} has no name to stage beside", game_root.display()))?;
    let parent = game_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the game folder {} has no parent to stage beside", game_root.display()))?;
    let staging_root = parent.join(format!("{}-playground-staging", game_name));
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_root = parent.join(format!("{}-prelayout-backup-{}", game_name, ts));
    if backup_root.exists() {
        anyhow::bail!(
            "the backup path {} already exists — refusing to overwrite it",
            backup_root.display()
        );
    }
    if staging_root.exists() {
        let ours = existing.as_ref().map_or(false, |s| {
            s.status == MigrationStatus::FailedReversed && s.staging_root == staging_root
        });
        if ours {
            std::fs::remove_dir_all(&staging_root).with_context(|| {
                format!(
                    "could not clear the leftover staging folder {}",
                    staging_root.display()
                )
            })?;
        } else {
            anyhow::bail!(
                "the staging folder {} already exists and no migration record claims it — \
                 refusing to touch it",
                staging_root.display()
            );
        }
    }

    // The destructive step, in plain words, before
    // anything is written anywhere.
    println!("Library Layout migration");
    println!("  Library:     {}", library_root.display());
    println!("  Game folder: {}", game_root.display());
    println!(
        "  This will rename {} library mod(s) into category subfolders inside the library,",
        work.summary.moves
    );
    println!(
        "  build a staging farm at {} holding {} projection(s) (hardlinks for file mods,",
        staging_root.display(),
        work.links.len()
    );
    println!("  junctions for folder mods), verify every one of them, and then rename the");
    println!(
        "  game folder to {} and put the farm in its place.",
        backup_root.display()
    );
    println!(
        "  The game folder's real copies are NOT deleted — they stay whole in that backup."
    );
    println!(
        "  Anything else sitting in the game folder rides into the backup with them and"
    );
    println!("  will not be in the new game folder.");
    println!(
        "  Undo any time with `cli layout-rollback`; the window stays open until YOU"
    );
    println!("  delete the backup. Neither this command nor the rollback ever deletes it.");
    for note in &drift {
        println!("  WARNING: {}", note);
    }
    for w in &work.warnings {
        println!("  Plan warning [{}]: {}", w.file_name, w.detail);
    }

    // State and log first; operations after.
    if let Some(p) = store.log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut log = RunLog::open(&store.log);
    let mut state = LayoutState {
        format: 1,
        status: MigrationStatus::InProgress,
        started_at: chrono::Utc::now().to_rfc3339(),
        library_root: library_root.to_path_buf(),
        game_root: game_root.to_path_buf(),
        staging_root: staging_root.clone(),
        backup_root: backup_root.clone(),
        log_file: store.log.clone(),
        plan: work.clone(),
        ops: Vec::new(),
    };
    save_state(&store.state, &state)?;
    say(
        &mut log,
        &format!(
            "layout-apply started: {} move(s), {} link(s)",
            work.moves.len(),
            work.links.len()
        ),
    );

    match execute(store, &mut state, &mut log, &work, progress) {
        Ok(()) => {
            state.status = MigrationStatus::Swapped;
            save_state(&store.state, &state)?;
            say(
                &mut log,
                &format!(
                    "Migration complete: {} move(s), {} projection(s). The game folder is now the farm; the original sits whole at {}. Roll back with `cli layout-rollback` until you delete that backup.",
                    work.moves.len(),
                    work.links.len(),
                    backup_root.display()
                ),
            );
            Ok(ApplyOutcome::Migrated {
                moves: work.moves.len(),
                links: work.links.len(),
                backup: backup_root,
            })
        }
        Err(e) => {
            let rev_errors = reverse_all(&state, &mut log);
            state.status = MigrationStatus::FailedReversed;
            let _ = save_state(&store.state, &state);
            for re in &rev_errors {
                eprintln!("reversal problem: {}", re);
            }
            say(
                &mut log,
                "migration FAILED and was reversed from the operation log",
            );
            Err(e.context(
                "migration failed; everything it did was reversed from the operation log \
                 and the state record is marked failed-and-reversed",
            ))
        }
    }
}

// ---------------------------------------------------------------------
// Rollback — restore the pre-migration world from the
// state record.
// ---------------------------------------------------------------------

#[derive(Debug)]
pub enum RollbackOutcome {
    NothingRecorded,
    AlreadyReversed,
    Restored,
}

pub fn rollback(store: &StorePaths) -> Result<RollbackOutcome> {
    let Some(state) = load_state(&store.state)? else {
        println!("No layout migration is recorded (no layout-state.json) — nothing to roll back.");
        return Ok(RollbackOutcome::NothingRecorded);
    };
    let mut log = RunLog::open(&state.log_file);
    match state.status {
        MigrationStatus::FailedReversed => {
            say(
                &mut log,
                "rollback: the recorded migration already failed and was reversed; clearing the record",
            );
            std::fs::remove_file(&store.state)?;
            println!("That migration already failed and reversed itself — nothing to restore. Record cleared.");
            Ok(RollbackOutcome::AlreadyReversed)
        }
        MigrationStatus::InProgress => {
            say(
                &mut log,
                "rollback: reversing an interrupted migration from its operation log",
            );
            let errors = reverse_all(&state, &mut log);
            if !errors.is_empty() {
                for e in &errors {
                    eprintln!("rollback problem: {}", e);
                }
                anyhow::bail!(
                    "rollback met {} problem(s) (listed above); the state record is kept so you can inspect and retry",
                    errors.len()
                );
            }
            std::fs::remove_file(&store.state)?;
            println!("Interrupted migration reversed: archive sort undone, staging removed, original game folder restored where the swap had begun.");
            Ok(RollbackOutcome::Restored)
        }
        MigrationStatus::Swapped => {
            if !state.backup_root.exists() {
                anyhow::bail!(
                    "the backup {} no longer exists — there is nothing to restore from. \
                     If you deleted it deliberately, delete {} by hand to close the record.",
                    state.backup_root.display(),
                    store.state.display()
                );
            }
            let offenders = audit_projection_folder(&state.game_root, &state.plan);
            if !offenders.is_empty() {
                println!("Rollback halted — the game folder holds things that are not projections of the library:");
                for o in &offenders {
                    println!("  {}", o);
                }
                anyhow::bail!(
                    "nothing was removed. Move those entries out of the game folder yourself, then run the rollback again."
                );
            }
            say(&mut log, "rollback: audit clean, removing the farm");
            remove_projection_tree(&state.game_root)?;
            std::fs::rename(&state.backup_root, &state.game_root).with_context(|| {
                format!(
                    "could not rename the backup {} back to {}",
                    state.backup_root.display(),
                    state.game_root.display()
                )
            })?;
            say(
                &mut log,
                "rollback: original game folder restored; reversing the archive sort",
            );
            let errors = reverse_moves_and_dirs(&state.ops);
            if !errors.is_empty() {
                for e in &errors {
                    eprintln!("rollback problem: {}", e);
                }
                anyhow::bail!(
                    "the game folder is restored, but {} archive move(s) could not be reversed (listed above); the state record is kept",
                    errors.len()
                );
            }
            std::fs::remove_file(&store.state)?;
            say(&mut log, "rollback complete");
            println!(
                "Rolled back: the original game folder is back in place and the library sort is undone. The rollback window is closed."
            );
            Ok(RollbackOutcome::Restored)
        }
    }
}

// ---------------------------------------------------------------------
// The CLI verbs' glue — thin wrappers over the core so
// the verbs and the tests drive the same code.
// ---------------------------------------------------------------------

/// `cli layout-apply`: build the fresh plan from the
/// manager, pick up the saved plan (Phase 1's contract)
/// when one is on disk, and migrate.
pub fn run_apply(mod_manager: &crate::mod_manager::ModManager) -> Result<()> {
    let store = prod_store_paths()?;
    let fresh = mod_manager.layout_plan();
    let mut saved = None;
    if let Some(p) = layout::plan_path() {
        if p.exists() {
            match std::fs::read_to_string(&p)
                .map_err(anyhow::Error::from)
                .and_then(|t| Ok(serde_json::from_str::<LayoutPlan>(&t)?))
            {
                Ok(plan) => saved = Some(plan),
                Err(e) => println!(
                    "Note: the saved layout-plan.json is unreadable ({}); the fresh plan will be used.",
                    e
                ),
            }
        }
    }
    let outcome = apply(
        &store,
        mod_manager.get_archive_dir(),
        mod_manager.get_active_dir(),
        &fresh,
        saved,
    )?;
    match outcome {
        ApplyOutcome::AlreadyLaidOut => {}
        ApplyOutcome::Migrated { .. } => {
            println!("Record: {}", store.state.display());
            println!("Log:    {}", store.log.display());
        }
    }
    Ok(())
}

/// `cli layout-rollback`: restore from the state record.
pub fn run_rollback() -> Result<()> {
    let store = prod_store_paths()?;
    rollback(&store)?;
    Ok(())
}

// ---------------------------------------------------------------------
// The GUI Layout page's glue (Roadmap #2, Phase 4) — the
// same cores through the production store paths, with
// the page's stricter contracts on top.
// ---------------------------------------------------------------------

/// The recorded migration state, if any: the Layout
/// page's standing picture (not migrated / window open
/// / failed-and-reversed), read from the same state
/// file the CLI verbs write.
pub fn standing_state() -> Result<Option<LayoutState>> {
    let Some(path) = state_path() else {
        return Ok(None);
    };
    load_state(&path)
}

/// Apply EXACTLY the plan the owner previewed and
/// approved on the Layout page. Stricter than the CLI
/// contract on purpose: the CLI executes the saved plan
/// with a drift warning, but a GUI approval was given
/// for specific counts — so a fresh census is built
/// from the current records, and ANY difference from
/// the previewed plan (or changed roots) refuses with
/// "build a fresh plan" instead of running unapproved
/// work. When the two plans agree, the previewed plan
/// is the work list, and progress ticks through the
/// sink as operations land.
pub fn apply_previewed(
    library_root: &Path,
    game_root: &Path,
    preview: &LayoutPlan,
    mods: &[super::types::Mod],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ApplyOutcome> {
    if preview.library_root != library_root || preview.game_root != game_root {
        anyhow::bail!(
            "the folders changed since this plan was built (it was made for library {}, \
             game {}). Build a fresh plan for the current folders.",
            preview.library_root.display(),
            preview.game_root.display()
        );
    }
    let fresh = layout::build_plan(library_root, game_root, mods);
    if !plans_equivalent(preview, &fresh) {
        anyhow::bail!(
            "the library changed since this plan was built (mods added, removed, or \
             changed). Build a fresh plan so the counts you approve are the work \
             that runs."
        );
    }
    let store = prod_store_paths()?;
    apply_with_progress(
        &store,
        library_root,
        game_root,
        &fresh,
        Some(preview.clone()),
        Some(progress),
    )
}

/// Roll back the recorded migration through the
/// production store paths — the same semantics (and
/// the same audit refusal) as `cli layout-rollback`.
pub fn rollback_recorded() -> Result<RollbackOutcome> {
    let store = prod_store_paths()?;
    rollback(&store)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_manager::layout::{
        CensusEntry, ModKind, PlanSummary, PlannedLink, PlannedMove,
    };

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "layout-exec-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_file(path: &Path, fill: u8, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![fill; bytes]).unwrap();
    }

    fn store_at(base: &Path) -> StorePaths {
        StorePaths {
            state: base.join("store").join("layout-state.json"),
            log: base.join("store").join("layout.log"),
        }
    }

    /// The synthetic world: a flat library (two zips +
    /// a folder mod) and a game folder holding real
    /// copies of two of them.
    fn build_tree(base: &Path) -> (PathBuf, PathBuf) {
        let lib = base.join("lib");
        let game = base.join("game");
        write_file(&lib.join("FS25_Tractor.zip"), b'T', 100);
        write_file(&lib.join("FS25_Plain.zip"), b'P', 50);
        write_file(
            &lib.join("FS25_FolderMod").join("modDesc.xml"),
            b'D',
            24,
        );
        write_file(&lib.join("FS25_FolderMod").join("data.bin"), b'd', 64);
        write_file(&game.join("FS25_Tractor.zip"), b'T', 100);
        write_file(
            &game.join("FS25_FolderMod").join("modDesc.xml"),
            b'D',
            24,
        );
        write_file(&game.join("FS25_FolderMod").join("data.bin"), b'd', 64);
        (lib, game)
    }

    fn entry(
        lib: &Path,
        game: &Path,
        name: &str,
        kind: ModKind,
        category: &str,
        size: u64,
        in_game: bool,
    ) -> CensusEntry {
        CensusEntry {
            file_name: name.to_string(),
            kind,
            size,
            category: category.to_string(),
            library_path: Some(lib.join(name)),
            game_path: if in_game { Some(game.join(name)) } else { None },
            target_path: lib.join(category).join(name),
            active: in_game,
        }
    }

    /// A hand-built plan for the synthetic tree — the
    /// executor's contract, without the engine.
    fn make_plan(lib: &Path, game: &Path) -> LayoutPlan {
        let entries = vec![
            entry(lib, game, "FS25_Tractor.zip", ModKind::File, "Tractors", 100, true),
            entry(lib, game, "FS25_Plain.zip", ModKind::File, "Other", 50, false),
            entry(lib, game, "FS25_FolderMod", ModKind::Folder, "Scripts", 88, true),
        ];
        let moves = entries
            .iter()
            .map(|e| PlannedMove {
                file_name: e.file_name.clone(),
                from: e.library_path.clone().unwrap(),
                to: e.target_path.clone(),
            })
            .collect();
        let links = vec![
            PlannedLink {
                file_name: "FS25_Tractor.zip".to_string(),
                target: lib.join("Tractors").join("FS25_Tractor.zip"),
                link_path: game.join("FS25_Tractor.zip"),
                kind: LinkKind::Hardlink,
                satisfied: false,
            },
            PlannedLink {
                file_name: "FS25_FolderMod".to_string(),
                target: lib.join("Scripts").join("FS25_FolderMod"),
                link_path: game.join("FS25_FolderMod"),
                kind: LinkKind::Junction,
                satisfied: false,
            },
        ];
        LayoutPlan {
            format: 1,
            library_root: lib.to_path_buf(),
            game_root: game.to_path_buf(),
            entries,
            moves,
            links,
            warnings: Vec::new(),
            summary: PlanSummary {
                mods: 3,
                moves: 3,
                hardlinks: 1,
                junctions: 1,
                links_satisfied: 0,
                already_in_place: 0,
                warnings: 0,
                total_bytes: 238,
                per_category: vec![
                    ("Other".to_string(), 1),
                    ("Scripts".to_string(), 1),
                    ("Tractors".to_string(), 1),
                ],
            },
        }
    }

    /// The plan for the same tree AFTER migration: no
    /// moves, both links satisfied.
    fn migrated_plan(lib: &Path, game: &Path) -> LayoutPlan {
        let mut plan = make_plan(lib, game);
        for e in &mut plan.entries {
            e.library_path = Some(e.target_path.clone());
        }
        plan.moves.clear();
        for l in &mut plan.links {
            l.satisfied = true;
        }
        plan.summary.moves = 0;
        plan.summary.hardlinks = 0;
        plan.summary.junctions = 0;
        plan.summary.links_satisfied = 2;
        plan
    }

    // -- The reparse buffer (pure, all platforms) -------

    #[test]
    fn reparse_buffer_layout() {
        let buf = junction_reparse_buffer("D:\\FS25\\Tractors\\FS25_X");
        let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        assert_eq!(u32_at(0), 0xA000_0003, "mount-point tag");
        assert_eq!(u16_at(4) as usize, buf.len() - 8, "payload length excludes the 8-byte header");
        assert_eq!(u16_at(6), 0, "reserved");
        assert_eq!(u16_at(8), 0, "substitute offset");
        // substitute = "\??\" + 23 chars = 27 wide chars
        assert_eq!(u16_at(10), 54, "substitute byte length");
        assert_eq!(u16_at(12), 56, "print offset = substitute + NUL");
        assert_eq!(u16_at(14), 46, "print byte length");
        let decode = |from: usize, bytes: u16| -> String {
            let units: Vec<u16> = (0..(bytes as usize) / 2)
                .map(|k| u16_at(from + 2 * k))
                .collect();
            String::from_utf16(&units).unwrap()
        };
        assert_eq!(decode(16, 54), "\\??\\D:\\FS25\\Tractors\\FS25_X");
        assert_eq!(decode(16 + 56, 46), "D:\\FS25\\Tractors\\FS25_X");
        assert_eq!(u16_at(16 + 54), 0, "substitute NUL terminator");
        assert_eq!(buf.len(), 16 + 56 + 46 + 2);
    }

    // -- Small pieces ------------------------------------

    #[test]
    fn same_volume_same_dir() {
        let base = temp_root("volume");
        assert_eq!(same_volume(&base, &base), Some(true));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hardlink_projection_creates_same_file() {
        let base = temp_root("hardlink");
        let target = base.join("lib.zip");
        write_file(&target, b'Z', 32);
        let link = base.join("game.zip");
        create_projection(&target, &link, LinkKind::Hardlink).unwrap();
        assert_eq!(std::fs::read(&link).unwrap(), vec![b'Z'; 32]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&target).unwrap().ino(),
                std::fs::metadata(&link).unwrap().ino()
            );
        }
        verify_file_link(&target, &link).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn junction_projection_and_verification() {
        let base = temp_root("junction");
        let target = base.join("LibMod");
        write_file(&target.join("modDesc.xml"), b'M', 16);
        write_file(&target.join("data.bin"), b'x', 8);
        let link = base.join("GameMod");
        create_projection(&target, &link, LinkKind::Junction).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        verify_folder_link(&target, &link).unwrap();
        // A copy with different contents is not a link
        // and must not verify.
        let copy = base.join("CopyMod");
        write_file(&copy.join("modDesc.xml"), b'X', 16);
        assert!(verify_folder_link(&target, &copy).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn verify_file_link_rejects_a_plain_copy() {
        let base = temp_root("tamper");
        let target = base.join("a.zip");
        write_file(&target, b'A', 32);
        let copy = base.join("b.zip");
        write_file(&copy, b'A', 32);
        // Same bytes, different inode: not a hardlink.
        assert!(verify_file_link(&target, &copy).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- The full round trip (Unix: junctions are links) -

    #[cfg(unix)]
    #[test]
    fn apply_then_rollback_restores_the_tree() {
        use std::os::unix::fs::MetadataExt;
        let base = temp_root("roundtrip");
        let (lib, game) = build_tree(&base);
        let store = store_at(&base);
        let plan = make_plan(&lib, &game);
        let lib_ino_before = std::fs::metadata(lib.join("FS25_Tractor.zip")).unwrap().ino();
        let game_ino_before = std::fs::metadata(game.join("FS25_Tractor.zip")).unwrap().ino();
        assert_ne!(lib_ino_before, game_ino_before);

        let outcome = apply(&store, &lib, &game, &plan, None).unwrap();
        let backup = match outcome {
            ApplyOutcome::Migrated { moves, links, backup } => {
                assert_eq!(moves, 3);
                assert_eq!(links, 2);
                backup
            }
            other => panic!("expected a migration, got {:?}", other),
        };
        // Library sorted; root copies gone.
        assert!(lib.join("Tractors").join("FS25_Tractor.zip").exists());
        assert!(lib.join("Other").join("FS25_Plain.zip").exists());
        assert!(lib.join("Scripts").join("FS25_FolderMod").join("modDesc.xml").exists());
        assert!(!lib.join("FS25_Tractor.zip").exists());
        // Game folder is the farm: same inode for the
        // zip, a symlink for the folder mod.
        assert_eq!(
            std::fs::metadata(game.join("FS25_Tractor.zip")).unwrap().ino(),
            std::fs::metadata(lib.join("Tractors").join("FS25_Tractor.zip")).unwrap().ino()
        );
        assert!(std::fs::symlink_metadata(game.join("FS25_FolderMod"))
            .unwrap()
            .file_type()
            .is_symlink());
        // The backup holds the original game copies,
        // whole and distinct.
        assert_eq!(
            std::fs::metadata(backup.join("FS25_Tractor.zip")).unwrap().ino(),
            game_ino_before
        );
        assert!(backup.join("FS25_FolderMod").join("data.bin").exists());
        // State recorded as swapped; staging consumed.
        let state = load_state(&store.state).unwrap().unwrap();
        assert_eq!(state.status, MigrationStatus::Swapped);
        assert!(!base.join("game-playground-staging").exists());

        // A second apply is a clean no-op.
        let again = apply(&store, &lib, &game, &migrated_plan(&lib, &game), None).unwrap();
        assert!(matches!(again, ApplyOutcome::AlreadyLaidOut));

        // Rollback restores the original world.
        let rb = rollback(&store).unwrap();
        assert!(matches!(rb, RollbackOutcome::Restored));
        assert!(lib.join("FS25_Tractor.zip").exists());
        assert_eq!(
            std::fs::metadata(lib.join("FS25_Tractor.zip")).unwrap().ino(),
            lib_ino_before
        );
        assert!(!lib.join("Tractors").exists(), "empty category folder removed");
        assert_eq!(
            std::fs::metadata(game.join("FS25_Tractor.zip")).unwrap().ino(),
            game_ino_before
        );
        assert!(!std::fs::symlink_metadata(game.join("FS25_FolderMod"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!backup.exists(), "backup consumed by the restore");
        assert!(!store.state.exists(), "state record cleared");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn apply_with_a_missing_source_reverses_everything() {
        let base = temp_root("ghost");
        let (lib, game) = build_tree(&base);
        let store = store_at(&base);
        let mut plan = make_plan(&lib, &game);
        // A second move whose source does not exist,
        // ordered after a real one so the reversal has
        // something to undo.
        plan.entries.push(entry(
            &lib,
            &game,
            "FS25_Ghost.zip",
            ModKind::File,
            "Tractors",
            10,
            false,
        ));
        plan.moves.push(PlannedMove {
            file_name: "FS25_Ghost.zip".to_string(),
            from: lib.join("FS25_Ghost.zip"),
            to: lib.join("Tractors").join("FS25_Ghost.zip"),
        });
        let err = apply(&store, &lib, &game, &plan, None).unwrap_err();
        assert!(format!("{}", err).contains("reversed"), "error says reversed: {}", err);
        // The real move that landed was renamed back;
        // created category folders are gone.
        assert!(lib.join("FS25_Tractor.zip").exists());
        assert!(lib.join("FS25_Plain.zip").exists());
        assert!(!lib.join("Tractors").exists());
        assert!(!lib.join("Other").exists());
        // The game folder was never touched.
        assert!(game.join("FS25_Tractor.zip").exists());
        assert!(!base.join("game-playground-staging").exists());
        // The state records the failure.
        let state = load_state(&store.state).unwrap().unwrap();
        assert_eq!(state.status, MigrationStatus::FailedReversed);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn rollback_halts_on_a_stray_real_file() {
        let base = temp_root("stray");
        let (lib, game) = build_tree(&base);
        let store = store_at(&base);
        let plan = make_plan(&lib, &game);
        apply(&store, &lib, &game, &plan, None).unwrap();
        // A real file dropped into the farm after the
        // migration: rollback must refuse, not delete it.
        write_file(&game.join("stray.txt"), b'!', 5);
        let err = rollback(&store).unwrap_err();
        assert!(format!("{}", err).contains("nothing was removed"), "{}", err);
        assert!(game.join("stray.txt").exists());
        assert!(store.state.exists(), "state kept after a halted rollback");
        // Remove the stray and the rollback proceeds.
        std::fs::remove_file(game.join("stray.txt")).unwrap();
        let rb = rollback(&store).unwrap();
        assert!(matches!(rb, RollbackOutcome::Restored));
        assert!(lib.join("FS25_Tractor.zip").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_while_a_rollback_window_is_open() {
        let base = temp_root("refuse");
        let (lib, game) = build_tree(&base);
        let store = store_at(&base);
        let plan = make_plan(&lib, &game);
        apply(&store, &lib, &game, &plan, None).unwrap();
        // The fresh census still shows work (the same
        // pre-migration plan), but the open migration
        // blocks a new apply.
        let err = apply(&store, &lib, &game, &plan, None).unwrap_err();
        assert!(format!("{}", err).contains("Roll it back first"), "{}", err);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rollback_without_a_state_is_a_clean_refusal() {
        let base = temp_root("nostate");
        let store = store_at(&base);
        let rb = rollback(&store).unwrap();
        assert!(matches!(rb, RollbackOutcome::NothingRecorded));
        let _ = std::fs::remove_dir_all(&base);
    }
}
