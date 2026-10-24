//! App self-update (owner, 2026-10-22): on launch the app
//! "checks for an update to the app, and installs it."
//!
//! The shape of it, and why:
//!
//! - THE CHECK lives in the GUI as ONE small request at
//!   launch — the same shape as the details panel's
//!   per-mod Check, which has always run from the GUI.
//!   The standing rule it must not break is about BULK
//!   network work: the catalog fill (hundreds of pages,
//!   scheduled, throttling-prone) belongs to update.exe
//!   and stays there. A single release lookup is not
//!   bulk work, and update.exe cannot do it for the GUI
//!   anyway — it runs on the scheduler's clock, not the
//!   launch's, so its answer would be up to eight hours
//!   stale at the moment it matters. The check never
//!   delays first paint (it rides a background task) and
//!   every failure — offline, API error, rate limit —
//!   is a silent no-op for that launch.
//! - THE DOWNLOAD also runs from the GUI, only on the
//!   owner's click, like a mod's Download & install: one
//!   file, downloaded completely into a staging folder
//!   under the data dir and VERIFIED there (size, the
//!   release's sha256 digest when it provides one, and
//!   the zip must open and contain the app binary, whose
//!   entry is read end-to-end so its CRC is checked)
//!   before anything is replaced.
//! - THE SWAP cannot run from the GUI: a running exe
//!   cannot overwrite itself, and the GUI must be gone
//!   for its own binary to be replaced. So the GUI hands
//!   off to update.exe's `apply-app-update` verb, exits,
//!   and the helper waits for the GUI process to die,
//!   swaps the binaries, and relaunches. update.exe can
//!   replace ITSELF in the same pass because a running
//!   exe can be RENAMED aside on Windows (and Unix) even
//!   though it cannot be overwritten.
//! - THE BACKUP: the outgoing playground binary is kept
//!   as `playground.exe.bak` beside the new one (and
//!   `update.exe.bak` likewise). Any failure mid-swap
//!   restores the backups and relaunches the old
//!   version — the user is never left without a working
//!   exe. The backups are deleted by the GUI when a
//!   boot load completes successfully: a binary that has
//!   carried the app through a full boot has proven
//!   itself, and that is the moment its predecessor's
//!   backup stops earning its keep. A binary that dies
//!   before finishing a boot never reaches the cleanup,
//!   so its .bak survives for a manual restore.
//!
//! The release endpoint is overridable through the
//! PLAYGROUND_RELEASE_API_URL environment variable (the
//! full URL of the "latest release" document), so the
//! whole flow can be pointed at a local stub in tests
//! and rigs without touching the real GitHub release.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The default release endpoint: the latest published
/// release of the app's own repo.
pub const RELEASE_API_URL_DEFAULT: &str =
    "https://api.github.com/repos/Jmilburn1/Playground/releases/latest";

/// The environment variable that overrides the release
/// endpoint (test/rig hook; see the module docs).
pub const RELEASE_API_ENV: &str = "PLAYGROUND_RELEASE_API_URL";

/// The release asset the self-update installs — the one
/// asset .github/workflows/release.yml attaches for this
/// platform (Linux release, owner 2026-10-24). The swap
/// machinery already accepts both platforms' binary
/// spellings, so this name is the whole difference.
#[cfg(windows)]
pub const ASSET_NAME: &str = "Playground-Windows.zip";
#[cfg(not(windows))]
pub const ASSET_NAME: &str = "Playground-Linux.zip";

/// The app binary's file stem (".exe" on Windows only).
pub const GUI_EXE_STEM: &str = "playground";

/// The helper binary's file stem (update.exe's own).
pub const HELPER_EXE_STEM: &str = "update";

/// How long the apply helper waits for the GUI to exit
/// before it gives up and changes nothing.
const GUI_EXIT_TIMEOUT: Duration = Duration::from_secs(60);

/// One asset of the release the self-update installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The asset's sha256 from the release document,
    /// when it provides one (hex, "sha256:" stripped).
    pub sha256: Option<String>,
}

