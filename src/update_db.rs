//! The personal update database: one row per mod in the
//! user's library, joining installed facts (from the scan)
//! to source facts (the ModHub catalog, the user's links).
//!
//! Design (2026-10-18, after a foreground check over 2,450
//! mods took ~54 minutes to find 149 updates): the questions
//! "what does the internet say today" and "what's new for
//! me" are separated. This file is where the first question's
//! answers live, so the second question — the only one a
//! user ever asks out loud — is a local query that takes
//! milliseconds.
//!
//! Rows are LISTED BY FILE ID (the source's stable id —
//! ModHub's mod id — which never collides and never gets
//! renamed) and CHECKED AGAINST FILE NAME (the one label the
//! library and the sources share). A row earns its file id
//! when it is matched to a source; unmatched rows are the
//! residue — a human queue, never a machine guess.
//!
//! The database is private: it records the user's own links,
//! so unlike the published catalog it never leaves the
//! machine. It lives beside the catalog cache:
//! ~/.config/playground/catalog-home.json — the owner's
//! name for it ("your mods") since the 2026-10-19 naming.
//! It was born as updates.json; the one-time migration in
//! `resolve_db_path` renames it on first run (the old file
//! is copied, never moved, and left in place).

use crate::mod_manager::{ModManager, ModState};
use crate::sources::{catalog, github, itch, kingmods, mod_hub};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRow {
    /// The listing key: the source's stable file id
    /// (ModHub's mod id). None until the row is matched.
    pub file_id: Option<String>,
    /// The check key: the exact file name, held up against
    /// the library's zips and the sources' records alike.
    pub file_name: String,
    /// Lowercase file stem — the join key every existing
    /// structure in this project already uses.
    pub stem: String,
    pub installed_version: String,
    pub installed_size: u64,
    /// Distinct states the file was seen in (Active,
    /// Archived, ...). A hardlinked file can wear both the
    /// library and the game-folder name at once.
    pub locations: Vec<String>,
    /// modhub | github | kingmods | itch | none
    pub source_kind: String,
    /// The locator at the source: the ModHub page URL, or
    /// the GitHub owner/repo. None for the residue.
    pub source_ref: Option<String>,
    pub latest_version: Option<String>,
    /// What latest_version was at the previous fill — the
    /// delta a launch presents is the rows where the two
    /// differ.
    pub prev_latest_version: Option<String>,
    pub download_url: Option<String>,
    /// Unix seconds when this row's source was last asked
    /// (for catalog rows: the record's own verified time).
    /// 0 = never asked.
    pub checked_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDb {
    pub format: u32,
    pub generated_at: u64,
    pub rows: Vec<UpdateRow>,
}

impl UpdateDb {
    pub fn load(path: &std::path::Path) -> Result<UpdateDb> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Could not read update database {:?}", path))?;
        serde_json::from_str(&text).context("The update database is not valid JSON")
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("Update database did not serialize")?;
        // Write-then-rename, same as the catalog: a killed
        // run never leaves a half-written database behind.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .with_context(|| format!("Could not write update database {:?}", tmp))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("Could not replace update database {:?}", path))?;
        Ok(())
    }
}

/// The database's file name (owner's naming, 2026-10-19).
pub const DB_FILE_NAME: &str = "catalog-home.json";

/// The name the database was born with, before the rename.
pub const LEGACY_DB_FILE_NAME: &str = "updates.json";

/// The folder every store in this project shares: beside
/// config.toml and the catalog cache.
pub fn data_dir() -> Option<PathBuf> {
    crate::paths::data_dir()
}

/// The one-time rename (owner, 2026-10-19): when
/// catalog-home.json is absent and updates.json exists, the
/// old file is COPIED to the new name — never moved, so
/// the old file stays in place exactly as it was, and
/// nothing is re-entered. Runs at most once: once the new
/// file exists it is the database, and the legacy file is
/// never consulted (or overwritten) again. Returns true
/// when this call performed the migration.
pub fn migrate_legacy_in(dir: &std::path::Path) -> bool {
    let new = dir.join(DB_FILE_NAME);
    let legacy = dir.join(LEGACY_DB_FILE_NAME);
    if new.exists() || !legacy.exists() {
        return false;
    }
    match std::fs::copy(&legacy, &new) {
        Ok(_) => {
            eprintln!(
                "update database: migrated {} -> {} (the old file was left in place)",
                LEGACY_DB_FILE_NAME, DB_FILE_NAME
            );
            true
        }
        Err(_) => false,
    }
}

