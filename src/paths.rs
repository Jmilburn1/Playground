//! The app's data directory — the ONE place that
//! resolves it, shared by the GUI, the CLI verbs and
//! update.exe alike (owner ruling, 2026-10-21: the rename
//! to Playground goes "right across the board").
//!
//! The directory is `~/.config/playground` now; it was
//! `~/.config/fs-mod-manager` up to v1.3.4. Everything
//! the app keeps — config.toml, the catalogs, the library
//! database, the layout state and logs — lives under it,
//! so the rename carries an automatic one-time migration:
//! on resolve, if the new directory does not exist and
//! the old one does, the old directory is MOVED to the
//! new name (a rename; a recursive copy if the rename
//! itself fails), contents wholesale. If BOTH exist, the
//! new directory wins and the old one is left untouched
//! — no merge, no deletion: the new directory is the
//! live one, and whatever the old one still holds is
//! the owner's to inspect or remove by hand.

use std::path::{Path, PathBuf};

/// The data directory's current leaf name.
pub const DATA_DIR_NAME: &str = "playground";

/// The data directory's pre-rename leaf name (v1.3.4 and
/// earlier) — the migration's source.
pub const LEGACY_DATA_DIR_NAME: &str = "fs-mod-manager";

/// Resolve the data directory, migrating a legacy
/// directory first if one is owed. Returns the new
/// directory's path whether or not it exists yet —
/// creating it stays each caller's business, as before.
pub fn data_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(resolve_data_dir(&home.join(".config")))
}

/// [`data_dir`] factored over an explicit config root,
/// so the migration decision is unit-testable without
/// touching the real home.
pub fn resolve_data_dir(config_root: &Path) -> PathBuf {
    let new_dir = config_root.join(DATA_DIR_NAME);
    let old_dir = config_root.join(LEGACY_DATA_DIR_NAME);
    migrate_data_dir(&new_dir, &old_dir);
    new_dir
}

/// The migration decision itself:
/// - new exists (with or without old): no-op — the new
///   directory wins, the old one is left untouched.
/// - only old exists: move it to the new name.
/// - neither exists: no-op (nothing to carry).
pub fn migrate_data_dir(new_dir: &Path, old_dir: &Path) {
    if new_dir.exists() || !old_dir.exists() {
        return;
    }
    if std::fs::rename(old_dir, new_dir).is_ok() {
        return;
    }
    // The rename failed (a cross-device config root is
    // the realistic cause): fall back to a recursive
    // copy, then remove the source so the state is not
    // forked across two directories. A failed copy
    // leaves the old directory in place — the next
    // resolve retries, and nothing was lost.
    if copy_recursive(old_dir, new_dir).is_ok() {
        let _ = std::fs::remove_dir_all(old_dir);
    }
}

/// Copy a directory tree wholesale: every subdirectory
/// and every file's bytes.
fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "playground-paths-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn seed(dir: &Path, marker: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("config.toml"), marker).unwrap();
        std::fs::create_dir_all(dir.join("layout-runs")).unwrap();
        std::fs::write(dir.join("layout-runs").join("run.log"), "log-bytes").unwrap();
    }

    #[test]
    fn old_only_directory_moves_to_the_new_name() {
        let root = temp_root("old-only");
        let old = root.join(LEGACY_DATA_DIR_NAME);
        let new = root.join(DATA_DIR_NAME);
        seed(&old, "the-owner's-config");
        let resolved = resolve_data_dir(&root);
        assert_eq!(resolved, new);
        assert!(!old.exists(), "the old directory moved away");
        assert_eq!(
            std::fs::read_to_string(new.join("config.toml")).unwrap(),
            "the-owner's-config",
            "the config rides the move"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("layout-runs").join("run.log")).unwrap(),
            "log-bytes",
            "nested state rides the move wholesale"
        );
    }

    #[test]
    fn new_only_directory_is_a_no_op() {
        let root = temp_root("new-only");
        let new = root.join(DATA_DIR_NAME);
        seed(&new, "already-playground");
        let resolved = resolve_data_dir(&root);
        assert_eq!(resolved, new);
        assert_eq!(
            std::fs::read_to_string(new.join("config.toml")).unwrap(),
            "already-playground"
        );
        assert!(!root.join(LEGACY_DATA_DIR_NAME).exists());
    }

    #[test]
    fn both_exist_new_wins_and_old_is_left_untouched() {
        let root = temp_root("both");
        let old = root.join(LEGACY_DATA_DIR_NAME);
        let new = root.join(DATA_DIR_NAME);
        seed(&old, "legacy-config");
        seed(&new, "current-config");
        let resolved = resolve_data_dir(&root);
        assert_eq!(resolved, new);
        assert_eq!(
            std::fs::read_to_string(new.join("config.toml")).unwrap(),
            "current-config",
            "the new directory's state is the live one"
        );
        assert_eq!(
            std::fs::read_to_string(old.join("config.toml")).unwrap(),
            "legacy-config",
            "the old directory is neither merged nor deleted"
        );
    }

    #[test]
    fn neither_exists_creates_nothing() {
        let root = temp_root("neither");
        let resolved = resolve_data_dir(&root);
        assert_eq!(resolved, root.join(DATA_DIR_NAME));
        assert!(!resolved.exists(), "resolve alone creates nothing");
    }
}