/// A newer Playground release, resolved to its one
/// installable asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRelease {
    /// The release tag as published ("v1.5.0").
    pub tag: String,
    /// The tag normalized for display/compare ("1.5.0").
    pub version: String,
    pub asset: AppAsset,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// The release endpoint in force: the environment
/// override when set and non-empty, else the default.
pub fn release_api_url() -> String {
    match std::env::var(RELEASE_API_ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => RELEASE_API_URL_DEFAULT.to_string(),
    }
}

/// True when `candidate` is a strictly newer version
/// than `current`. The comparison is the mod checks'
/// own (dotted numbers, a leading "v" forgiven, missing
/// parts counting as 0); a candidate with no parseable
/// numbers is never newer.
pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    crate::sources::github::is_newer(candidate, current)
}

/// Parse a "latest release" document into the
/// self-update's shape. Ok(None) when the release
/// carries no asset named [`ASSET_NAME`] — a release
/// with nothing installable is not an update. A
/// malformed document is an error.
pub fn parse_release(body: &str) -> Result<Option<AppRelease>> {
    let api: ApiRelease =
        serde_json::from_str(body).context("The release reply was not readable JSON")?;
    let Some(asset) = api.assets.into_iter().find(|a| a.name == ASSET_NAME) else {
        return Ok(None);
    };
    let sha256 = asset.digest.as_deref().and_then(|d| {
        d.strip_prefix("sha256:")
            .map(|hex| hex.trim().to_lowercase())
    });
    Ok(Some(AppRelease {
        version: crate::sources::github::parse_version(&api.tag_name)
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("."),
        tag: api.tag_name,
        asset: AppAsset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
            sha256,
        },
    }))
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Playground-Mod-Manager")
        .timeout(Duration::from_secs(30))
        .build()?)
}

/// Fetch and parse the latest release. Ok(None) when
/// the endpoint has no release (404) or the release has
/// nothing installable.
pub async fn fetch_latest() -> Result<Option<AppRelease>> {
    let url = release_api_url();
    let resp = http_client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Could not reach the release endpoint")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body = resp
        .error_for_status()
        .context("The release check was refused")?
        .text()
        .await
        .context("The release reply did not arrive")?;
    parse_release(&body)
}

/// The launch check: the latest release when it is
/// strictly newer than the running version, else None.
/// EVERY failure is None — the check is a courtesy, and
/// a failed one must read exactly like "no news".
pub async fn check_at_launch() -> Option<AppRelease> {
    let release = fetch_latest().await.ok()??;
    if is_newer_version(&release.tag, env!("CARGO_PKG_VERSION")) {
        Some(release)
    } else {
        None
    }
}

/// The staging root for app updates, under the data
/// dir: `app-update/staging` holds the download and the
/// extracted files, `app-update/app-update.log` is the
/// apply helper's written record of each swap attempt.
pub fn staging_root() -> Result<PathBuf> {
    let data = crate::paths::data_dir()
        .context("No data directory could be resolved for staging")?;
    Ok(data.join("app-update"))
}

/// The zip's GUI binary entry names, either platform's:
/// the release zip carries "playground.exe"; a dev/rig
/// build's zip carries the bare name. The entry's
/// presence is what makes a zip an app update at all.
fn gui_entry_names() -> [&'static str; 2] {
    ["playground.exe", "playground"]
}

/// The sha256 of a file, as lowercase hex.
pub fn sha256_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Could not open {:?} for hashing", path))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("Could not read {:?} for hashing", path))?;
    Ok(hex::encode(hasher.finalize()))
}