/// The database path to read in a folder: migrates the
/// legacy file first (see `migrate_legacy_in`), then names
/// the new file — whether or not it exists yet.
pub fn resolve_db_path_in(dir: &std::path::Path) -> PathBuf {
    migrate_legacy_in(dir);
    dir.join(DB_FILE_NAME)
}

/// `resolve_db_path_in` for the standard data folder.
pub fn resolve_db_path() -> Option<PathBuf> {
    let dir = data_dir()?;
    Some(resolve_db_path_in(&dir))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Status is derived from the two versions, never stored —
/// so the downgrade block is arithmetic, not vigilance.
/// Installed NEWER than the source is `ahead_blocked`:
/// such a row must never be offered as an update.
pub fn status_of(installed: &str, latest: Option<&str>) -> &'static str {
    match latest {
        None => "unknown",
        Some(l) if github::is_newer(l, installed) => "update_available",
        Some(l) if github::is_newer(installed, l) => "ahead_blocked",
        Some(_) => "up_to_date",
    }
}

/// Load the database from its standard home, if a fill has
/// landed there and the file parses. The GUI reader's one
/// call: silent, local, milliseconds. A missing or
/// unreadable database is not an error — it just means no
/// fill has run yet, and launch proceeds without answers.
pub fn load_default() -> Option<UpdateDb> {
    // Resolution runs the one-time legacy migration, so a
    // pre-rename updates.json is still the database the GUI
    // reads on its first launch after the rename ships.
    let path = resolve_db_path()?;
    UpdateDb::load(&path).ok()
}

/// True when this row's latest version moved at the most
/// recent fill — one half of the delta a launch presents
/// (the rows where prev and latest differ).
pub fn is_changed(row: &UpdateRow) -> bool {
    row.latest_version.is_some() && row.latest_version != row.prev_latest_version
}

/// The launch summary: how the database's rows land against
/// the library as it stands NOW. Installed versions are
/// taken live (stem -> version), so a mod updated by hand
/// since the fill is judged by what is actually on disk,
/// not by what the fill recorded. Counts are per stem —
/// a file present in both Active and Archive is one mod
/// wearing two names, and counts once.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DbSummary {
    pub updates: usize,
    pub ahead_blocked: usize,
    pub changed: usize,
    pub with_source: usize,
}

/// Judge the database's rows against the library, with
/// progress: the callback receives (judged, total) as each
/// row is examined — every row in the database counts,
/// matched or not, because examining it IS the work. The
/// launch splash's second phase is driven by this.
pub fn summarize_with_progress(
    db: &UpdateDb,
    installed: &HashMap<String, String>,
    on_progress: &dyn Fn(usize, usize),
) -> DbSummary {
    let total = db.rows.len();
    on_progress(0, total);
    let mut s = DbSummary::default();
    for (i, row) in db.rows.iter().enumerate() {
        // Tick first: an unmatched row was still examined, and
        // a `continue` below must not leave the bar short of
        // its total.
        on_progress(i + 1, total);
        let Some(inst) = installed.get(&row.stem) else {
            continue;
        };
        if row.source_kind != "none" {
            s.with_source += 1;
        }
        match status_of(inst, row.latest_version.as_deref()) {
            "update_available" => s.updates += 1,
            "ahead_blocked" => s.ahead_blocked += 1,
            _ => {}
        }
        if is_changed(row) {
            s.changed += 1;
        }
    }
    s
}

#[derive(Debug, Default)]
pub struct FillSummary {
    pub total: usize,
    pub modhub_rows: usize,
    pub github_rows: usize,
    pub other_rows: usize,
    pub residue: usize,
    pub updates_available: usize,
    pub ahead_blocked: usize,
    pub changed_since_last_fill: usize,
}

