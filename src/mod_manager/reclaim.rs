//! Reclaim disk space (owner ruling, 2026-10-23): the
//! Layout page's cleanup, over exactly the two pools
//! the owner named.
//!
//! - BACKUPS: every backup family the app keeps is
//!   pruned to its newest entry. Older Keybindings
//!   backups, Game Screens captures and pre-layout
//!   backups are deleted; the newest of each family
//!   always survives, and the backup a recorded
//!   migration's rollback points at survives with it.
//! - DUPLICATES: exact duplicate mod copies — same
//!   file name AND same version — are reduced to one
//!   copy. The survivor is the Active copy when there
//!   is one, else the library copy ("the Active copy
//!   wins" is the library's standing identity rule).
//!   A "copy" that is the same underlying object as
//!   the survivor (a hardlink twin, a junction to it)
//!   is not a duplicate at all and is never touched.
//!
//! The census is read-only. Deletion happens only
//! from the page's armed Reclaim button, and every
//! run writes its record to the data folder's logs/.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{Mod, ModState};

/// One backup family: a labelled set of timestamped
/// entries in one place. Entries are newest first —
/// every family's names carry zero-padded timestamps,
/// so name order IS chronological order (the same
/// convention keybindings::list_backups documents).
#[derive(Debug, Clone)]
pub struct BackupFamily {
    pub label: String,
    /// All entries, newest first.
    pub entries: Vec<PathBuf>,
    /// Size per entry, parallel to `entries`.
    pub sizes: Vec<u64>,
    /// An extra keeper outside the newest: the backup
    /// a recorded migration's rollback points at. Never
    /// pruned, even when it is not the newest.
    pub protected: Option<PathBuf>,
}

impl BackupFamily {
    fn is_prunable(&self, path: &Path) -> bool {
        Some(path) != self.entries.first().map(|p| p.as_path())
            && Some(path) != self.protected.as_deref()
    }

    /// The entries a prune would delete: everything
    /// except the newest and the protected one.
    pub fn prunable(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|p| self.is_prunable(p))
            .cloned()
            .collect()
    }

    pub fn prunable_bytes(&self) -> u64 {
        self.entries
            .iter()
            .zip(&self.sizes)
            .filter(|(p, _)| self.is_prunable(p))
            .map(|(_, s)| *s)
            .sum()
    }
}

/// One exact-duplicate set: records sharing a file
/// name and a version, reduced to a single survivor.
#[derive(Debug, Clone)]
pub struct DupSet {
    pub file_name: String,
    pub version: String,
    pub keep_id: String,
    pub keep_path: PathBuf,
    /// (mod id, path, size) copies a run would remove.
    pub remove: Vec<(String, PathBuf, u64)>,
    /// The removed copies' own sizes summed.
    pub bytes: u64,
}

/// The whole census: backup families plus duplicate
/// sets, with the totals the page reports.
#[derive(Debug, Clone, Default)]
pub struct ReclaimCensus {
    pub families: Vec<BackupFamily>,
    pub dup_sets: Vec<DupSet>,
}

impl ReclaimCensus {
    pub fn prunable_backups(&self) -> usize {
        self.families.iter().map(|f| f.prunable().len()).sum()
    }

    pub fn prunable_backup_bytes(&self) -> u64 {
        self.families.iter().map(|f| f.prunable_bytes()).sum()
    }

    pub fn duplicate_copies(&self) -> usize {
        self.dup_sets.iter().map(|d| d.remove.len()).sum()
    }

    pub fn duplicate_bytes(&self) -> u64 {
        self.dup_sets.iter().map(|d| d.bytes).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.prunable_backup_bytes() + self.duplicate_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.prunable_backups() == 0 && self.duplicate_copies() == 0
    }
}

/// Total size of a file or a folder tree, in bytes.
/// Missing paths measure zero.
pub fn entry_size(path: &Path) -> u64 {
    let Ok(md) = std::fs::metadata(path) else {
        return 0;
    };
    if md.is_file() {
        return md.len();
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Whether two paths name the same underlying object:
/// the same canonical path, or (Unix) the same device
/// and inode — the hardlink test. Keeps a survivor's
/// own twins out of the duplicate pool.
pub fn same_object(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(a) == canon(b) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return ma.dev() == mb.dev() && ma.ino() == mb.ino();
        }
    }
    false
}