/// Verify a downloaded release zip BEFORE anything is
/// replaced: the byte count must equal the release
/// document's, the sha256 must match when the document
/// provided one, and the zip must open and contain the
/// app binary — whose entry is read end to end, so a
/// truncated or corrupt download fails on its CRC here
/// and not on the user's desktop later.
pub fn verify_download(zip_path: &Path, asset: &AppAsset) -> Result<()> {
    let actual_size = std::fs::metadata(zip_path)
        .with_context(|| format!("The downloaded {:?} is missing", zip_path))?
        .len();
    if actual_size != asset.size {
        bail!(
            "The download is incomplete: {} bytes arrived, the release promises {}",
            actual_size,
            asset.size
        );
    }
    if let Some(expected) = &asset.sha256 {
        let actual = sha256_hex(zip_path)?;
        if actual != *expected {
            bail!(
                "The download's checksum does not match the release (expected {}, got {})",
                expected,
                actual
            );
        }
    }
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("Could not open the downloaded {:?}", zip_path))?;
    let mut archive = zip::ZipArchive::new(file)
        .context("The download is not a readable zip — it may be truncated")?;
    let entry = gui_entry_names()
        .iter()
        .find(|name| archive.by_name(name).is_ok())
        .copied();
    let Some(entry) = entry else {
        bail!(
            "The zip does not contain {} — it is not an app update",
            gui_entry_names()[0]
        );
    };
    let mut entry_file = archive
        .by_name(entry)
        .with_context(|| format!("Could not read {} in the zip", entry))?;
    std::io::copy(&mut entry_file, &mut std::io::sink())
        .context("The app binary inside the zip is damaged")?;
    Ok(())
}

/// What an extracted update holds: the new app binary,
/// and the new helper when the zip carried one (the
/// current release workflow's zip does NOT — see the
/// repo note in the release workflow; the swap handles
/// its absence by simply leaving the helper in place).
#[derive(Debug)]
pub struct ExtractedUpdate {
    pub gui: PathBuf,
    pub helper: Option<PathBuf>,
}

/// Extract an update zip's payload into `dest` (created
/// fresh by the caller): the app binary, the helper
/// when present, and the README. Entry paths are taken
/// through the zip crate's enclosed-name check, so a
/// hostile entry can never write outside `dest`.
pub fn extract_update(zip_path: &Path, dest: &Path) -> Result<ExtractedUpdate> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Could not create the staging folder {:?}", dest))?;
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("Could not open the downloaded {:?}", zip_path))?;
    let mut archive = zip::ZipArchive::new(file)
        .context("The download is not a readable zip — it may be truncated")?;
    let wanted = [
        "playground.exe",
        "playground",
        "update.exe",
        "update",
        "README.md",
    ];
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let Some(file_name) = rel.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !wanted.contains(&file_name) {
            continue;
        }
        let out = dest.join(file_name);
        let mut sink = std::fs::File::create(&out)
            .with_context(|| format!("Could not stage {:?}", out))?;
        std::io::copy(&mut entry, &mut sink)
            .with_context(|| format!("Could not extract {}", file_name))?;
        #[cfg(unix)]
        if file_name != "README.md" {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755));
        }
    }
    let gui = gui_entry_names()
        .iter()
        .map(|name| dest.join(name))
        .find(|p| p.is_file())
        .context("The zip does not contain the app binary — it is not an app update")?;
    let helper = ["update.exe", "update"]
        .iter()
        .map(|name| dest.join(name))
        .find(|p| p.is_file());
    Ok(ExtractedUpdate { gui, helper })
}

/// Stream a URL to a file, returning the byte count.
/// (The mod sources' downloader carries a repo token;
/// the app's own release needs none, so this one is
/// local — same streaming shape, no partial files: the
/// caller verifies before anything is replaced.)
async fn download_to(url: &str, dest: &Path) -> Result<u64> {
    use futures::StreamExt;
    let resp = http_client()?
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .context("The update download did not start")?
        .error_for_status()
        .context("The update download was refused")?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("Could not create {:?}", dest))?;
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("The update download was interrupted")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .context("Could not write the downloaded update")?;
        written += chunk.len() as u64;
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .context("Could not finish the downloaded update")?;
    Ok(written)
}

/// The Install click's whole job: download the release
/// zip completely into staging, verify it there, and
/// extract it — returning the folder holding the new
/// binaries, ready for the apply helper. Nothing
/// outside staging is touched; a failure anywhere
/// leaves the installed app exactly as it was.
pub async fn download_and_stage(release: AppRelease) -> Result<PathBuf> {
    let root = staging_root()?;
    let staging = root.join("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .context("Could not clear the previous update staging")?;
    }
    std::fs::create_dir_all(&staging)
        .context("Could not create the update staging folder")?;
    let zip_path = staging.join(&release.asset.name);
    download_to(&release.asset.url, &zip_path).await?;
    let asset = release.asset.clone();
    let zip_for_verify = zip_path.clone();
    let files = staging.join("files");
    let files_for_extract = files.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        verify_download(&zip_for_verify, &asset)?;
        extract_update(&zip_for_verify, &files_for_extract)?;
        Ok(())
    })
    .await
    .context("The staging job failed")??;
    Ok(files)
}