impl FillSummary {
    pub fn text(&self, db_file: &str, when: u64) -> String {
        format!(
            "Playground update database fill — unix {}\n\
             file: {}\n\
             rows: {} (ModHub {}, GitHub {}, other sites {}, no source {})\n\
             updates available: {}\n\
             ahead of source (downgrade-blocked): {}\n\
             rows whose latest version changed since the last fill: {}\n",
            when, db_file, self.total, self.modhub_rows, self.github_rows,
            self.other_rows, self.residue, self.updates_available,
            self.ahead_blocked, self.changed_since_last_fill
        )
    }
}

/// Copy a previous fill's facts into a row whose source
/// section has halted (throttled) — the row keeps its
/// previous state rather than being blanked.
fn carry_prev(
    prev: Option<&UpdateRow>,
    latest_version: &mut Option<String>,
    download_url: &mut Option<String>,
    checked_at: &mut u64,
) {
    if let Some(p) = prev {
        *latest_version = p.latest_version.clone();
        *download_url = p.download_url.clone();
        *checked_at = p.checked_at;
    }
}

/// The one swipe: local facts, the catalog join, link polls,
/// residue. Writes catalog-home.json (and a human-readable
/// summary beside it) and returns the counts. Intended to
/// run scheduled and unattended; every per-source failure is
/// reported and skipped, never fatal — with one exception:
/// the GIANTS tripwire (see `fill_at`).
pub async fn fill(mm: &ModManager) -> Result<FillSummary> {
    let mh = mod_hub::ModHubSource::new();
    fill_with(mm, &mh).await
}

/// `fill` with the ModHub leg supplied by the caller — the
/// seam tests use to point the leg at a stub instead of the
/// real GIANTS (production always passes a stock source).
pub async fn fill_with(mm: &ModManager, mh: &mod_hub::ModHubSource) -> Result<FillSummary> {
    let dir = data_dir();
    fill_at(mm, mh, dir.as_deref()).await
}

