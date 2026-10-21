//! The local library database: parsed mods, remembered.
//!
//! Design (John, 2026-10-20): "if we db the file names of
//! the mods locally, then it wouldn't have to search all of
//! them every time." The launch question is "are these mods
//! in the db, or no?" — YES: read from the DB and ignore the
//! folder copy. NO: load from the folder and write the
//! results into the DB. The folder is ground truth on a
//! miss; the DB is the authority on a hit.
//!
//! The key is the mod's EXACT file name (characters and
//! case included — the project's identity rule), and a hit
//! also requires the file's fingerprint (size + modified
//! time, exactly as `Mod::new` computes them) to match the
//! stored record: a replaced or edited file changes one of
//! the two and is re-parsed, alone. A stored mod served at
//! a different path than it was recorded at (the hardlink
//! twin in the other folder, or a file moved between
//! folders) has its path, id and created time refreshed to
//! the candidate, so a hit is indistinguishable from a
//! fresh parse.
//!
//! The database is an accelerator only. It lives beside
//! config.toml and the update database:
//! ~/.config/fs-mod-manager/library-db.json. A missing file
//! is normal (first run); an unreadable or damaged one is
//! quarantined (renamed to library-db.json.damaged-<unix>,
//! never wiped) and the library simply loads from the
//! folder, once, and rebuilds it. No scan ever fails
//! because of this file.

use super::types::{id_for_path, Mod};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// On-disk envelope — the same shape as the project's
/// other JSON stores (format + generated_at + payload).
#[derive(Debug, Serialize, Deserialize)]
struct DbFile {
    format: u32,
    generated_at: u64,
    rows: BTreeMap<String, Mod>,
}

pub struct LibraryDb {
    path: Option<PathBuf>,
    rows: Mutex<BTreeMap<String, Mod>>,
    /// The rows exactly as loaded, for the save decision:
    /// the file is rewritten only when the session's rows
    /// actually differ from these. (A hardlink twin's
    /// record points at the archive path mid-scan and back
    /// at the active path by the end — the round trip
    /// changes nothing and must not rewrite the file.)
    loaded: BTreeMap<String, Mod>,
    /// File names presented to `get_or_parse` this session
    /// — the sweep's keep-set (together with records whose
    /// file still exists at its recorded path).
    seen: Mutex<HashSet<String>>,
}