/// A binary's backup path: the file name with ".bak"
/// appended (`playground.exe` -> `playground.exe.bak`).
fn bak_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// Find a staged binary in the extracted files: the
/// exact target file name wins, then either platform's
/// spelling of the stem.
fn staged_binary(files: &Path, target_file_name: &str, stem: &str) -> Option<PathBuf> {
    let exact = files.join(target_file_name);
    if exact.is_file() {
        return Some(exact);
    }
    for candidate in [format!("{}.exe", stem), stem.to_string()] {
        let path = files.join(&candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Copy a binary into place, restoring its executable
/// bit on Unix (a plain copy already keeps it on
/// Windows; the staged file's mode carries it there).
fn copy_exec(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Put a backed-up binary back where it was.
fn restore(target: &Path, bak: &Path) {
    let _ = std::fs::remove_file(target);
    let _ = std::fs::rename(bak, target);
}

/// The swap's outcome, for the helper's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapOutcome {
    /// The outgoing app binary was set aside as a .bak.
    pub gui_backed_up: bool,
    /// The helper binary itself was replaced.
    pub helper_swapped: bool,
}

/// Replace the installed binaries with the staged ones:
/// the app exe (required — the staged files are
/// validated BEFORE the install is touched), and the
/// helper exe when the update carries one. Each
/// outgoing binary is renamed to its .bak first; any
/// failure after that point restores the backups, so
/// the install is never left half-swapped.
pub fn perform_swap(files: &Path, target_gui: &Path) -> Result<SwapOutcome> {
    let install_dir = target_gui
        .parent()
        .context("The app exe has no parent directory")?;
    let gui_name = target_gui
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("The app exe has no file name")?;
    let staged_gui = staged_binary(files, &gui_name, GUI_EXE_STEM)
        .context("The staged update holds no app binary — nothing was changed")?;
    let suffix = if gui_name.ends_with(".exe") { ".exe" } else { "" };
    let helper_name = format!("{}{}", HELPER_EXE_STEM, suffix);
    let target_helper = install_dir.join(&helper_name);
    let staged_helper = staged_binary(files, &helper_name, HELPER_EXE_STEM);

    let gui_bak = bak_path(target_gui);
    let mut gui_backed_up = false;
    if target_gui.exists() {
        let _ = std::fs::remove_file(&gui_bak);
        std::fs::rename(target_gui, &gui_bak)
            .context("Could not set the current app binary aside")?;
        gui_backed_up = true;
    }
    if let Err(e) = copy_exec(&staged_gui, target_gui) {
        if gui_backed_up {
            restore(target_gui, &gui_bak);
        }
        return Err(e).context("Installing the new app binary failed — the old one was restored");
    }

    let mut helper_swapped = false;
    if let Some(staged_helper) = staged_helper {
        let helper_bak = bak_path(&target_helper);
        let mut helper_backed_up = false;
        if target_helper.exists() {
            let _ = std::fs::remove_file(&helper_bak);
            if std::fs::rename(&target_helper, &helper_bak).is_ok() {
                helper_backed_up = true;
            }
        }
        match copy_exec(&staged_helper, &target_helper) {
            Ok(()) => helper_swapped = true,
            Err(e) => {
                if helper_backed_up {
                    restore(&target_helper, &helper_bak);
                }
                if gui_backed_up {
                    restore(target_gui, &gui_bak);
                }
                return Err(e).context(
                    "Installing the new update helper failed — the whole update was rolled back",
                );
            }
        }
    }
    Ok(SwapOutcome {
        gui_backed_up,
        helper_swapped,
    })
}

/// Restore both binaries from their .bak backups (the
/// apply helper's last resort, when a freshly swapped
/// app binary will not even start).
pub fn rollback_from_backups(target_gui: &Path) -> Result<()> {
    let install_dir = target_gui
        .parent()
        .context("The app exe has no parent directory")?;
    let gui_name = target_gui
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("The app exe has no file name")?;
    let suffix = if gui_name.ends_with(".exe") { ".exe" } else { "" };
    let targets = [
        target_gui.to_path_buf(),
        install_dir.join(format!("{}{}", HELPER_EXE_STEM, suffix)),
    ];
    let mut restored_any = false;
    for target in &targets {
        let bak = bak_path(target);
        if bak.exists() {
            restore(target, &bak);
            restored_any = true;
        }
    }
    if restored_any {
        Ok(())
    } else {
        bail!("No backups were found to restore")
    }
}

/// Delete the stale .bak backups beside the app exe.
/// Called by the GUI once a boot load has completed:
/// see the module docs for why that is the moment.
pub fn cleanup_stale_backups() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cleanup_stale_backups_in(dir);
        }
    }
}