/// The fill itself, against an explicit data folder (None:
/// compute everything, write nothing — the no-home case).
///
/// The pass is STAGED: rows are computed in memory and the
/// database is written only when the whole pass completes.
/// The GIANTS TRIPWIRE (owner, 2026-10-19): a 403/429 from
/// GIANTS ModHub aborts the pass with an error before any
/// write — a half-finished pass never lands over the good
/// database, which stays on disk byte-identical. A 403/429
/// from KingMods or itch halts only that source's section:
/// its remaining rows keep their previous state (the last
/// fill's facts) instead of being blanked, and the rest of
/// the pass completes and writes normally.
async fn fill_at(
    mm: &ModManager,
    mh: &mod_hub::ModHubSource,
    dir: Option<&std::path::Path>,
) -> Result<FillSummary> {
    // Previous fill: the delta (stem -> latest version)
    // and the full rows, for the delta and for carrying a
    // halted section's rows over with their state intact.
    // Resolution runs the one-time legacy migration first.
    let prev_db = dir
        .map(resolve_db_path_in)
        .and_then(|p| UpdateDb::load(&p).ok());
    let prev_latest: HashMap<String, String> = prev_db
        .as_ref()
        .map(|db| {
            db.rows
                .iter()
                .filter_map(|r| r.latest_version.clone().map(|v| (r.stem.clone(), v)))
                .collect()
        })
        .unwrap_or_default();
    let prev_rows: HashMap<String, UpdateRow> = prev_db
        .map(|db| db.rows.into_iter().map(|r| (r.stem.clone(), r)).collect())
        .unwrap_or_default();

    // Local facts, grouped by stem. The same file can be
    // seen in more than one place (active + archive); the
    // representative copy prefers Active.
    let mut by_stem: BTreeMap<String, Vec<&crate::mod_manager::Mod>> = BTreeMap::new();
    for m in mm.get_mods() {
        by_stem
            .entry(ModManager::mod_stem(&m.path))
            .or_default()
            .push(m);
    }

    let gh_links: HashMap<String, String> = mm.mod_links().into_iter().collect();
    let mh_links: HashMap<String, String> = mm.modhub_links().into_iter().collect();

    // The catalog join: whole index, local lookups only.
    let cat = catalog::ensure(&mm.catalog_url(), mm.cache_ttl_hours()).await;
    let index = cat.as_ref().map(catalog::by_stem);

    let (base, token) = mm.github_endpoint();
    let gh = github::GithubSource::new(token);
    let km = kingmods::KingModsSource::new();
    let it = itch::ItchSource::new();
    let now = now_secs();

    // Per-source throttle halts (the HTTP status that
    // stopped the section). GIANTS has no entry here: its
    // throttle aborts the whole pass on the spot.
    let mut km_halt: Option<u16> = None;
    let mut it_halt: Option<u16> = None;

    let mut summary = FillSummary::default();
    let mut rows: Vec<UpdateRow> = Vec::new();

    for (stem, copies) in &by_stem {
        let rep = copies
            .iter()
            .find(|m| m.state == ModState::Active)
            .copied()
            .or_else(|| copies.first().copied())
            .expect("a stem group always has at least one copy");
        let file_name = rep
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| stem.clone());
        let mut locations: Vec<String> = copies
            .iter()
            .map(|m| format!("{:?}", m.state))
            .collect();
        locations.sort();
        locations.dedup();

        let entry = index.as_ref().and_then(|i| i.get(stem));
        // The file id: an explicit ModHub link wins; the
        // catalog's record for the same file name is next.
        let file_id = mh_links
            .get(stem)
            .cloned()
            .or_else(|| entry.map(|e| e.modhub_id.clone()));

        let mut source_kind = "none";
        let mut source_ref: Option<String> = None;
        let mut latest_version: Option<String> = None;
        let mut download_url: Option<String> = None;
        let mut checked_at: u64 = 0;
        // True when this row's facts were carried over from
        // the previous fill because its source's section
        // halted (throttled) before the row could be asked.
        let mut carried = false;

        if let Some(repo) = gh_links.get(stem) {
            // An explicit link is the well this copy drinks
            // from — it outranks catalog inference.
            source_kind = "github";
            source_ref = Some(repo.clone());
            match gh.latest_release(&base, repo).await {
                Ok(Some(rel)) => {
                    latest_version = Some(rel.version.clone());
                    download_url = github::pick_asset(&rel, stem)
                        .map(|a| a.browser_download_url.clone());
                    checked_at = now;
                }
                Ok(None) => {
                    checked_at = now;
                }
                Err(e) => {
                    eprintln!("db-fill: {} ({}): GitHub check failed: {}", stem, repo, e);
                }
            }
        } else if let Some(id) = &file_id {
            source_kind = "modhub";
            source_ref = Some(mod_hub::page_url(id));
            if let Some(e) = entry {
                latest_version = Some(e.version.clone());
                download_url = Some(e.download_url.clone());
                checked_at = e.checked_at;
            } else {
                // Linked to ModHub but the catalog doesn't
                // carry it yet: ask the page itself, once.
                match mh.fetch_info(id).await {
                    Ok(Some(info)) => {
                        latest_version = Some(info.version);
                        checked_at = now;
                    }
                    Ok(None) => {
                        checked_at = now;
                    }
                    Err(e) => {
                        // THE TRIPWIRE: GIANTS said stop.
                        // Abort the whole pass — the staged
                        // rows are dropped with this return,
                        // and the database on disk is never
                        // touched.
                        if let Some(status) =
                            crate::sources::throttle::throttle_status(&e)
                        {
                            return Err(e.context(format!(
                                "TRIPWIRE: GIANTS ModHub throttled this fill (HTTP {}) — the pass is discarded and the database on disk was left untouched",
                                status
                            )));
                        }
                        eprintln!("db-fill: {} (ModHub {}): page check failed: {}", stem, id, e);
                    }
                }
            }
        }

        if source_kind == "none" {
            // The catalog-other store: the mod's home is a
            // KingMods or itch.io page, and the row's link
            // is polled by that site's leg like any other
            // source. A page whose own file name contradicts
            // this mod's (KingMods captured one, itch's
            // uploads tie to a different mod) is UNVERIFIED:
            // the row keeps the link and the check time, but
            // no version is judged from it — the identity
            // rule is the exact file name, and a page that
            // names another file is not this mod's page
            // anymore. The fill's stderr notes say which
            // rows were left unjudged and why.
            // (GitHub store rows never reach here: they ride
            // in with the links above and are judged by the
            // GitHub leg like any config link.)
            if let Some(other) = mm.other_row(stem) {
                if other.source_kind == "kingmods" {
                    source_kind = "kingmods";
                    source_ref = Some(other.source_ref.clone());
                    if km_halt.is_some() {
                        // Section halted: keep the previous
                        // fill's facts for this row.
                        carry_prev(
                            prev_rows.get(stem),
                            &mut latest_version,
                            &mut download_url,
                            &mut checked_at,
                        );
                        carried = true;
                    } else {
                        match km.fetch_info(&other.source_ref).await {
                            Ok(Some(info)) => {
                                checked_at = now;
                                match &info.file_name {
                                    Some(dl) if *dl != file_name => {
                                        eprintln!(
                                            "db-fill: {} (KingMods): the page's download file is {} — not this mod's file; version left unjudged",
                                            file_name, dl
                                        );
                                    }
                                    _ => {
                                        latest_version = Some(info.version);
                                        download_url = info.download_url;
                                    }
                                }
                            }
                            Ok(None) => {
                                checked_at = now;
                            }
                            Err(e) => {
                                if let Some(status) =
                                    crate::sources::throttle::throttle_status(&e)
                                {
                                    // Section tripwire: KingMods
                                    // halts here; this row and
                                    // every KingMods row after it
                                    // keep their previous state.
                                    km_halt = Some(status);
                                    eprintln!(
                                        "db-fill: KingMods throttled this fill (HTTP {}) — the KingMods section halts here; its remaining rows keep their previous state",
                                        status
                                    );
                                    carry_prev(
                                        prev_rows.get(stem),
                                        &mut latest_version,
                                        &mut download_url,
                                        &mut checked_at,
                                    );
                                    carried = true;
                                } else {
                                    eprintln!(
                                        "db-fill: {} (KingMods): page check failed: {}",
                                        stem, e
                                    );
                                }
                            }
                        }
                    }
                } else if other.source_kind == "itch" {
                    source_kind = "itch";
                    source_ref = Some(other.source_ref.clone());
                    if it_halt.is_some() {
                        carry_prev(
                            prev_rows.get(stem),
                            &mut latest_version,
                            &mut download_url,
                            &mut checked_at,
                        );
                        carried = true;
                    } else {
                        match it.fetch_latest(&other.source_ref, &file_name).await {
                            Ok(Some(info)) => {
                                checked_at = now;
                                if info.mismatch {
                                    eprintln!(
                                        "db-fill: {} (itch): the page's uploads belong to a different mod; version left unjudged",
                                        file_name
                                    );
                                } else {
                                    latest_version = info.version;
                                }
                            }
                            Ok(None) => {
                                checked_at = now;
                            }
                            Err(e) => {
                                if let Some(status) =
                                    crate::sources::throttle::throttle_status(&e)
                                {
                                    it_halt = Some(status);
                                    eprintln!(
                                        "db-fill: itch.io throttled this fill (HTTP {}) — the itch section halts here; its remaining rows keep their previous state",
                                        status
                                    );
                                    carry_prev(
                                        prev_rows.get(stem),
                                        &mut latest_version,
                                        &mut download_url,
                                        &mut checked_at,
                                    );
                                    carried = true;
                                } else {
                                    eprintln!(
                                        "db-fill: {} (itch): page check failed: {}",
                                        stem, e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        match source_kind {
            "modhub" => summary.modhub_rows += 1,
            "github" => summary.github_rows += 1,
            "kingmods" | "itch" => summary.other_rows += 1,
            _ => summary.residue += 1,
        }
        match status_of(&rep.version, latest_version.as_deref()) {
            "update_available" => {
                summary.updates_available += 1;
                println!(
                    "- {} : v{} installed, v{} available ({})",
                    file_name,
                    rep.version,
                    latest_version.as_deref().unwrap_or("?"),
                    source_kind
                );
            }
            "ahead_blocked" => summary.ahead_blocked += 1,
            _ => {}
        }
        let prev = prev_latest.get(stem).cloned();
        if latest_version.is_some() && latest_version != prev {
            summary.changed_since_last_fill += 1;
        }

        rows.push(UpdateRow {
            file_id,
            file_name,
            stem: stem.clone(),
            installed_version: rep.version.clone(),
            installed_size: rep.size,
            locations,
            source_kind: source_kind.to_string(),
            source_ref,
            // A row carried over a halted section reports
            // its own (unchanged) latest as the previous
            // latest, so the halt never masquerades as a
            // version change in the next launch's delta.
            prev_latest_version: if carried {
                latest_version.clone()
            } else {
                prev
            },
            latest_version,
            download_url,
            checked_at,
        });
    }

    summary.total = rows.len();

    // Listed by file id: id-bearing rows first, numeric
    // order; the residue follows, by file name.
    rows.sort_by(|a, b| {
        use std::cmp::Ordering;
        match (&a.file_id, &b.file_id) {
            (Some(x), Some(y)) => {
                let xi = x.parse::<u64>().unwrap_or(u64::MAX);
                let yi = y.parse::<u64>().unwrap_or(u64::MAX);
                xi.cmp(&yi).then_with(|| a.file_name.cmp(&b.file_name))
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.file_name.cmp(&b.file_name),
        }
    });

    let db = UpdateDb {
        format: 1,
        generated_at: now,
        rows,
    };
    // The commit point: everything above was staging. The
    // database is written only here, on a completed pass —
    // a tripwire abort returned long before this line, so
    // the file on disk is still the last good fill.
    if let Some(dir) = dir {
        let _ = std::fs::create_dir_all(dir);
        let path = resolve_db_path_in(dir);
        db.save(&path)?;
        let text = summary.text(&path.to_string_lossy(), now);
        let _ = std::fs::write(dir.join("updates-summary.txt"), &text);
        print!("{}", text);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        stem: &str,
        kind: &str,
        latest: Option<&str>,
        prev: Option<&str>,
    ) -> UpdateRow {
        UpdateRow {
            file_id: None,
            file_name: format!("{}.zip", stem),
            stem: stem.to_string(),
            installed_version: "1.0.0".to_string(),
            installed_size: 0,
            locations: vec!["Active".to_string()],
            source_kind: kind.to_string(),
            source_ref: None,
            latest_version: latest.map(|s| s.to_string()),
            prev_latest_version: prev.map(|s| s.to_string()),
            download_url: None,
            checked_at: 0,
        }
    }

    #[test]
    fn status_derivation_blocks_downgrades() {
        assert_eq!(status_of("1.2.0", Some("1.3.0")), "update_available");
        assert_eq!(status_of("1.3.0", Some("1.2.0")), "ahead_blocked");
        assert_eq!(status_of("1.2.0", Some("1.2.0")), "up_to_date");
        assert_eq!(status_of("1.2.0", None), "unknown");
    }

    #[test]
    fn changed_means_latest_moved_at_the_last_fill() {
        assert!(is_changed(&row("a", "modhub", Some("1.3.0"), Some("1.2.0"))));
        assert!(is_changed(&row("a", "modhub", Some("1.3.0"), None)));
        assert!(!is_changed(&row("a", "modhub", Some("1.3.0"), Some("1.3.0"))));
        assert!(!is_changed(&row("a", "none", None, None)));
    }

    #[test]
    fn summarize_judges_rows_by_live_installed_versions() {
        let db = UpdateDb {
            format: 1,
            generated_at: 0,
            rows: vec![
                row("newer", "modhub", Some("2.0.0"), Some("1.0.0")),
                row("ahead", "github", Some("1.0.0"), Some("1.0.0")),
                row("same", "modhub", Some("1.5.0"), Some("1.5.0")),
                row("residue", "none", None, None),
                row("not_installed", "modhub", Some("9.9.9"), None),
            ],
        };
        let installed: HashMap<String, String> = [
            ("newer".to_string(), "1.0.0".to_string()),
            // The fill recorded 0.9.0 for this one, but the
            // copy on disk is 1.1.0 — ahead of the source,
            // and the summary must say so.
            ("ahead".to_string(), "1.1.0".to_string()),
            ("same".to_string(), "1.5.0".to_string()),
            ("residue".to_string(), "3.0.0".to_string()),
        ]
        .into_iter()
        .collect();
        let s = summarize_with_progress(&db, &installed, &|_, _| {});
        assert_eq!(s.updates, 1);
        assert_eq!(s.ahead_blocked, 1);
        assert_eq!(s.changed, 1);
        assert_eq!(s.with_source, 3);
    }

    #[test]
    fn summarize_progress_ticks_every_row_and_ends_at_total() {
        let db = UpdateDb {
            format: 1,
            generated_at: 0,
            rows: vec![
                row("installed", "modhub", Some("2.0.0"), None),
                // Not installed: no judging outcome, but the
                // row is still examined, so it still ticks —
                // the launch bar must reach its total.
                row("not_installed", "modhub", Some("9.9.9"), None),
                row("residue", "none", None, None),
            ],
        };
        let installed: HashMap<String, String> = [
            ("installed".to_string(), "1.0.0".to_string()),
            ("residue".to_string(), "3.0.0".to_string()),
        ]
        .into_iter()
        .collect();
        let ticks = std::sync::Mutex::new(Vec::new());
        let s = summarize_with_progress(&db, &installed, &|done, total| {
            ticks.lock().unwrap().push((done, total))
        });
        assert_eq!(s.updates, 1);
        let ticks = ticks.lock().unwrap();
        assert_eq!(
            *ticks,
            vec![(0, 3), (1, 3), (2, 3), (3, 3)],
            "one tick per row, monotonic, ending exactly at the total"
        );
    }

    // ---- The rename: updates.json -> catalog-home.json ----

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mm-updatedb-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn legacy_db_migrates_once_and_is_left_in_place() {
        let dir = scratch("migrate");
        let legacy = dir.join(LEGACY_DB_FILE_NAME);
        std::fs::write(&legacy, b"{\"format\":1}").unwrap();
        let resolved = resolve_db_path_in(&dir);
        assert_eq!(resolved, dir.join(DB_FILE_NAME));
        assert_eq!(std::fs::read(&resolved).unwrap(), b"{\"format\":1}");
        assert!(legacy.exists(), "the migration copies; it never moves");
        // Once the new file exists it IS the database: a
        // later change to the legacy file is not re-migrated
        // over it.
        std::fs::write(&legacy, b"changed later").unwrap();
        let _ = resolve_db_path_in(&dir);
        assert_eq!(std::fs::read(&resolved).unwrap(), b"{\"format\":1}");
    }

    #[test]
    fn existing_catalog_home_wins_over_legacy() {
        let dir = scratch("both-exist");
        std::fs::write(dir.join(DB_FILE_NAME), b"new").unwrap();
        std::fs::write(dir.join(LEGACY_DB_FILE_NAME), b"old").unwrap();
        let resolved = resolve_db_path_in(&dir);
        assert_eq!(std::fs::read(&resolved).unwrap(), b"new");
        assert_eq!(
            std::fs::read(dir.join(LEGACY_DB_FILE_NAME)).unwrap(),
            b"old",
            "the legacy file is left exactly as it was"
        );
    }

    #[test]
    fn no_legacy_no_migration() {
        let dir = scratch("neither");
        let resolved = resolve_db_path_in(&dir);
        assert_eq!(resolved, dir.join(DB_FILE_NAME));
        assert!(!resolved.exists(), "resolution alone creates nothing");
    }

    // ---- The GIANTS tripwire, proven by injection ----

    /// Loopback stub answering every request with the given
    /// status and body — the tripwire is proven here, never
    /// by provoking the real GIANTS.
    fn stub_http(status_line: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status_line.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{}", addr)
    }

    fn write_zip_mod(path: &std::path::Path, version: &str) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("modDesc.xml", opts).unwrap();
        zip.write_all(
            format!(
                "<modDesc><author>T</author><title><en>Probe</en></title><version>{}</version></modDesc>",
                version
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }

    /// A manager over a scratch library holding one zip mod
    /// linked to ModHub id 424242, with the catalog disabled
    /// (so the fill must ask the "page" — the stub — itself).
    fn probe_manager(base: &std::path::Path) -> ModManager {
        use crate::config::{Config, GameDirectories, ModSources, Settings};
        let archive = base.join("archive");
        let active = base.join("active");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        write_zip_mod(&archive.join("FS25_Tripwire_Probe.zip"), "1.0.0.0");
        let config = Config {
            game_directories: GameDirectories {
                fs17: None,
                fs19: None,
                fs22: None,
                fs25: None,
            },
            mod_sources: ModSources {
                mod_hub: vec![],
                github: vec![],
                kingmods: vec![],
            },
            settings: Settings {
                archive_directory: archive,
                active_directory: active,
                check_conflicts: true,
                parallel_scans: 2,
                cache_ttl_hours: 24,
                favorites: vec![],
                save_loadouts: HashMap::new(),
                game_directory: None,
                mod_links: HashMap::new(),
                modhub_links: [(
                    "fs25_tripwire_probe".to_string(),
                    "424242".to_string(),
                )]
                .into_iter()
                .collect(),
                catalog_url: String::new(),
            },
        };
        ModManager::new(config).unwrap()
    }

    #[tokio::test]
    async fn fill_halts_on_giants_403_and_leaves_the_database_untouched() {
        let base = scratch("tripwire");
        let data = base.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // The last good fill, byte for byte — a halted pass
        // must leave exactly this behind.
        let sentinel = "{\"format\": 1, \"generated_at\": 7, \"rows\": []}";
        std::fs::write(data.join(DB_FILE_NAME), sentinel).unwrap();

        let mm = probe_manager(&base);
        let mh = mod_hub::ModHubSource::with_base(&stub_http("403 Forbidden", ""));
        let err = fill_at(&mm, &mh, Some(&data)).await.unwrap_err();
        assert_eq!(
            crate::sources::throttle::throttle_status(&err),
            Some(403),
            "the abort must carry the throttle signal: {}",
            err
        );
        assert_eq!(
            std::fs::read(data.join(DB_FILE_NAME)).unwrap(),
            sentinel.as_bytes(),
            "a halted pass never writes over the good database"
        );
        assert!(
            !data.join(LEGACY_DB_FILE_NAME).exists(),
            "no legacy file is conjured by a halted pass"
        );
    }

    #[tokio::test]
    async fn fill_writes_catalog_home_on_a_clean_pass() {
        let base = scratch("clean-pass");
        let data = base.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // Shaped like the real ModHub page (see mod_hub's
        // parser tests): the probe mod, at version 2.0.0.0.
        let page = r#"
            <div class="table-cell"><b>Filename</b></div>
            <div class="table-cell">FS25_Tripwire_Probe.zip</div>
            <div class="table-cell"><b>Version</b></div>
            <div class="table-cell">2.0.0.0</div>
            <a href="https://cdn26.giants-software.com/modHub/storage/00424242/FS25_Tripwire_Probe.zip">DOWNLOAD</a>
        "#;
        let mm = probe_manager(&base);
        let mh = mod_hub::ModHubSource::with_base(&stub_http("200 OK", page));
        let summary = fill_at(&mm, &mh, Some(&data)).await.unwrap();
        assert_eq!(summary.modhub_rows, 1);
        assert_eq!(summary.updates_available, 1);
        let db = UpdateDb::load(&data.join(DB_FILE_NAME)).unwrap();
        assert_eq!(db.rows.len(), 1);
        assert_eq!(db.rows[0].latest_version.as_deref(), Some("2.0.0.0"));
        assert!(
            !data.join(LEGACY_DB_FILE_NAME).exists(),
            "fills land under the new name only"
        );
    }
}

