//! The "other sites" link store: catalog-other.json —
//! where mods whose home is GitHub, KingMods or itch.io
//! are linked, beside the ModHub catalog (catalog-hub)
//! and the personal update database (catalog-home.json).
//!
//! Design (update-database-design.md): the links database
//! for everything that isn't ModHub. Placed once, read at
//! launch, and db-fill polls the sources from it. It is
//! the store the per-mod GitHub links migrate INTO from
//! config.toml: the store wins, config.toml's `mod_links`
//! remain only as a fallback while the transition settles.
//!
//! Like the library database, this store is never a
//! dependency: a missing file is normal (nothing placed
//! yet), and a damaged one is quarantined (renamed to
//! catalog-other.json.damaged-<unix>, never wiped) and
//! treated as an empty store. Rows are looked up by the
//! two keys the project already trusts: the EXACT file
//! name (characters and case included) and the lowercase
//! file stem.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// One link: this file's home at one of the other sites.
/// `grade` carries the hunt's verdict (confirmed /
/// candidate / lead) or "manual" for a link a human set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OtherRow {
    pub file_name: String,
    pub stem: String,
    /// github | kingmods | itch
    pub source_kind: String,
    /// The locator at the source: a full page/repo URL.
    pub source_ref: String,
    pub grade: String,
}

/// On-disk envelope — the same shape as the project's
/// other JSON stores (format + generated_at + payload).
#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    format: u32,
    generated_at: u64,
    rows: Vec<OtherRow>,
}

pub struct CatalogOther {
    path: Option<PathBuf>,
    rows: Vec<OtherRow>,
}