/// Where the database lives: beside config.toml and the
/// update database.
pub fn db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("library-db.json"),
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl LibraryDb {
    /// Load the database from its standard home. Never
    /// fails: a missing, unreadable or damaged file yields
    /// an empty database (and a quarantine rename, when
    /// there is a damaged file to set aside).
    pub fn load_default() -> Self {
        match db_path() {
            Some(path) => Self::load_from(path),
            None => Self::empty(None),
        }
    }

    /// Load from an explicit path (the standard home in
    /// production; a scratch path in tests).
    pub fn load_from(path: PathBuf) -> Self {
        let mut db = Self::empty(Some(path.clone()));
        let Ok(text) = std::fs::read_to_string(&path) else {
            // Missing (first run) or unreadable: an empty
            // database, and nothing to quarantine.
            return db;
        };
        match serde_json::from_str::<DbFile>(&text) {
            Ok(file) if file.format == 1 => {
                db.loaded = file.rows.clone();
                db.rows = Mutex::new(file.rows);
            }
            _ => {
                // Damaged (or a stranger's format): set the
                // file aside under a damaged name — never
                // wiped — and rebuild from the folder.
                let damaged = path.with_file_name(format!(
                    "{}.damaged-{}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "library-db.json".to_string()),
                    now_secs()
                ));
                let _ = std::fs::rename(&path, &damaged);
            }
        }
        db
    }

    fn empty(path: Option<PathBuf>) -> Self {
        Self {
            path,
            rows: Mutex::new(BTreeMap::new()),
            loaded: BTreeMap::new(),
            seen: Mutex::new(HashSet::new()),
        }
    }

    /// The scan's one question, per candidate path: "is
    /// this mod in the db?" A hit returns the stored mod
    /// without opening or parsing the file at all. A miss
    /// runs the full `Mod::new` parse and records the
    /// result. None when the path cannot be parsed as a
    /// mod at all — the same answer `Mod::new` gives.
    pub fn get_or_parse(&self, path: &Path) -> Option<Mod> {
        // The key is the exact file name. A name that isn't
        // plain UTF-8 can't key the database; parse it the
        // old way and leave it unrecorded.
        let Some(key) = path.file_name().and_then(|n| n.to_str()) else {
            return Mod::new(path.to_path_buf()).ok();
        };
        let key = key.to_string();
        if let Ok(mut seen) = self.seen.lock() {
            seen.insert(key.clone());
        }

        // A hit needs the fingerprint to match as well as
        // the name: same name but a changed file is a miss.
        if let Some((size, modified_at)) = fingerprint(path) {
            let mut rows = self.rows.lock().ok()?;
            if let Some(stored) = rows.get_mut(&key) {
                if stored.size == size && stored.modified_at == modified_at {
                    // Serve the record at THIS path: refresh
                    // the location-derived fields so the
                    // result matches a fresh parse of the
                    // candidate exactly (the hardlink twin
                    // in the other folder, a moved file).
                    if stored.path != path {
                        stored.path = path.to_path_buf();
                        stored.id = id_for_path(path);
                        stored.created_at = created_at_of(path, stored.created_at);
                    }
                    return Some(stored.clone());
                }
            }
        }

        let parsed = Mod::new(path.to_path_buf()).ok()?;
        if let Ok(mut rows) = self.rows.lock() {
            rows.insert(key, parsed.clone());
        }
        Some(parsed)
    }

    /// Persist after a scan completes. Entries whose files
    /// vanished are swept (a record survives when this
    /// session saw its name, or its file still exists at
    /// the recorded path — scanning one folder must not
    /// evict the other folder's mods). The write is atomic
    /// (write-then-rename, like the update database), and
    /// skipped entirely when nothing changed, so a warm
    /// scan leaves the file untouched. All errors are
    /// swallowed: the database is an accelerator, never a
    /// dependency.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(mut rows) = self.rows.lock() else {
            return;
        };
        let seen = self.seen.lock().map(|s| s.clone()).unwrap_or_default();
        rows.retain(|key, m| seen.contains(key) || m.path.exists());
        // Rewrite only when the session actually changed
        // the database's content versus what was loaded —
        // a scan of pure hits leaves the file untouched.
        let changed = serde_json::to_string(&*rows).ok()
            != serde_json::to_string(&self.loaded).ok();
        if !changed {
            return;
        }
        let file = DbFile {
            format: 1,
            generated_at: now_secs(),
            rows: rows.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// The validation fingerprint, computed exactly the way
/// `Mod::new` computes a mod's size and modified time —
/// for folder mods the size is the sum of the files inside
/// (a directory's own metadata length is not the mod's
/// size). None when the path can't be stat'ed.
fn fingerprint(path: &Path) -> Option<(u64, DateTime<Utc>)> {
    let metadata = std::fs::metadata(path).ok()?;
    let size = if metadata.is_dir() {
        walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    } else {
        metadata.len()
    };
    let modified_at = metadata
        .modified()
        .map(|t| t.into())
        .unwrap_or_else(|_| Utc::now());
    Some((size, modified_at))
}

fn created_at_of(path: &Path, fallback: DateTime<Utc>) -> DateTime<Utc> {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .map(|t| t.into())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_zip(path: &Path, title: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("modDesc.xml", opts).unwrap();
        zip.write_all(
            format!(
                "<modDesc><author>T</author><title><en>{}</en></title><version>1.0.0.0</version></modDesc>",
                title
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }

    fn zip_mod(dir: &Path, file_name: &str, title: &str) -> PathBuf {
        let path = dir.join(file_name);
        write_zip(&path, title);
        path
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mm-libdb-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Replace a zip with a same-size build carrying a
    /// different title, then restore the original mtime:
    /// the file's fingerprint is unchanged, but a re-parse
    /// would read the NEW title. ("Alpha" and "Omega" are
    /// the same length, so the two builds are byte-size
    /// identical.)
    fn rewrite_same_fingerprint(path: &Path, new_title: &str) {
        let meta = std::fs::metadata(path).unwrap();
        let mtime = meta.modified().unwrap();
        let size = meta.len();
        write_zip(path, new_title);
        assert_eq!(std::fs::metadata(path).unwrap().len(), size);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn hit_serves_stored_record_miss_reparses() {
        let dir = scratch("hitmiss");
        let mods = dir.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let a = zip_mod(&mods, "FS25_Alpha.zip", "Alpha");
        let b = zip_mod(&mods, "FS25_Beta.zip", "Betaa");
        let db_file = dir.join("library-db.json");

        // Cold: both parse and are recorded.
        let db = LibraryDb::load_from(db_file.clone());
        assert_eq!(db.get_or_parse(&a).unwrap().name, "Alpha");
        assert_eq!(db.get_or_parse(&b).unwrap().name, "Betaa");
        db.save();
        assert!(db_file.exists());

        // Swap Alpha's file for a same-fingerprint build
        // titled "Omega". A re-parse would now read "Omega".
        rewrite_same_fingerprint(&a, "Omega");

        // Warm: a fresh DB over the same file must serve
        // the STORED record (Alpha) — proof the archive
        // was never opened.
        let db2 = LibraryDb::load_from(db_file.clone());
        let served = db2.get_or_parse(&a).unwrap();
        assert_eq!(served.name, "Alpha");
        assert_eq!(served.path, a);
        db2.save();

        // Corrupt Alpha's stored fingerprint in the DB
        // file: the next lookup must miss and re-parse,
        // reading the mutated bytes (Omega), and the
        // record must heal on save.
        let mut file: DbFile =
            serde_json::from_str(&std::fs::read_to_string(&db_file).unwrap()).unwrap();
        file.rows.get_mut("FS25_Alpha.zip").unwrap().size += 1;
        std::fs::write(&db_file, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        let db3 = LibraryDb::load_from(db_file.clone());
        let healed = db3.get_or_parse(&a).unwrap();
        assert_eq!(healed.name, "Omega");
        db3.save();
        let file: DbFile =
            serde_json::from_str(&std::fs::read_to_string(&db_file).unwrap()).unwrap();
        assert_eq!(
            file.rows["FS25_Alpha.zip"].size,
            std::fs::metadata(&a).unwrap().len()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn twin_at_another_path_gets_that_paths_identity() {
        let dir = scratch("twin");
        let archive = dir.join("archive");
        let active = dir.join("active");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        let a = zip_mod(&archive, "FS25_Twin.zip", "Twin");
        let copy = active.join("FS25_Twin.zip");
        std::fs::copy(&a, &copy).unwrap();
        // Same inode would be ideal (hardlink), but a copy
        // with the mtime restored exercises the same path:
        // fingerprint equal, recorded path different.
        let mtime = std::fs::metadata(&a).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&copy)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let db = LibraryDb::load_from(dir.join("library-db.json"));
        let first = db.get_or_parse(&a).unwrap();
        let second = db.get_or_parse(&copy).unwrap();
        assert_eq!(second.path, copy);
        assert_eq!(second.id, id_for_path(&copy));
        assert_ne!(first.id, second.id);
        assert_eq!(first.name, second.name);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn damaged_database_is_quarantined_not_fatal() {
        let dir = scratch("damaged");
        let db_file = dir.join("library-db.json");
        std::fs::write(&db_file, "this is not json").unwrap();
        let mods = dir.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let a = zip_mod(&mods, "FS25_Alpha.zip", "Alpha");

        let db = LibraryDb::load_from(db_file.clone());
        // The damaged file was set aside, not deleted...
        let quarantined: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".damaged-"))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert!(!db_file.exists());
        // ...and the library still loads from the folder.
        assert_eq!(db.get_or_parse(&a).unwrap().name, "Alpha");
        db.save();
        assert!(db_file.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vanished_files_are_swept_seen_files_kept() {
        let dir = scratch("sweep");
        let mods = dir.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let a = zip_mod(&mods, "FS25_Alpha.zip", "Alpha");
        let b = zip_mod(&mods, "FS25_Beta.zip", "Betaa");
        let db_file = dir.join("library-db.json");

        let db = LibraryDb::load_from(db_file.clone());
        db.get_or_parse(&a).unwrap();
        db.get_or_parse(&b).unwrap();
        db.save();

        // Beta's file vanishes; a scan that only sees
        // Alpha must sweep Beta's record and keep Alpha's.
        std::fs::remove_file(&b).unwrap();
        let db2 = LibraryDb::load_from(db_file.clone());
        db2.get_or_parse(&a).unwrap();
        db2.save();
        let file: DbFile =
            serde_json::from_str(&std::fs::read_to_string(&db_file).unwrap()).unwrap();
        assert!(file.rows.contains_key("FS25_Alpha.zip"));
        assert!(!file.rows.contains_key("FS25_Beta.zip"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