/// [`cleanup_stale_backups`] factored over a directory,
/// for tests.
pub fn cleanup_stale_backups_in(dir: &Path) {
    for stem in [GUI_EXE_STEM, HELPER_EXE_STEM] {
        for name in [format!("{}.bak", stem), format!("{}.exe.bak", stem)] {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
}

/// Launch the app binary at `path`, detached, from its
/// own directory.
pub fn relaunch(path: &Path) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(path);
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn()
}

/// True once the process `pid` no longer exists (or
/// within `timeout`, false). A pid of 0 — and this
/// process's own pid — count as already gone: the
/// helper must never wait on itself.
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    if pid == 0 || pid == std::process::id() {
        return true;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if !process_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use std::ffi::c_void;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }
    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            // No such process: it has already exited.
            // (Any other failure reads as "alive" — the
            // caller waits and gives up on its deadline
            // rather than swapping under a live app.)
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let result = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        result == WAIT_TIMEOUT
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const ESRCH: i32 = 3;
    unsafe {
        if kill(pid as i32, 0) != 0 {
            // ESRCH: no such process — gone. EPERM and
            // every other answer mean it exists.
            return std::io::Error::last_os_error().raw_os_error() != Some(ESRCH);
        }
    }
    // kill says it exists — but a zombie exists too, and
    // a zombie is an exited process whose parent has not
    // reaped it yet. An unreaped GUI would otherwise read
    // as alive forever and the swap would never start.
    // (Linux /proc; where it cannot be read, the kill
    // answer stands.)
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
        // The state is the field after the comm's closing
        // paren — comm itself may hold spaces and parens.
        if let Some(state) = stat
            .rfind(')')
            .and_then(|i| stat[i + 1..].split_whitespace().next())
        {
            if state == "Z" {
                return false;
            }
        }
    }
    true
}

#[cfg(not(any(windows, unix)))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// Spawn update.exe's apply verb for the staged files,
/// from the GUI: the helper sits beside the app exe.
pub fn spawn_apply_helper(files: &Path) -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no install dir"))?;
    let suffix = if exe
        .file_name()
        .map(|n| n.to_string_lossy().ends_with(".exe"))
        .unwrap_or(false)
    {
        ".exe"
    } else {
        ""
    };
    let helper = install_dir.join(format!("{}{}", HELPER_EXE_STEM, suffix));
    let mut cmd = std::process::Command::new(&helper);
    cmd.arg("apply-app-update")
        .arg("--staging")
        .arg(files)
        .arg("--target")
        .arg(&exe)
        .arg("--pid")
        .arg(std::process::id().to_string());
    // The helper's log belongs in the staging ROOT, not
    // inside the staging folder — the folder is wiped at
    // the start of every install, which would erase the
    // record of the last one.
    if let Ok(root) = staging_root() {
        cmd.arg("--log").arg(root.join("app-update.log"));
    }
    cmd.spawn()
}

/// Append one timestamped line to the apply helper's
/// log. Logging never fails the update.
fn append_log(log_path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "[{}] {}", stamp, line);
    }
}