fn family_from_dir(label: &str, dir: &Path) -> Option<BackupFamily> {
    let read = std::fs::read_dir(dir).ok()?;
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    entries.reverse();
    if entries.is_empty() {
        return None;
    }
    let sizes = entries.iter().map(|p| entry_size(p)).collect();
    Some(BackupFamily {
        label: label.to_string(),
        entries,
        sizes,
        protected: None,
    })
}

/// Census the backup families. `game_dir` is the game
/// mods folder: pre-layout backups are its timestamped
/// siblings. `protected_backup` is the recorded
/// migration's rollback target, when one is recorded.
pub fn census_backups(
    profile: Option<&Path>,
    data_dir: Option<&Path>,
    game_dir: &Path,
    protected_backup: Option<&Path>,
) -> Vec<BackupFamily> {
    let mut out = Vec::new();
    if let Some(profile) = profile {
        if let Some(f) = family_from_dir(
            "Keybindings backups",
            &super::keybindings::backups_dir(profile),
        ) {
            out.push(f);
        }
    }
    if let Some(data_dir) = data_dir {
        if let Some(f) = family_from_dir(
            "Game Screens backups",
            &super::gamescreens::backups_root(data_dir),
        ) {
            out.push(f);
        }
    }
    if let (Some(parent), Some(game_name)) = (game_dir.parent(), game_dir.file_name()) {
        let prefix = format!("{}-prelayout-backup-", game_name.to_string_lossy());
        if let Ok(read) = std::fs::read_dir(parent) {
            let mut entries: Vec<PathBuf> = read
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().starts_with(&prefix))
                        .unwrap_or(false)
                })
                .collect();
            entries.sort();
            entries.reverse();
            if !entries.is_empty() {
                let sizes = entries.iter().map(|p| entry_size(p)).collect();
                out.push(BackupFamily {
                    label: "Pre-layout backups".to_string(),
                    entries,
                    sizes,
                    protected: protected_backup.map(|p| p.to_path_buf()),
                });
            }
        }
    }
    out
}

/// Group records by exact (file name, version) and
/// reduce each group of 2+ to its survivor. Survivor
/// order: an Active copy first, then a copy under the
/// library root, then the first path. A copy that is
/// the survivor's own twin (same_object) is left out
/// of the remove list — it holds no second copy of
/// the data.
pub fn census_duplicates<'m>(
    mods: impl IntoIterator<Item = &'m Mod>,
    library_root: &Path,
) -> Vec<DupSet> {
    let mut groups: HashMap<(String, String), Vec<&Mod>> = HashMap::new();
    for m in mods {
        groups
            .entry((m.file_name(), m.version.clone()))
            .or_default()
            .push(m);
    }
    let mut sets = Vec::new();
    for ((file_name, version), mut copies) in groups {
        if copies.len() < 2 {
            continue;
        }
        copies.sort_by(|a, b| {
            let rank = |m: &Mod| {
                (
                    std::cmp::Reverse(m.state == ModState::Active),
                    std::cmp::Reverse(m.path.starts_with(library_root)),
                )
            };
            rank(a).cmp(&rank(b)).then_with(|| a.path.cmp(&b.path))
        });
        let keeper = copies[0];
        let mut remove = Vec::new();
        let mut bytes = 0u64;
        for m in &copies[1..] {
            if same_object(&m.path, &keeper.path) {
                continue;
            }
            remove.push((m.id.clone(), m.path.clone(), m.size));
            bytes += m.size;
        }
        if remove.is_empty() {
            continue;
        }
        sets.push(DupSet {
            file_name,
            version,
            keep_id: keeper.id.clone(),
            keep_path: keeper.path.clone(),
            remove,
            bytes,
        });
    }
    sets.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    sets
}

