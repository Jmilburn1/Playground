//! The ModHub miss store: catalog-miss.json — the mods a
//! live ModHub search already found NOTHING for (file name
//! + when searched).
//!
//! Design (owner, 2026-10-20): a miss is remembered like a
//! link, so the same fruitless search is never burned
//! twice. Every live-search decision runs the same order:
//!
//! 1. catalog-other first — a mod that has gained a link
//!    there has its miss record REMOVED (the prune), and
//!    a mod present in that store never earns a search;
//! 2. then the ModHub catalog (catalog-hub) — a stem it
//!    knows links instantly, no search;
//! 3. only a mod present in NO store earns a live search,
//!    and its outcome is written down: found -> the link
//!    (saved as links always are); nothing -> a miss
//!    record here.
//!
//! STEVIE EXCLUSION (owner-ruled): Stevie mods are a
//! known manual-only class — he gets them from Stevie's
//! YouTube in batches, and no storefront link exists. The
//! miss logic must never burn a live search on them and
//! never record them as misses. The hunt ledger is the
//! evidence: all 352 no-link names carry "Stevie" (the
//! `...EditedByStevie.zip` family, plus `...ByStevie` /
//! `...byStevie` and `_converted` variants), and no other
//! mod's name does — so the predicate is a
//! case-insensitive "stevie" substring test, enforced in
//! the store itself as well as at the search decision.
//!
//! Like the other stores, this one is never a dependency:
//! a missing file is normal, and a damaged one is
//! quarantined (renamed to catalog-miss.json.damaged-
//! <unix>, never wiped) and treated as an empty store.

use super::catalog_other::CatalogOther;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One remembered miss: this file was live-searched on
/// ModHub at `searched_at` (unix seconds) and ModHub had
/// nothing for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissRow {
    pub file_name: String,
    pub stem: String,
    pub searched_at: u64,
}

/// On-disk envelope — the same shape as the project's
/// other JSON stores (format + generated_at + payload).
#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    format: u32,
    generated_at: u64,
    rows: Vec<MissRow>,
}

pub struct CatalogMiss {
    path: Option<PathBuf>,
    rows: Vec<MissRow>,
}

/// Where the store lives: beside config.toml, the update
/// database, the library database and catalog-other.
pub fn store_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("catalog-miss.json"),
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Is this file one of Stevie's manual-only mods? See the
/// module docs: the name carries "Stevie" (any case) —
/// EditedByStevie, ByStevie, byStevie, _converted forms.
pub fn is_stevie(file_name: &str) -> bool {
    file_name.to_lowercase().contains("stevie")
}

impl CatalogMiss {
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
            // Missing (nothing remembered yet) or unreadable:
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
                        .unwrap_or_else(|| "catalog-miss.json".to_string()),
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

    /// Is this exact file name a remembered miss? The file
    /// name is the identity (characters and case
    /// included), like everywhere else in the project.
    pub fn contains(&self, file_name: &str) -> bool {
        self.rows.iter().any(|r| r.file_name == file_name)
    }

    /// Is this stem a remembered miss? (The stem is the
    /// project's other join key; a record under either key
    /// is the same mod.)
    pub fn contains_stem(&self, stem: &str) -> bool {
        self.rows.iter().any(|r| r.stem == stem)
    }

    /// Remember that a live search found nothing for this
    /// file. Re-recording refreshes `searched_at` instead
    /// of duplicating the row. Stevie files are refused —
    /// they are never recorded, by the owner's ruling, no
    /// matter who asks. Returns true when the store
    /// changed.
    pub fn record_miss(&mut self, file_name: &str, stem: &str) -> bool {
        if is_stevie(file_name) {
            return false;
        }
        if let Some(row) = self.rows.iter_mut().find(|r| r.file_name == file_name) {
            row.searched_at = now_secs();
            row.stem = stem.to_string();
            return true;
        }
        self.rows.push(MissRow {
            file_name: file_name.to_string(),
            stem: stem.to_string(),
            searched_at: now_secs(),
        });
        true
    }