/// Where the store lives: beside config.toml, the update
/// database and the library database.
pub fn store_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("catalog-other.json"),
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl CatalogOther {
    /// Load the store from its standard home. Never fails:
    /// a missing, unreadable or damaged file yields an
    /// empty store (and a quarantine rename, when there is
    /// a damaged file to set aside).
    pub fn load_default() -> Self {
        match store_path() {
            Some(path) => Self::load_from(path),
            None => Self::empty(None),
        }
    }

    /// Load from an explicit path (the standard home in
    /// production; a scratch path in tests).
    pub fn load_from(path: PathBuf) -> Self {
        let mut store = Self::empty(Some(path.clone()));
        let Ok(text) = std::fs::read_to_string(&path) else {
            // Missing (nothing placed yet) or unreadable:
            // an empty store, and nothing to quarantine.
            return store;
        };
        match serde_json::from_str::<StoreFile>(&text) {
            Ok(file) if file.format == 1 => {
                store.rows = file.rows;
            }
            _ => {
                // Damaged (or a stranger's format): set the
                // file aside under a damaged name — never
                // wiped — and carry on with an empty store.
                let damaged = path.with_file_name(format!(
                    "{}.damaged-{}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "catalog-other.json".to_string()),
                    now_secs()
                ));
                let _ = std::fs::rename(&path, &damaged);
            }
        }
        store
    }

    fn empty(path: Option<PathBuf>) -> Self {
        Self { path, rows: Vec::new() }
    }

    /// Every row in the store, in file order.
    pub fn rows(&self) -> &[OtherRow] {
        &self.rows
    }

    /// Lookup by the exact file name — characters and case
    /// included; this is the identity test, so a name that
    /// differs only in case is a different mod and misses.
    pub fn by_file_name(&self, file_name: &str) -> Option<&OtherRow> {
        self.rows.iter().find(|r| r.file_name == file_name)
    }

    /// Lookup by lowercase file stem — the join key the
    /// rest of the project already uses. First row wins.
    pub fn by_stem(&self, stem: &str) -> Option<&OtherRow> {
        self.rows.iter().find(|r| r.stem == stem)
    }

    /// The stem's GitHub row, if it has one (a stem can in
    /// principle carry rows for more than one site; the
    /// GitHub link is the one update checks consume).
    pub fn github_by_stem(&self, stem: &str) -> Option<&OtherRow> {
        self.rows
            .iter()
            .find(|r| r.stem == stem && r.source_kind == "github")
    }

    /// The one-time migration: absorb config.toml's GitHub
    /// links (stem -> "owner/repo") into the store. A stem
    /// the store already has a row for is left exactly as
    /// it is — the store wins. A migrated row's file name
    /// is the exact name of the loaded library mod with
    /// that stem when one is loaded (`file_names`), and
    /// the stem itself when none is. Returns true when a
    /// row was added, so the caller can persist; running
    /// it again adds nothing.
    pub fn migrate(
        &mut self,
        mod_links: &HashMap<String, String>,
        file_names: &HashMap<String, String>,
    ) -> bool {
        let mut stems: Vec<&String> = mod_links.keys().collect();
        stems.sort();
        let mut added = false;
        for stem in stems {
            if self.by_stem(stem).is_some() {
                continue;
            }
            let repo = &mod_links[stem];
            self.rows.push(OtherRow {
                file_name: file_names
                    .get(stem)
                    .cloned()
                    .unwrap_or_else(|| stem.clone()),
                stem: stem.clone(),
                source_kind: "github".to_string(),
                source_ref: format!("https://github.com/{}", repo),
                grade: "manual".to_string(),
            });
            added = true;
        }
        added
    }

    /// Record (or replace) the stem's GitHub row from a
    /// link a human just set: the row is graded "manual"
    /// and points at the repo's page URL. Returns true
    /// when the store changed.
    pub fn upsert_github(&mut self, stem: &str, file_name: &str, repo: &str) -> bool {
        let source_ref = format!("https://github.com/{}", repo);
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.stem == stem && r.source_kind == "github")
        {
            if row.source_ref == source_ref && row.file_name == file_name {
                return false;
            }
            row.source_ref = source_ref;
            row.file_name = file_name.to_string();
            row.grade = "manual".to_string();
            return true;
        }
        self.rows.push(OtherRow {
            file_name: file_name.to_string(),
            stem: stem.to_string(),
            source_kind: "github".to_string(),
            source_ref,
            grade: "manual".to_string(),
        });
        true
    }

    /// Drop the stem's GitHub row (a human unlinking the
    /// mod). Rows for other sites are untouched. Returns
    /// true when the store changed.
    pub fn remove_github(&mut self, stem: &str) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|r| !(r.stem == stem && r.source_kind == "github"));
        self.rows.len() != before
    }

    /// Persist the store: atomically (write-then-rename,
    /// like the update database). All errors are swallowed
    /// (false) — the store is data the app reads, never a
    /// dependency a run can fail on.
    pub fn save(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let file = StoreFile {
            format: 1,
            generated_at: now_secs(),
            rows: self.rows.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return false;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            return std::fs::rename(&tmp, path).is_ok();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "format": 1,
        "generated_at": 1792542274,
        "rows": [
            {"file_name": "FS25_StickyFeet.zip", "stem": "fs25_stickyfeet",
             "source_kind": "github",
             "source_ref": "https://github.com/Timmeey86/FS25_StickyFeet",
             "grade": "confirmed"},
            {"file_name": "ACH25_DrivewayPack.zip", "stem": "ach25_drivewaypack",
             "source_kind": "itch",
             "source_ref": "https://snakebizzle.itch.io/ach25-driveway-pack",
             "grade": "confirmed"},
            {"file_name": "FS25_SomeMod.zip", "stem": "fs25_somemod",
             "source_kind": "kingmods",
             "source_ref": "https://www.kingmods.net/en/fs25/mods/some-mod",
             "grade": "candidate"}
        ]
    }"#;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mm-other-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_store(dir: &PathBuf) -> (CatalogOther, PathBuf) {
        let path = dir.join("catalog-other.json");
        std::fs::write(&path, FIXTURE).unwrap();
        (CatalogOther::load_from(path.clone()), path)
    }

    #[test]
    fn parses_and_looks_up_by_file_name_and_stem() {
        let dir = scratch("parse");
        let (store, _) = fixture_store(&dir);
        assert_eq!(store.rows().len(), 3);

        // Exact file name: the identity test, case included.
        let row = store.by_file_name("FS25_StickyFeet.zip").unwrap();
        assert_eq!(row.source_kind, "github");
        assert_eq!(
            row.source_ref,
            "https://github.com/Timmeey86/FS25_StickyFeet"
        );
        assert!(store.by_file_name("fs25_stickyfeet.zip").is_none());
        assert!(store.by_file_name("FS25_Missing.zip").is_none());

        // Stem lookups, per kind and in general.
        let itch = store.by_stem("ach25_drivewaypack").unwrap();
        assert_eq!(itch.source_kind, "itch");
        assert_eq!(store.github_by_stem("fs25_stickyfeet").unwrap().grade, "confirmed");
        assert!(store.github_by_stem("ach25_drivewaypack").is_none());
        assert!(store.by_stem("nope").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migration_adds_missing_stems_once() {
        let dir = scratch("migrate");
        let (mut store, path) = fixture_store(&dir);

        let links: HashMap<String, String> = [
            // Already in the store: must NOT be overwritten.
            ("fs25_stickyfeet".to_string(), "someone/else".to_string()),
            // New, with a loaded library mod behind it.
            ("fs25_newmod".to_string(), "owner/repo".to_string()),
            // New, with nothing loaded under that stem.
            ("fs25_ghost".to_string(), "ghost/repo".to_string()),
        ]
        .into_iter()
        .collect();
        let file_names: HashMap<String, String> = [(
            "fs25_newmod".to_string(),
            "FS25_NewMod.zip".to_string(),
        )]
        .into_iter()
        .collect();

        assert!(store.migrate(&links, &file_names));
        assert_eq!(store.rows().len(), 5);
        // The store's own row wins over the config link.
        assert_eq!(
            store.github_by_stem("fs25_stickyfeet").unwrap().source_ref,
            "https://github.com/Timmeey86/FS25_StickyFeet"
        );
        let migrated = store.github_by_stem("fs25_newmod").unwrap();
        assert_eq!(migrated.file_name, "FS25_NewMod.zip");
        assert_eq!(migrated.source_ref, "https://github.com/owner/repo");
        assert_eq!(migrated.grade, "manual");
        // No loaded mod: the stem itself stands in as the name.
        assert_eq!(
            store.github_by_stem("fs25_ghost").unwrap().file_name,
            "fs25_ghost"
        );

        // Idempotent: a second run adds nothing.
        assert!(!store.migrate(&links, &file_names));
        assert_eq!(store.rows().len(), 5);

        // And what migration added survives a save + reload.
        assert!(store.save());
        let reloaded = CatalogOther::load_from(path);
        assert_eq!(reloaded.rows(), store.rows());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn damaged_store_is_quarantined_and_empty() {
        let dir = scratch("damaged");
        let path = dir.join("catalog-other.json");
        std::fs::write(&path, "this is not json").unwrap();

        let store = CatalogOther::load_from(path.clone());
        assert!(store.rows().is_empty());
        assert!(store.by_stem("fs25_stickyfeet").is_none());
        // The damaged file was set aside, not deleted.
        let quarantined: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".damaged-"))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert!(!path.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_store_is_empty_and_upsert_round_trips() {
        let dir = scratch("missing");
        let path = dir.join("catalog-other.json");
        let mut store = CatalogOther::load_from(path.clone());
        assert!(store.rows().is_empty());

        assert!(store.upsert_github("fs25_newmod", "FS25_NewMod.zip", "owner/repo"));
        // Same link again: no change.
        assert!(!store.upsert_github("fs25_newmod", "FS25_NewMod.zip", "owner/repo"));
        assert!(store.save());
        let reloaded = CatalogOther::load_from(path);
        assert_eq!(reloaded.rows().len(), 1);
        assert_eq!(
            reloaded.github_by_stem("fs25_newmod").unwrap().source_ref,
            "https://github.com/owner/repo"
        );
        // Unlink removes only the GitHub row.
        let mut store = reloaded;
        assert!(store.remove_github("fs25_newmod"));
        assert!(!store.remove_github("fs25_newmod"));
        assert!(store.rows().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