/// The outcome of pruning one family.
pub struct PruneOutcome {
    pub removed: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

/// Delete a family's prunable entries. A failure on
/// one entry is reported and never stops the rest.
pub fn prune_family(family: &BackupFamily) -> PruneOutcome {
    let mut out = PruneOutcome {
        removed: 0,
        bytes: 0,
        errors: Vec::new(),
    };
    for (path, size) in family.entries.iter().zip(&family.sizes) {
        if !family.is_prunable(path) {
            continue;
        }
        let res = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match res {
            Ok(()) => {
                out.removed += 1;
                out.bytes += size;
            }
            Err(e) => out.errors.push(format!("{}: {}", path.display(), e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_mod(path: &str, version: &str, state: ModState, size: u64) -> Mod {
        Mod {
            id: format!("id-{}", path),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            version: version.to_string(),
            author: None,
            description: None,
            mod_type: super::super::types::ModType::Zip,
            state,
            path: PathBuf::from(path),
            size,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            game_versions: Vec::new(),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            source: None,
            download_url: None,
            hash: None,
            icon_filename: None,
            categories: Vec::new(),
            declared_actions: Vec::new(),
            action_binding_refs: Vec::new(),
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "reclaim-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn duplicates_group_on_name_and_version_and_the_active_copy_wins() {
        let lib = Path::new("/lib");
        let active = test_mod("/game/FS25_A.zip", "1.0", ModState::Active, 100);
        let archived = test_mod("/lib/FS25_A.zip", "1.0", ModState::Archived, 100);
        let other_version = test_mod("/lib/sub/FS25_A.zip", "2.0", ModState::Archived, 100);
        let sets = census_duplicates([&active, &archived, &other_version], lib);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].keep_id, active.id);
        assert_eq!(sets[0].remove.len(), 1);
        assert_eq!(sets[0].remove[0].0, archived.id);
        assert_eq!(sets[0].bytes, 100);
    }

    #[test]
    fn duplicates_without_an_active_copy_keep_the_library_one() {
        let lib = Path::new("/lib");
        let in_game = test_mod("/game/FS25_B.zip", "1.0", ModState::Archived, 50);
        let in_lib = test_mod("/lib/FS25_B.zip", "1.0", ModState::Archived, 50);
        let sets = census_duplicates([&in_game, &in_lib], lib);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].keep_id, in_lib.id);
    }

    #[cfg(unix)]
    #[test]
    fn a_hardlink_twin_is_not_a_duplicate() {
        let base = temp_root("twin");
        let real = base.join("FS25_Twin.zip");
        std::fs::write(&real, b"data").unwrap();
        let twin = base.join("elsewhere.zip");
        std::fs::hard_link(&real, &twin).unwrap();
        // The twin needs the same file name to group;
        // use a subdir so both carry the mod's name.
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let twin2 = sub.join("FS25_Twin.zip");
        std::fs::hard_link(&real, &twin2).unwrap();
        let a = test_mod(real.to_str().unwrap(), "1.0", ModState::Archived, 4);
        let mut b = test_mod(twin2.to_str().unwrap(), "1.0", ModState::Archived, 4);
        b.name = "FS25_Twin.zip".to_string();
        // file_name() reads the path's file name, so the
        // records group; the twin must still be spared.
        let sets = census_duplicates([&a, &b], &base);
        assert!(sets.is_empty(), "twin set should be empty: {:?}", sets);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_keeps_the_newest_and_any_protected_entry() {
        let base = temp_root("family");
        let dir = base.join("inputBinding-backups");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in [
            ("inputBinding-20260101-000000.xml", "old"),
            ("inputBinding-20260102-000000.xml", "mid"),
            ("inputBinding-20260103-000000.xml", "new"),
        ] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let mut family = family_from_dir("Keybindings backups", &dir).unwrap();
        assert_eq!(family.entries.len(), 3);
        assert_eq!(family.prunable().len(), 2);
        // Protect the middle entry: only the oldest goes.
        family.protected = Some(dir.join("inputBinding-20260102-000000.xml"));
        assert_eq!(family.prunable().len(), 1);
        let outcome = prune_family(&family);
        assert_eq!(outcome.removed, 1);
        assert!(dir.join("inputBinding-20260103-000000.xml").exists());
        assert!(dir.join("inputBinding-20260102-000000.xml").exists());
        assert!(!dir.join("inputBinding-20260101-000000.xml").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