    /// Forget a miss (the mod got linked after all).
    /// Matches on the exact file name or the stem.
    /// Returns true when the store changed.
    pub fn forget(&mut self, file_name: &str, stem: &str) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|r| r.file_name != file_name && r.stem != stem);
        self.rows.len() != before
    }

    /// Step 1 of the run order: reconcile against
    /// catalog-other. A miss whose mod has gained a link
    /// there (matched by exact file name or by stem) is
    /// removed — the mod has a home now, and the miss must
    /// not shadow it. Stevie rows are dropped too: the
    /// store must never hold one (self-healing, in case a
    /// record predates the exclusion). Returns how many
    /// rows were removed.
    pub fn prune_against(&mut self, other: &CatalogOther) -> usize {
        let before = self.rows.len();
        self.rows.retain(|r| {
            !is_stevie(&r.file_name)
                && other.by_file_name(&r.file_name).is_none()
                && other.by_stem(&r.stem).is_none()
        });
        before - self.rows.len()
    }

    /// Persist the store: atomically (write-then-rename,
    /// like the other stores). All errors are swallowed
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

/// The live-search decision for one mod (owner's run
/// order — see the module docs). Callers build jobs only
/// for mods with no ModHub link yet; this decides what
/// happens to each job before any request is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Stevie manual-only class: never searched, never
    /// recorded.
    Stevie,
    /// catalog-other knows this mod's home: no search.
    InOther,
    /// The ModHub catalog knows the stem: link instantly.
    InCatalog,
    /// A live search already found nothing: no new search.
    KnownMiss,
    /// Present in no store: earns the live search.
    Search,
}

