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
//! ~/.config/fs-mod-manager/updates.json

use crate::mod_manager::{ModManager, ModState};
use crate::sources::{catalog, github, mod_hub};
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
    /// modhub | github | none
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

/// Where the database lives: beside config.toml and the
/// catalog cache.
pub fn db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("updates.json"),
    )
}

fn summary_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("updates-summary.txt"),
    )
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
    let path = db_path()?;
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

pub fn summarize(db: &UpdateDb, installed: &HashMap<String, String>) -> DbSummary {
    let mut s = DbSummary::default();
    for row in &db.rows {
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
             rows: {} (ModHub {}, GitHub {}, no source {})\n\
             updates available: {}\n\
             ahead of source (downgrade-blocked): {}\n\
             rows whose latest version changed since the last fill: {}\n",
            when, db_file, self.total, self.modhub_rows, self.github_rows,
            self.residue, self.updates_available, self.ahead_blocked,
            self.changed_since_last_fill
        )
    }
}

/// The one swipe: local facts, the catalog join, link polls,
/// residue. Writes updates.json (and a human-readable
/// summary beside it) and returns the counts. Intended to
/// run scheduled and unattended; every per-source failure is
/// reported and skipped, never fatal.
pub async fn fill(mm: &ModManager) -> Result<FillSummary> {
    // Previous fill, for the delta: stem -> latest version.
    let prev_latest: HashMap<String, String> = db_path()
        .and_then(|p| UpdateDb::load(&p).ok())
        .map(|db| {
            db.rows
                .into_iter()
                .filter_map(|r| r.latest_version.map(|v| (r.stem, v)))
                .collect()
        })
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
    let cat = catalog::ensure(&mm.catalog_url()).await;
    let index = cat.as_ref().map(catalog::by_stem);

    let (base, token) = mm.github_endpoint();
    let gh = github::GithubSource::new(token);
    let mh = mod_hub::ModHubSource::new();
    let now = now_secs();

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
                        eprintln!("db-fill: {} (ModHub {}): page check failed: {}", stem, id, e);
                    }
                }
            }
        }

        match source_kind {
            "modhub" => summary.modhub_rows += 1,
            "github" => summary.github_rows += 1,
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
            latest_version,
            prev_latest_version: prev,
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
    if let Some(path) = db_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        db.save(&path)?;
        let text = summary.text(&path.to_string_lossy(), now);
        if let Some(sp) = summary_path() {
            let _ = std::fs::write(&sp, &text);
        }
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
        let s = summarize(&db, &installed);
        assert_eq!(s.updates, 1);
        assert_eq!(s.ahead_blocked, 1);
        assert_eq!(s.changed, 1);
        assert_eq!(s.with_source, 3);
    }
}