/// update.exe's `apply-app-update` verb, whole: wait
/// for the GUI to exit, swap the binaries, relaunch.
/// Any failure restores the previous version and
/// relaunches THAT — the owner is never left without
/// a working exe. `log_path` (the app-update.log in
/// the staging root) records every step.
pub fn apply_app_update(
    files: &Path,
    target_gui: &Path,
    gui_pid: u32,
    log_path: Option<&Path>,
) -> Result<()> {
    let log = |msg: String| {
        eprintln!("[apply-app-update] {}", msg);
        if let Some(path) = log_path {
            append_log(path, &msg);
        }
    };
    log(format!(
        "applying the staged update in {} to {}",
        files.display(),
        target_gui.display()
    ));
    if !wait_for_process_exit(gui_pid, GUI_EXIT_TIMEOUT) {
        log("the app did not exit in time — nothing was changed".to_string());
        bail!("The app did not exit in time — the update was not applied and nothing was changed");
    }
    match perform_swap(files, target_gui) {
        Ok(outcome) => {
            log(format!(
                "swap complete (app backup kept: {}, helper swapped: {})",
                outcome.gui_backed_up, outcome.helper_swapped
            ));
            match relaunch(target_gui) {
                Ok(_) => {
                    log("the new version was launched".to_string());
                    Ok(())
                }
                Err(e) => {
                    log(format!(
                        "the new version would not start ({}); restoring the previous one",
                        e
                    ));
                    let _ = rollback_from_backups(target_gui);
                    match relaunch(target_gui) {
                        Ok(_) => bail!(
                            "The new version would not start; the previous version was restored and relaunched"
                        ),
                        Err(e2) => bail!(
                            "The new version would not start ({}) and the restored one would not relaunch ({})",
                            e, e2
                        ),
                    }
                }
            }
        }
        Err(e) => {
            log(format!(
                "the swap failed ({:#}); relaunching the current version",
                e
            ));
            let _ = relaunch(target_gui);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "playground-app-update-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// A release document in the GitHub API's shape,
    /// digest and all, with an unrelated asset in front
    /// of the one the self-update installs.
    const RELEASE_JSON: &str = r#"{
        "tag_name": "v1.5.0",
        "name": "Playground v1.5.0",
        "assets": [
            {
                "name": "notes.txt",
                "browser_download_url": "https://example.invalid/notes.txt",
                "size": 12,
                "digest": null
            },
            {
                "name": "Playground-Windows.zip",
                "browser_download_url": "https://example.invalid/Playground-Windows.zip",
                "size": 107861,
                "digest": "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
            }
        ]
    }"#;

    #[test]
    fn versions_compare_strictly() {
        assert!(is_newer_version("v1.5.0", "1.4.1"));
        assert!(is_newer_version("1.4.2", "1.4.1"));
        assert!(is_newer_version("v1.10.0", "1.9.9"));
        assert!(is_newer_version("2", "1.4.1"));
        assert!(!is_newer_version("v1.4.1", "1.4.1"), "equal is not newer");
        assert!(!is_newer_version("1.4.1", "v1.4.1"), "equal, either prefix");
        assert!(!is_newer_version("v1.4.0", "1.4.1"), "older is not newer");
        assert!(!is_newer_version("1.3.9", "1.4.1"));
        assert!(!is_newer_version("nightly", "1.4.1"), "no numbers, no update");
    }

    #[test]
    fn release_json_parses_to_the_installable_asset() {
        // The fixture names the Windows asset; judge the
        // platform's own asset name so the test means the
        // same thing on both platforms.
        let body = RELEASE_JSON.replace("Playground-Windows.zip", ASSET_NAME);
        let release = parse_release(&body).unwrap().unwrap();
        assert_eq!(release.tag, "v1.5.0");
        assert_eq!(release.version, "1.5.0");
        assert_eq!(release.asset.name, ASSET_NAME);
        assert_eq!(
            release.asset.url,
            format!("https://example.invalid/{}", ASSET_NAME)
        );
        assert_eq!(release.asset.size, 107861);
        assert_eq!(
            release.asset.sha256.as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            "the digest's sha256: prefix is stripped and the hex lowered"
        );
    }

    #[test]
    fn release_without_the_zip_asset_is_no_update() {
        let body = r#"{ "tag_name": "v9.9.9", "assets": [
            { "name": "source.tar.gz",
              "browser_download_url": "https://example.invalid/s.tgz",
              "size": 5 }
        ] }"#;
        assert_eq!(parse_release(body).unwrap(), None);
        let body = r#"{ "tag_name": "v9.9.9", "assets": [] }"#;
        assert_eq!(parse_release(body).unwrap(), None);
    }

    #[test]
    fn release_document_must_be_json() {
        assert!(parse_release("not json at all").is_err());
        assert!(parse_release("{}").is_err(), "no tag_name, no release");
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn asset_for(zip_path: &Path, sha256: Option<String>) -> AppAsset {
        AppAsset {
            name: ASSET_NAME.to_string(),
            url: "https://example.invalid/Playground-Windows.zip".to_string(),
            size: std::fs::metadata(zip_path).unwrap().len(),
            sha256,
        }
    }

    #[test]
    fn a_good_zip_verifies() {
        let root = temp_root("verify-good");
        let zip_path = root.join(ASSET_NAME);
        write_zip(
            &zip_path,
            &[
                ("playground.exe", b"new-app-binary"),
                ("README.md", b"notes"),
            ],
        );
        let digest = sha256_hex(&zip_path).unwrap();
        verify_download(&zip_path, &asset_for(&zip_path, Some(digest))).unwrap();
        // No digest in the document: size + zip sanity still verify.
        verify_download(&zip_path, &asset_for(&zip_path, None)).unwrap();
    }

    #[test]
    fn a_truncated_zip_is_rejected() {
        let root = temp_root("verify-truncated");
        let zip_path = root.join(ASSET_NAME);
        write_zip(&zip_path, &[("playground.exe", b"new-app-binary")]);
        let asset = asset_for(&zip_path, None);
        let bytes = std::fs::read(&zip_path).unwrap();
        std::fs::write(&zip_path, &bytes[..bytes.len() / 2]).unwrap();
        // The size no longer matches the release's word.
        assert!(verify_download(&zip_path, &asset).is_err());
        // And padded back to size, the zip itself is unreadable
        // (the central directory lived in the cut half).
        let mut padded = std::fs::read(&zip_path).unwrap();
        padded.resize(asset.size as usize, 0);
        std::fs::write(&zip_path, &padded).unwrap();
        assert!(verify_download(&zip_path, &asset).is_err());
    }

    #[test]
    fn a_corrupt_zip_is_rejected() {
        let root = temp_root("verify-corrupt");
        let zip_path = root.join(ASSET_NAME);
        std::fs::write(&zip_path, b"this is not a zip, just bytes of the right size")
            .unwrap();
        let asset = asset_for(&zip_path, None);
        assert!(verify_download(&zip_path, &asset).is_err());
    }

    #[test]
    fn a_checksum_mismatch_is_rejected() {
        let root = temp_root("verify-sha");
        let zip_path = root.join(ASSET_NAME);
        write_zip(&zip_path, &[("playground.exe", b"new-app-binary")]);
        let wrong = "0".repeat(64);
        assert!(verify_download(&zip_path, &asset_for(&zip_path, Some(wrong))).is_err());
    }

    #[test]
    fn a_zip_without_the_app_binary_is_rejected() {
        let root = temp_root("verify-noexe");
        let zip_path = root.join(ASSET_NAME);
        write_zip(&zip_path, &[("README.md", b"just notes")]);
        assert!(verify_download(&zip_path, &asset_for(&zip_path, None)).is_err());
    }

    #[test]
    fn extraction_lands_the_binaries() {
        let root = temp_root("extract");
        let zip_path = root.join(ASSET_NAME);
        write_zip(
            &zip_path,
            &[
                ("playground.exe", b"new-app"),
                ("update.exe", b"new-helper"),
                ("README.md", b"notes"),
                ("unrelated.txt", b"left behind"),
            ],
        );
        let files = root.join("files");
        let extracted = extract_update(&zip_path, &files).unwrap();
        assert_eq!(std::fs::read(&extracted.gui).unwrap(), b"new-app");
        assert_eq!(
            std::fs::read(extracted.helper.as_ref().unwrap()).unwrap(),
            b"new-helper"
        );
        assert!(files.join("README.md").is_file());
        assert!(
            !files.join("unrelated.txt").exists(),
            "only the release's payload is staged"
        );
    }

    #[test]
    fn extraction_without_the_app_binary_fails() {
        let root = temp_root("extract-noexe");
        let zip_path = root.join(ASSET_NAME);
        write_zip(&zip_path, &[("update.exe", b"helper only")]);
        assert!(extract_update(&zip_path, &root.join("files")).is_err());
    }

    #[test]
    fn the_swap_replaces_both_binaries_and_keeps_backups() {
        let root = temp_root("swap");
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let target = install.join("playground");
        std::fs::write(&target, b"old-app").unwrap();
        std::fs::write(install.join("update"), b"old-helper").unwrap();
        let files = root.join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("playground"), b"new-app").unwrap();
        std::fs::write(files.join("update"), b"new-helper").unwrap();

        let outcome = perform_swap(&files, &target).unwrap();
        assert!(outcome.gui_backed_up);
        assert!(outcome.helper_swapped);
        assert_eq!(std::fs::read(&target).unwrap(), b"new-app");
        assert_eq!(std::fs::read(install.join("update")).unwrap(), b"new-helper");
        assert_eq!(
            std::fs::read(install.join("playground.bak")).unwrap(),
            b"old-app",
            "the outgoing app binary is the backup"
        );
        assert_eq!(
            std::fs::read(install.join("update.bak")).unwrap(),
            b"old-helper"
        );

        // And the backups restore the old pair whole.
        rollback_from_backups(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old-app");
        assert_eq!(std::fs::read(install.join("update")).unwrap(), b"old-helper");
    }

    #[test]
    fn the_swap_without_a_staged_helper_leaves_the_helper_alone() {
        // The current release workflow's zip carries no
        // update.exe (see the module docs): the swap must
        // still land the app and not invent a helper swap.
        let root = temp_root("swap-nohelper");
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let target = install.join("playground.exe");
        std::fs::write(&target, b"old-app").unwrap();
        std::fs::write(install.join("update.exe"), b"old-helper").unwrap();
        let files = root.join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("playground.exe"), b"new-app").unwrap();

        let outcome = perform_swap(&files, &target).unwrap();
        assert!(outcome.gui_backed_up);
        assert!(!outcome.helper_swapped);
        assert_eq!(std::fs::read(&target).unwrap(), b"new-app");
        assert_eq!(
            std::fs::read(install.join("update.exe")).unwrap(),
            b"old-helper",
            "the helper on disk is untouched"
        );
        assert!(!install.join("update.exe.bak").exists());
    }

    #[test]
    fn a_swap_with_no_staged_app_binary_changes_nothing() {
        let root = temp_root("swap-empty");
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let target = install.join("playground");
        std::fs::write(&target, b"old-app").unwrap();
        let files = root.join("files");
        std::fs::create_dir_all(&files).unwrap();

        assert!(perform_swap(&files, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old-app");
        assert!(
            !install.join("playground.bak").exists(),
            "validation happens before the install is touched"
        );
    }

    #[test]
    fn stale_backup_cleanup_removes_only_backups() {
        let root = temp_root("cleanup");
        for name in ["playground.exe.bak", "update.exe.bak", "playground.bak"] {
            std::fs::write(root.join(name), b"bak").unwrap();
        }
        std::fs::write(root.join("playground.exe"), b"app").unwrap();
        cleanup_stale_backups_in(&root);
        assert!(!root.join("playground.exe.bak").exists());
        assert!(!root.join("update.exe.bak").exists());
        assert!(!root.join("playground.bak").exists());
        assert!(root.join("playground.exe").is_file(), "the app itself stays");
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_waiting_sees_both_sides() {
        assert!(wait_for_process_exit(0, Duration::from_millis(10)));
        assert!(
            wait_for_process_exit(u32::MAX - 4242, Duration::from_secs(2)),
            "a pid that cannot exist reads as exited"
        );
        let mut child = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(
            !wait_for_process_exit(pid, Duration::from_millis(150)),
            "a live process is not exited"
        );
        assert!(wait_for_process_exit(pid, Duration::from_secs(5)));
        let _ = child.wait();
    }
}