pub fn decide(
    file_name: &str,
    stem: &str,
    other: &CatalogOther,
    in_catalog: bool,
    miss: &CatalogMiss,
) -> Decision {
    if is_stevie(file_name) {
        return Decision::Stevie;
    }
    if other.by_file_name(file_name).is_some() || other.by_stem(stem).is_some() {
        return Decision::InOther;
    }
    if in_catalog {
        return Decision::InCatalog;
    }
    if miss.contains(file_name) || miss.contains_stem(stem) {
        return Decision::KnownMiss;
    }
    Decision::Search
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER_FIXTURE: &str = r#"{
        "format": 1,
        "generated_at": 1792542274,
        "rows": [
            {"file_name": "FS25_Gained_Link.zip", "stem": "fs25_gained_link",
             "source_kind": "itch",
             "source_ref": "https://someone.itch.io/gained-link",
             "grade": "confirmed"},
            {"file_name": "FS25_StickyFeet.zip", "stem": "fs25_stickyfeet",
             "source_kind": "github",
             "source_ref": "https://github.com/Timmeey86/FS25_StickyFeet",
             "grade": "confirmed"}
        ]
    }"#;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mm-miss-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn other_store(dir: &PathBuf) -> CatalogOther {
        let path = dir.join("catalog-other.json");
        std::fs::write(&path, OTHER_FIXTURE).unwrap();
        CatalogOther::load_from(path)
    }

    #[test]
    fn stevie_predicate_matches_the_ledger_names() {
        // Real names from the hunt ledger's 352-record
        // no-link (Stevie) set: the EditedByStevie family,
        // a lowercase byStevie, and a _converted variant.
        assert!(is_stevie("FS25_4408CornHeaderEditedByStevie.zip"));
        assert!(is_stevie("F225_JohnDeereS7EditedByStevie.zip"));
        assert!(is_stevie("FS25_Rapide8400WbyStevie.zip"));
        assert!(is_stevie("FS25_horschJoker12RTByStevie_converted.zip"));
        assert!(is_stevie("FS25_Kinze3665BlueDriveEditedByStevie_converted.zip"));
        // Names from outside the set must not match.
        assert!(!is_stevie("FS25_StickyFeet.zip"));
        assert!(!is_stevie("FS25_CaseIH_Magnum_2025.zip"));
        assert!(!is_stevie("ACH25_DrivewayPack.zip"));
    }

    #[test]
    fn miss_records_round_trip_and_refresh() {
        let dir = scratch("roundtrip");
        let path = dir.join("catalog-miss.json");
        let mut store = CatalogMiss::load_from(path.clone());
        assert!(store.rows.is_empty());
        assert!(!store.contains("FS25_Nowhere.zip"));

        assert!(store.record_miss("FS25_Nowhere.zip", "fs25_nowhere"));
        assert!(store.contains("FS25_Nowhere.zip"));
        assert!(store.contains_stem("fs25_nowhere"));
        // Exact file name is the identity: case differs, miss.
        assert!(!store.contains("fs25_nowhere.zip"));

        // Re-recording refreshes in place, never duplicates.
        assert!(store.record_miss("FS25_Nowhere.zip", "fs25_nowhere"));
        assert_eq!(store.rows.len(), 1);
        assert!(store.rows[0].searched_at > 0);

        assert!(store.save());
        let reloaded = CatalogMiss::load_from(path);
        assert_eq!(reloaded.rows, store.rows);

        // Forget works by file name and by stem.
        let mut store = reloaded;
        assert!(store.forget("FS25_Nowhere.zip", "fs25_nowhere"));
        assert!(!store.forget("FS25_Nowhere.zip", "fs25_nowhere"));
        assert!(store.rows.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stevie_files_are_never_recorded() {
        let dir = scratch("stevie");
        let path = dir.join("catalog-miss.json");
        let mut store = CatalogMiss::load_from(path);
        assert!(!store.record_miss(
            "FS25_8RSeriesEditedByStevie.zip",
            "fs25_8rserieseditedbystevie"
        ));
        assert!(store.rows.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_removes_misses_that_gained_a_link() {
        let dir = scratch("prune");
        let other = other_store(&dir);
        let path = dir.join("catalog-miss.json");
        let mut store = CatalogMiss::load_from(path);
        // A miss for a mod that has since gained an
        // itch link (matched by exact file name)...
        store.record_miss("FS25_Gained_Link.zip", "fs25_gained_link");
        // ...a miss matched to catalog-other by stem only...
        store.record_miss("FS25_StickyFeet_Old.zip", "fs25_stickyfeet");
        // ...a genuine miss with no home anywhere...
        store.record_miss("FS25_Nowhere.zip", "fs25_nowhere");
        // ...and a Stevie row that somehow predates the
        // exclusion (written straight to the file, since
        // record_miss refuses to create one).
        let mut seeded = CatalogMiss::load_from(dir.join("seed.json"));
        seeded.rows.push(MissRow {
            file_name: "FS25_8RSeriesEditedByStevie.zip".to_string(),
            stem: "fs25_8rserieseditedbystevie".to_string(),
            searched_at: 1,
        });
        store.rows.extend(seeded.rows);

        let removed = store.prune_against(&other);
        assert_eq!(removed, 3);
        assert_eq!(store.rows.len(), 1);
        assert!(store.contains("FS25_Nowhere.zip"));

        // Idempotent: a second prune removes nothing.
        assert_eq!(store.prune_against(&other), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decision_follows_the_run_order() {
        let dir = scratch("decide");
        let other = other_store(&dir);
        let path = dir.join("catalog-miss.json");
        let mut miss = CatalogMiss::load_from(path);
        miss.record_miss("FS25_Nowhere.zip", "fs25_nowhere");
        // A miss whose stem the catalog has since learned.
        miss.record_miss("FS25_Promoted.zip", "fs25_promoted");

        // Stevie beats everything, even a recorded miss.
        assert_eq!(
            decide(
                "FS25_8RSeriesEditedByStevie.zip",
                "fs25_8rserieseditedbystevie",
                &other,
                true,
                &miss
            ),
            Decision::Stevie
        );
        // In catalog-other: no search (file-name and stem).
        assert_eq!(
            decide("FS25_StickyFeet.zip", "fs25_stickyfeet", &other, false, &miss),
            Decision::InOther
        );
        assert_eq!(
            decide("FS25_Other_Name.zip", "fs25_stickyfeet", &other, false, &miss),
            Decision::InOther
        );
        // In the ModHub catalog: instant link...
        assert_eq!(
            decide("FS25_Fresh.zip", "fs25_fresh", &other, true, &miss),
            Decision::InCatalog
        );
        // ...and the catalog outranks a remembered miss.
        assert_eq!(
            decide("FS25_Promoted.zip", "fs25_promoted", &other, true, &miss),
            Decision::InCatalog
        );
        // A remembered miss: no new search.
        assert_eq!(
            decide("FS25_Nowhere.zip", "fs25_nowhere", &other, false, &miss),
            Decision::KnownMiss
        );
        // Nowhere at all: the only case that earns a search.
        assert_eq!(
            decide("FS25_Unknown.zip", "fs25_unknown", &other, false, &miss),
            Decision::Search
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn damaged_store_is_quarantined_and_empty() {
        let dir = scratch("damaged");
        let path = dir.join("catalog-miss.json");
        std::fs::write(&path, "this is not json").unwrap();

        let store = CatalogMiss::load_from(path.clone());
        assert!(store.rows.is_empty());
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
}
