//! Library Layout (Roadmap #2) — Phase 1: the read-only
//! layout engine.
//!
//! Given the library root (config `archive_dir`), the game
//! folder (config `active_dir`), and the scanned mod
//! records, build a [`LayoutPlan`]: where every mod lives
//! now, which library copies must move into their category
//! subfolder, and which game-folder entries must become
//! projections (hardlinks for file mods, junctions for
//! folder mods) of the library copies. The full design is
//! in `library-layout-design.md` (repo root of the
//! workspace clone); the rulings that shape this module:
//!
//! - **A. Category = the category the card displays.** The
//!   Browse card (`mod_card` in `ui/components.rs`) renders
//!   exactly one category pill: `mod.categories.first()`.
//!   `categories::detect_categories` returns the buckets
//!   sorted and deduplicated, so `first()` is the mod's
//!   alphabetically-first bucket. The layout files a mod
//!   under that same first bucket — one home, one copy.
//! - **B. No category → `Other`.** Detection's own
//!   absence marker, `categories::UNCATEGORIZED`
//!   ("Uncategorized"), is how the card renders "this mod
//!   has no category" (no modDesc, or store items nothing
//!   could be mapped from), so it files under `Other` too:
//!   a real displayed bucket is a home; the absence marker
//!   is not. Positive fallback buckets (Scripts, Maps) are
//!   real categories and file under themselves.
//! - **DLC rider (owner ruling, 2026-10-21).** A `.dlc`
//!   pack is a movable FILE mod whose category is
//!   `categories::DLC_CATEGORY` ("DLC's") — assigned by
//!   file kind because the pack is encrypted and has no
//!   modDesc to detect from. Its card shows the same
//!   category (the scan assigns it too), so Ruling A
//!   holds: the displayed category is the home. The
//!   engine applies the override itself, so a pack whose
//!   record predates the ruling — or has no record at
//!   all — still homes under `DLC's`.
//!
//! READ-ONLY IS THE PHASE'S WHOLE POINT: planning reads
//! directory listings, metadata, and link targets, and
//! never creates, modifies, renames, or deletes anything
//! in the library or game folders. The only write in the
//! whole phase is the plan JSON in the app config dir
//! ([`write_plan_json`]), which is Phase 2's input
//! contract.
//!
//! The engine defends itself against record gaps: its
//! own listings locate copies wherever they sit (the
//! library root or one category level down), and a name
//! with no copy in either folder never enters the
//! census. Since Phase 3 the scanner is recursive too,
//! so copies at either depth arrive WITH records and
//! file under their parsed category; a copy with no
//! record at all (its parse failed) files under `Other`
//! wherever it sits — the Phase 2 stopgap that kept a
//! record-less copy in its subfolder is retired, because
//! the records it substituted for now exist.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::categories;
use super::types::{Mod, ModType};

/// The folder a mod with no category files under
/// (Ruling B — the owner's name for it).
pub const OTHER_CATEGORY: &str = "Other";

/// Is this path / file name a `.dlc` pack? The kind
/// test for the DLC rider: the extension, case-folded,
/// and nothing else about the file matters.
pub fn is_dlc(path: &Path) -> bool {
    path.extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("dlc"))
}

/// The layout category for a mod: the bucket its Browse
/// card displays (see the module docs), with the
/// no-category cases folded to [`OTHER_CATEGORY`]. A
/// `.dlc` pack's bucket is its kind-assigned category,
/// whatever its record says.
pub fn layout_category(m: &Mod) -> String {
    if is_dlc(&m.path) {
        return categories::DLC_CATEGORY.to_string();
    }
    match m.categories.first() {
        Some(c) if c != categories::UNCATEGORIZED => c.clone(),
        _ => OTHER_CATEGORY.to_string(),
    }
}

/// Make a category string safe to use as a folder name:
/// path-hostile characters become '-', trailing spaces
/// and dots are trimmed, Windows reserved device names
/// are disarmed, and an empty result falls back to
/// `Other`. Returns the name to use; callers compare
/// against the input to report that a change happened.
pub fn sanitize_category(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    while out.ends_with(' ') || out.ends_with('.') {
        out.pop();
    }
    if out.is_empty() {
        return OTHER_CATEGORY.to_string();
    }
    let stem = out.split('.').next().unwrap_or("").to_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    ) {
        out.push('_');
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModKind {
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    Hardlink,
    Junction,
}

/// One mod in the census: what it is, where its copies
/// sit today, and where the layout wants its library
/// copy to live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CensusEntry {
    /// Identity: the exact file name, characters and
    /// case included — never the modDesc title.
    pub file_name: String,
    pub kind: ModKind,
    /// Size of the library copy when one exists,
    /// otherwise of the game-folder copy / the record.
    pub size: u64,
    /// The layout category, before folder-name
    /// sanitizing (see `target_path` for the folder
    /// actually used).
    pub category: String,
    /// Where the library copy is now, if one exists.
    pub library_path: Option<PathBuf>,
    /// Where the game-folder copy is now, if one exists.
    pub game_path: Option<PathBuf>,
    /// `<library>/<sanitized category>/<file_name>` —
    /// the library copy's home in the laid-out library.
    pub target_path: PathBuf,
    /// Active = a copy sits in the game folder today.
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedMove {
    pub file_name: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedLink {
    pub file_name: String,
    /// The library copy's post-move path (the census
    /// entry's `target_path`) — what the link points at.
    pub target: PathBuf,
    /// `<game folder>/<file_name>` — where the link goes.
    pub link_path: PathBuf,
    pub kind: LinkKind,
    /// True when the game folder already projects the
    /// right target, so Phase 2 has nothing to do. On
    /// Unix this is verified for real (same inode for a
    /// hardlink; a symlink resolving to the target for a
    /// junction). On other platforms Phase 1 cannot prove
    /// it from std metadata and reports false — the real
    /// verification is Phase 2's job there.
    pub satisfied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningKind {
    /// A game-folder copy with no library copy: it
    /// cannot be projected until it has a library home.
    NoLibraryCopy,
    /// Two copies or records claim the same layout
    /// target; the plan suppresses the move and the link
    /// rather than picking a winner silently.
    TargetCollision,
    /// The category needed changing to be a legal
    /// folder name; `detail` names both forms.
    CategorySanitized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanWarning {
    pub kind: WarningKind,
    pub file_name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanSummary {
    /// Mods in the census.
    pub mods: usize,
    /// Library copies that must move into their
    /// category subfolder.
    pub moves: usize,
    /// Hardlinks to create (unsatisfied file-mod links).
    pub hardlinks: usize,
    /// Junctions to create (unsatisfied folder-mod links).
    pub junctions: usize,
    /// Links already projecting the right target.
    pub links_satisfied: usize,
    /// Library copies already sitting at their target.
    pub already_in_place: usize,
    pub warnings: usize,
    /// Total size of the library copies, bytes.
    pub total_bytes: u64,
    /// Mods homed per category folder (folder name ->
    /// count), name-sorted for stable output.
    pub per_category: Vec<(String, usize)>,
}

/// The full plan. Serialized to `layout-plan.json` in
/// the app config dir — Phase 2's input contract — so
/// every path is absolute and every move, link, and
/// warning is spelled out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutPlan {
    pub format: u32,
    pub library_root: PathBuf,
    pub game_root: PathBuf,
    pub entries: Vec<CensusEntry>,
    pub moves: Vec<PlannedMove>,
    pub links: Vec<PlannedLink>,
    pub warnings: Vec<PlanWarning>,
    pub summary: PlanSummary,
}

/// Is `p` shaped like a mod candidate — a folder, a
/// .zip file, or a `.dlc` pack (a movable file mod
/// since the 2026-10-21 DLC rider)? Mirrors the
/// scanner's own candidate filters
/// (`ModManager::enumerate_candidates` for the game
/// folder, `enumerate_library_candidates` for the
/// library, which admits `.dlc` at both levels).
fn is_candidate(p: &Path) -> bool {
    p.is_dir()
        || (p.is_file()
            && p.extension().map_or(false, |e| {
                e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("dlc")
            }))
}

/// The byte size of a copy on disk: the file's length,
/// or the summed file lengths inside a folder mod —
/// the same accounting `Mod::new` uses. Public for the
/// Phase 2 executor's verification pass, which reconciles
/// post-move sizes against pre-move measurements.
pub fn size_on_disk(p: &Path) -> u64 {
    let Ok(md) = std::fs::metadata(p) else {
        return 0;
    };
    if md.is_dir() {
        walkdir::WalkDir::new(p)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    } else {
        md.len()
    }
}

/// List the library's mod-shaped entries by exact file
/// name: the root level, plus one level down inside
/// subfolders that are not themselves folder mods (a
/// folder holding a `modDesc.xml` is a mod, not a
/// category — its contents are not separate mods).
/// Returns the name -> path map and the names found at
/// more than one library location.
fn list_library(root: &Path) -> (HashMap<String, PathBuf>, Vec<String>) {
    let mut map: HashMap<String, PathBuf> = HashMap::new();
    let mut dupes: Vec<String> = Vec::new();
    let record = |map: &mut HashMap<String, PathBuf>,
                      dupes: &mut Vec<String>,
                      path: PathBuf| {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let name = name.to_string();
            if map.insert(name.clone(), path).is_some() && !dupes.contains(&name) {
                dupes.push(name);
            }
        }
    };
    let Ok(top) = std::fs::read_dir(root) else {
        return (map, dupes);
    };
    let mut top_paths: Vec<PathBuf> = top.filter_map(|e| e.ok().map(|e| e.path())).collect();
    top_paths.sort();
    let mut category_dirs: Vec<PathBuf> = Vec::new();
    for path in top_paths {
        if path.is_dir() {
            if path.join("modDesc.xml").exists() {
                // A folder mod sitting at the root.
                record(&mut map, &mut dupes, path);
            } else {
                // A container (a category folder, once
                // laid out): listed through, not listed
                // as a mod itself.
                category_dirs.push(path);
            }
        } else if is_candidate(&path) {
            record(&mut map, &mut dupes, path);
        }
    }
    for dir in category_dirs {
        let Ok(children) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut child_paths: Vec<PathBuf> =
            children.filter_map(|e| e.ok().map(|e| e.path())).collect();
        child_paths.sort();
        for path in child_paths {
            if is_candidate(&path) {
                record(&mut map, &mut dupes, path);
            }
        }
    }
    (map, dupes)
}

/// List the game folder's top-level mod-shaped entries
/// by exact file name.
fn list_game(root: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut paths: Vec<PathBuf> =
            entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if is_candidate(&path) {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    map.entry(name.to_string()).or_insert(path);
                }
            }
        }
    }
    map
}

/// Does the entry at `link_path` already project
/// `target`? Read-only link inspection (see
/// [`PlannedLink::satisfied`] for the platform story).
/// Public for the Phase 2 rollback audit, which must
/// prove the game folder holds only projections before
/// it removes anything.
pub fn link_satisfied(target: &Path, link_path: &Path, kind: LinkKind) -> bool {
    let Ok(link_md) = std::fs::symlink_metadata(link_path) else {
        return false;
    };
    if link_md.file_type().is_symlink() {
        // A symlink (and, on Unix, the shape a junction
        // takes here): satisfied when it resolves to the
        // target.
        let resolved = match std::fs::read_link(link_path) {
            Ok(raw) => {
                if raw.is_absolute() {
                    raw
                } else {
                    link_path.parent().unwrap_or(Path::new("")).join(raw)
                }
            }
            Err(_) => return false,
        };
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        return canon(&resolved) == canon(target);
    }
    if kind == LinkKind::Hardlink {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (std::fs::metadata(target), std::fs::metadata(link_path)) {
                return a.dev() == b.dev() && a.ino() == b.ino();
            }
        }
    }
    false
}

/// Build the layout plan. A pure function of the two
/// roots and the scanned records, apart from read-only
/// filesystem inspection (listings, metadata, link
/// targets). Nothing is written, moved, or deleted.
pub fn build_plan(library_root: &Path, game_root: &Path, mods: &[Mod]) -> LayoutPlan {
    let (library_listing, library_dupes) = list_library(library_root);
    let game_listing = list_game(game_root);

    // Records grouped by exact file name (identity is
    // the file name, never the title).
    let mut records: HashMap<String, Vec<&Mod>> = HashMap::new();
    for m in mods {
        records.entry(m.file_name()).or_default().push(m);
    }

    // The census universe: every name any source knows.
    let mut names: Vec<String> = Vec::new();
    for name in records
        .keys()
        .chain(library_listing.keys())
        .chain(game_listing.keys())
    {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names.sort();

    let mut entries: Vec<CensusEntry> = Vec::new();
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut links: Vec<PlannedLink> = Vec::new();
    let mut warnings: Vec<PlanWarning> = Vec::new();

    for name in &names {
        let recs: Vec<&Mod> = records.get(name).cloned().unwrap_or_default();
        let library_path = library_listing.get(name).cloned();
        let game_path = game_listing.get(name).cloned();

        // A name no folder actually holds a copy of is
        // not a mod the layout can place, move, or
        // project — it never enters the census. In
        // practice this is exactly the depth-1 scanner's
        // phantom reading of a category folder on an
        // already-migrated tree (the folder is a
        // container, and the listing above deliberately
        // does not count containers as copies); the real
        // recursive scan is Phase 3.
        if library_path.is_none() && game_path.is_none() {
            continue;
        }

        // Category from the chosen record: prefer the
        // record sitting at the library copy, then the
        // game copy, then the first by path — the choice
        // only matters for diverged twins, which are
        // collisions anyway.
        let mut sorted_recs = recs.clone();
        sorted_recs.sort_by_key(|r| r.path.clone());
        let chosen = sorted_recs
            .iter()
            .find(|r| Some(&r.path) == library_path.as_ref())
            .or_else(|| sorted_recs.iter().find(|r| Some(&r.path) == game_path.as_ref()))
            .or_else(|| sorted_recs.first())
            .copied();
        // The DLC rider overrides the record entirely:
        // a .dlc pack homes under its kind-assigned
        // category whether the chosen record predates
        // the ruling or no record knows the copy at all.
        let category = if is_dlc(Path::new(name)) {
            categories::DLC_CATEGORY.to_string()
        } else {
            match chosen {
                Some(r) => layout_category(r),
                // No scanned record knows this copy (its
                // parse failed — the recursive scan reaches
                // every depth the layout uses): it files
                // under Other wherever it sits. Phase 2's
                // stopgap, which kept a record-less copy in
                // its subfolder because the depth-1 scan
                // could not see it, is retired — the records
                // it substituted for now exist (Phase 3).
                None => OTHER_CATEGORY.to_string(),
            }
        };
        let folder = sanitize_category(&category);
        if folder != category {
            warnings.push(PlanWarning {
                kind: WarningKind::CategorySanitized,
                file_name: name.clone(),
                detail: format!(
                    "category '{}' is not a legal folder name; filed under '{}'",
                    category, folder
                ),
            });
        }
        let target_path = library_root.join(&folder).join(name);

        let kind = if let Some(p) = library_path.as_ref().or(game_path.as_ref()) {
            if p.is_dir() {
                ModKind::Folder
            } else {
                ModKind::File
            }
        } else {
            match chosen.map(|r| &r.mod_type) {
                Some(ModType::Folder) => ModKind::Folder,
                _ => ModKind::File,
            }
        };
        let size = match chosen {
            Some(r) => r.size,
            None => library_path
                .as_ref()
                .or(game_path.as_ref())
                .map(|p| size_on_disk(p))
                .unwrap_or(0),
        };

        let entry = CensusEntry {
            file_name: name.clone(),
            kind,
            size,
            category: category.clone(),
            library_path: library_path.clone(),
            game_path: game_path.clone(),
            target_path: target_path.clone(),
            active: game_path.is_some(),
        };
        entries.push(entry);

        // Collisions claim the plan's honesty: two
        // diverged records with one name, or two library
        // copies with one name, mean two things want the
        // same target. Warn, and plan neither a move nor
        // a link for the name — never pick a winner
        // silently.
        let record_collision = recs.len() > 1;
        let library_collision = library_dupes.contains(name);
        if record_collision || library_collision {
            let detail = if record_collision {
                let paths: Vec<String> =
                    recs.iter().map(|r| r.path.display().to_string()).collect();
                format!(
                    "{} records share this file name with diverged size or version: {}",
                    recs.len(),
                    paths.join(", ")
                )
            } else {
                format!(
                    "the library holds more than one copy named '{}'",
                    name
                )
            };
            warnings.push(PlanWarning {
                kind: WarningKind::TargetCollision,
                file_name: name.clone(),
                detail,
            });
            continue;
        }

        if let Some(from) = &library_path {
            if *from != target_path {
                moves.push(PlannedMove {
                    file_name: name.clone(),
                    from: from.clone(),
                    to: target_path.clone(),
                });
            }
        }

        if game_path.is_some() {
            if library_path.is_none() {
                warnings.push(PlanWarning {
                    kind: WarningKind::NoLibraryCopy,
                    file_name: name.clone(),
                    detail:
                        "active in the game folder but has no library copy; it cannot be \
                         projected until it has a library home"
                            .to_string(),
                });
            } else {
                let link_kind = match kind {
                    ModKind::File => LinkKind::Hardlink,
                    ModKind::Folder => LinkKind::Junction,
                };
                let link_path = game_root.join(name);
                let satisfied = link_satisfied(&target_path, &link_path, link_kind);
                links.push(PlannedLink {
                    file_name: name.clone(),
                    target: target_path.clone(),
                    link_path,
                    kind: link_kind,
                    satisfied,
                });
            }
        }
    }

    let mut per_category_map: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        *per_category_map
            .entry(sanitize_category(&e.category))
            .or_insert(0) += 1;
    }
    let summary = PlanSummary {
        mods: entries.len(),
        moves: moves.len(),
        hardlinks: links
            .iter()
            .filter(|l| l.kind == LinkKind::Hardlink && !l.satisfied)
            .count(),
        junctions: links
            .iter()
            .filter(|l| l.kind == LinkKind::Junction && !l.satisfied)
            .count(),
        links_satisfied: links.iter().filter(|l| l.satisfied).count(),
        already_in_place: entries
            .iter()
            .filter(|e| e.library_path.as_ref() == Some(&e.target_path))
            .count(),
        warnings: warnings.len(),
        total_bytes: entries
            .iter()
            .filter(|e| e.library_path.is_some())
            .map(|e| e.size)
            .sum(),
        per_category: per_category_map.into_iter().collect(),
    };

    LayoutPlan {
        format: 1,
        library_root: library_root.to_path_buf(),
        game_root: game_root.to_path_buf(),
        entries,
        moves,
        links,
        warnings,
        summary,
    }
}

/// Where the plan JSON lives: the app config dir, next
/// to the library database and the other stores
/// (`~/.config/playground/layout-plan.json`).
pub fn plan_path() -> Option<PathBuf> {
    Some(crate::paths::data_dir()?.join("layout-plan.json"))
}

/// Serialize the plan to [`plan_path`]. The ONLY write
/// the layout phase performs — Phase 2's input contract.
pub fn write_plan_json(plan: &LayoutPlan) -> anyhow::Result<PathBuf> {
    let path = plan_path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(plan)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// The human-readable plan: summary first, then moves,
/// then links, then warnings — the CLI prints this
/// verbatim.
pub fn render_text(plan: &LayoutPlan) -> String {
    let s = &plan.summary;
    let mut out = String::new();
    out.push_str("Library layout plan (READ-ONLY — nothing was moved)\n");
    out.push_str(&format!("Library:     {}\n", plan.library_root.display()));
    out.push_str(&format!("Game folder: {}\n", plan.game_root.display()));
    out.push_str(&format!(
        "Mods: {}  |  Moves: {}  |  Hardlinks: {}  |  Junctions: {}  |  Already linked: {}  |  Already in place: {}  |  Warnings: {}\n",
        s.mods, s.moves, s.hardlinks, s.junctions, s.links_satisfied, s.already_in_place, s.warnings
    ));
    out.push_str(&format!(
        "Library size: {} bytes across {} categor{}:\n",
        s.total_bytes,
        s.per_category.len(),
        if s.per_category.len() == 1 { "y" } else { "ies" }
    ));
    for (name, count) in &s.per_category {
        out.push_str(&format!("  {}: {}\n", name, count));
    }
    out.push_str("\nMoves:\n");
    if plan.moves.is_empty() {
        out.push_str("  (none)\n");
    }
    for m in &plan.moves {
        out.push_str(&format!(
            "  {}: {} -> {}\n",
            m.file_name,
            m.from.display(),
            m.to.display()
        ));
    }
    out.push_str("\nLinks:\n");
    if plan.links.is_empty() {
        out.push_str("  (none)\n");
    }
    for l in &plan.links {
        let kind = match l.kind {
            LinkKind::Hardlink => "hardlink",
            LinkKind::Junction => "junction",
        };
        let state = if l.satisfied { "already linked" } else { "to create" };
        out.push_str(&format!(
            "  [{}] {} -> {} ({})\n",
            kind,
            l.link_path.display(),
            l.target.display(),
            state
        ));
    }
    out.push_str("\nWarnings:\n");
    if plan.warnings.is_empty() {
        out.push_str("  (none)\n");
    }
    for w in &plan.warnings {
        let kind = match w.kind {
            WarningKind::NoLibraryCopy => "no library copy",
            WarningKind::TargetCollision => "target collision",
            WarningKind::CategorySanitized => "category sanitized",
        };
        out.push_str(&format!("  [{}] {}: {}\n", kind, w.file_name, w.detail));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_mod(
        path: &str,
        categories: &[&str],
        size: u64,
        version: &str,
        folder: bool,
    ) -> Mod {
        Mod {
            id: format!("id-{}", path),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            version: version.to_string(),
            author: None,
            description: None,
            mod_type: if folder { ModType::Folder } else { ModType::Zip },
            state: super::super::types::ModState::Archived,
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
            categories: categories.iter().map(|s| s.to_string()).collect(),
            declared_actions: Vec::new(),
            action_binding_refs: Vec::new(),
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "layout-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_file(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    // -- The category rule (Ruling A + B) ----------------

    #[test]
    fn category_is_the_cards_first_bucket() {
        // The stored first element is the category (owner
        // ruling, 2026-10-23: first-listed wins) — the card
        // pill shows categories.first(), and the engine
        // takes that same first element as the home.
        let m = test_mod("/lib/x.zip", &["Tractors", "Vehicles"], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&m), "Tractors");
    }

    #[test]
    fn category_no_category_files_under_other() {
        let uncat = test_mod("/lib/x.zip", &["Uncategorized"], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&uncat), OTHER_CATEGORY);
        let empty = test_mod("/lib/x.zip", &[], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&empty), OTHER_CATEGORY);
    }

    #[test]
    fn category_positive_fallbacks_are_real_homes() {
        let scripts = test_mod("/lib/x.zip", &["Scripts"], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&scripts), "Scripts");
        let maps = test_mod("/lib/x.zip", &["Maps"], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&maps), "Maps");
    }

    #[test]
    fn recordless_copies_file_under_other_wherever_they_sit() {
        // Phase 3 retired the stopgap: the recursive
        // scan gives every reachable copy a record, so
        // a record-less copy (a failed parse) files
        // under Other even when it sits in a category
        // folder — its folder is no longer taken as its
        // category.
        let base = temp_root("recordless");
        let lib = base.join("lib");
        let game = base.join("game");
        write_file(&lib.join("Maps").join("FS25_Map.zip"), 40);
        write_file(&lib.join("FS25_Root.zip"), 10);
        std::fs::create_dir_all(&game).unwrap();
        let plan = build_plan(&lib, &game, &[]);
        let map_entry = plan
            .entries
            .iter()
            .find(|e| e.file_name == "FS25_Map.zip")
            .unwrap();
        assert_eq!(map_entry.category, OTHER_CATEGORY);
        assert_eq!(
            map_entry.target_path,
            lib.join("Other").join("FS25_Map.zip")
        );
        assert!(plan.moves.iter().any(|m| {
            m.file_name == "FS25_Map.zip"
                && m.to == lib.join("Other").join("FS25_Map.zip")
        }));
        assert!(plan.moves.iter().any(|m| {
            m.file_name == "FS25_Root.zip"
                && m.to == lib.join("Other").join("FS25_Root.zip")
        }));
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- The DLC rider (owner ruling, 2026-10-21) -------

    #[test]
    fn dlc_folder_name_is_verbatim() {
        // The trap this pins: the category -> folder-name
        // sanitizer must hand back the owner's exact
        // spelling, apostrophe included — it is a legal
        // folder name on NTFS and ext4, and anything else
        // would file the pack somewhere its card does not
        // point.
        assert_eq!(sanitize_category(categories::DLC_CATEGORY), "DLC's");
    }

    #[test]
    fn dlc_record_category_is_its_kind() {
        // A record that still carries the pre-ruling
        // fallback ("Uncategorized") resolves to the
        // kind-assigned category by its path alone.
        let m = test_mod("/lib/pack.dlc", &["Uncategorized"], 1, "1.0.0.0", false);
        assert_eq!(layout_category(&m), categories::DLC_CATEGORY);
    }

    #[test]
    fn dlc_pack_at_the_root_plans_a_move_to_dlcs() {
        let base = temp_root("dlc-root");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("strawHarvestPack.dlc"), 500);
        // The record predates the ruling: it still says
        // Uncategorized. The engine's override must win.
        let mods = vec![test_mod(
            lib.join("strawHarvestPack.dlc").to_str().unwrap(),
            &["Uncategorized"],
            500,
            "1.0.0.0",
            false,
        )];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.mods, 1);
        let e = &plan.entries[0];
        assert_eq!(e.kind, ModKind::File);
        assert_eq!(e.category, categories::DLC_CATEGORY);
        assert_eq!(
            e.target_path,
            lib.join("DLC's").join("strawHarvestPack.dlc")
        );
        assert_eq!(plan.summary.moves, 1);
        assert_eq!(
            plan.moves[0].to,
            lib.join("DLC's").join("strawHarvestPack.dlc")
        );
        // The per-category census names the literal folder.
        let per: HashMap<String, usize> = plan.summary.per_category.iter().cloned().collect();
        assert_eq!(per.get("DLC's"), Some(&1));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dlc_pack_recordless_still_homes_under_dlcs() {
        // No record at all: the listing alone censuses
        // the pack now, and it still files by kind.
        let base = temp_root("dlc-recordless");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("strawHarvestPack.dlc"), 500);
        let plan = build_plan(&lib, &game, &[]);
        assert_eq!(plan.summary.mods, 1);
        assert_eq!(plan.entries[0].category, categories::DLC_CATEGORY);
        assert_eq!(
            plan.moves[0].to,
            lib.join("DLC's").join("strawHarvestPack.dlc")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dlc_pack_filed_under_dlcs_is_in_place() {
        let base = temp_root("dlc-filed");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("DLC's").join("strawHarvestPack.dlc"), 500);
        let mods = vec![test_mod(
            lib.join("DLC's").join("strawHarvestPack.dlc").to_str().unwrap(),
            &["DLC's"],
            500,
            "1.0.0.0",
            false,
        )];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.moves, 0);
        assert_eq!(plan.summary.already_in_place, 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dlc_pack_in_a_wrong_folder_moves_to_dlcs() {
        let base = temp_root("dlc-wrong");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("Other").join("strawHarvestPack.dlc"), 500);
        let mods = vec![test_mod(
            lib.join("Other").join("strawHarvestPack.dlc").to_str().unwrap(),
            &["DLC's"],
            500,
            "1.0.0.0",
            false,
        )];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.moves, 1);
        assert_eq!(plan.moves[0].from, lib.join("Other").join("strawHarvestPack.dlc"));
        assert_eq!(
            plan.moves[0].to,
            lib.join("DLC's").join("strawHarvestPack.dlc")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dlc_pack_active_plans_a_hardlink_like_any_file_mod() {
        let base = temp_root("dlc-active");
        let lib = base.join("lib");
        let game = base.join("game");
        write_file(&lib.join("strawHarvestPack.dlc"), 500);
        write_file(&game.join("strawHarvestPack.dlc"), 500);
        let mods = vec![test_mod(
            lib.join("strawHarvestPack.dlc").to_str().unwrap(),
            &["DLC's"],
            500,
            "1.0.0.0",
            false,
        )];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.hardlinks, 1);
        let link = &plan.links[0];
        assert_eq!(link.kind, LinkKind::Hardlink);
        // The link points at the post-move library home.
        assert_eq!(
            link.target,
            lib.join("DLC's").join("strawHarvestPack.dlc")
        );
        assert_eq!(link.link_path, game.join("strawHarvestPack.dlc"));
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- The plan over a synthetic flat tree -------------

    #[test]
    fn flat_library_plan() {
        let base = temp_root("flat");
        let lib = base.join("lib");
        let game = base.join("game");
        write_file(&lib.join("FS25_Tractor.zip"), 100);
        write_file(&lib.join("FS25_Pack.zip"), 200);
        write_file(&lib.join("FS25_FolderMod").join("modDesc.xml"), 10);
        write_file(&lib.join("FS25_FolderMod").join("data.bin"), 50);
        write_file(&game.join("FS25_Tractor.zip"), 100);
        std::fs::create_dir_all(game.join("FS25_FolderMod")).unwrap();
        write_file(&game.join("FS25_GameOnly.zip"), 70);

        let mods = vec![
            test_mod(
                lib.join("FS25_Tractor.zip").to_str().unwrap(),
                &["Tractors"],
                100,
                "1.0.0.0",
                false,
            ),
            test_mod(
                lib.join("FS25_Pack.zip").to_str().unwrap(),
                &["Tractors", "Vehicles"],
                200,
                "1.0.0.0",
                false,
            ),
            test_mod(
                lib.join("FS25_FolderMod").to_str().unwrap(),
                &["Scripts"],
                60,
                "1.0.0.0",
                true,
            ),
            test_mod(
                game.join("FS25_GameOnly.zip").to_str().unwrap(),
                &["Cars"],
                70,
                "1.0.0.0",
                false,
            ),
        ];
        let plan = build_plan(&lib, &game, &mods);

        assert_eq!(plan.summary.mods, 4);
        // Moves: Tractor and Pack into Tractors/, the
        // folder mod into Scripts/. GameOnly has no
        // library copy — nothing to move.
        assert_eq!(plan.summary.moves, 3);
        let move_targets: Vec<String> = plan
            .moves
            .iter()
            .map(|m| m.to.display().to_string())
            .collect();
        assert!(move_targets.contains(&lib.join("Tractors").join("FS25_Tractor.zip").display().to_string()));
        assert!(move_targets.contains(&lib.join("Tractors").join("FS25_Pack.zip").display().to_string()));
        assert!(move_targets.contains(&lib.join("Scripts").join("FS25_FolderMod").display().to_string()));
        assert_eq!(plan.summary.already_in_place, 0);
        // Links: the tractor (hardlink) and the folder
        // mod (junction); both are real copies today, so
        // neither is satisfied.
        assert_eq!(plan.summary.hardlinks, 1);
        assert_eq!(plan.summary.junctions, 1);
        assert_eq!(plan.summary.links_satisfied, 0);
        let tractor_link = plan.links.iter().find(|l| l.file_name == "FS25_Tractor.zip").unwrap();
        assert_eq!(tractor_link.kind, LinkKind::Hardlink);
        assert_eq!(tractor_link.target, lib.join("Tractors").join("FS25_Tractor.zip"));
        assert_eq!(tractor_link.link_path, game.join("FS25_Tractor.zip"));
        let folder_link = plan.links.iter().find(|l| l.file_name == "FS25_FolderMod").unwrap();
        assert_eq!(folder_link.kind, LinkKind::Junction);
        // GameOnly: exactly one warning, no library copy.
        assert_eq!(plan.summary.warnings, 1);
        let w = &plan.warnings[0];
        assert_eq!(w.kind, WarningKind::NoLibraryCopy);
        assert_eq!(w.file_name, "FS25_GameOnly.zip");
        // Per-category census: Tractors 2, Scripts 1, Cars 1.
        let per: HashMap<String, usize> = plan.summary.per_category.iter().cloned().collect();
        assert_eq!(per.get("Tractors"), Some(&2));
        assert_eq!(per.get("Scripts"), Some(&1));
        assert_eq!(per.get("Cars"), Some(&1));
        // Library bytes count library copies only.
        assert_eq!(plan.summary.total_bytes, 100 + 200 + 60);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn listing_only_mod_is_censused() {
        // A mod the records never mention (stale scan)
        // still gets a census entry and a move — the
        // folder walk is part of the census.
        let base = temp_root("listing-only");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("FS25_Stray.zip"), 42);
        let plan = build_plan(&lib, &game, &[]);
        assert_eq!(plan.summary.mods, 1);
        let e = &plan.entries[0];
        assert_eq!(e.file_name, "FS25_Stray.zip");
        assert_eq!(e.category, OTHER_CATEGORY);
        assert_eq!(e.size, 42);
        assert_eq!(plan.summary.moves, 1);
        assert_eq!(plan.moves[0].to, lib.join("Other").join("FS25_Stray.zip"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitization_is_applied_and_reported() {
        let base = temp_root("sanitize");
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_file(&lib.join("FS25_Weird.zip"), 10);
        let mods = vec![test_mod(
            lib.join("FS25_Weird.zip").to_str().unwrap(),
            &["Weird: Mods"],
            10,
            "1.0.0.0",
            false,
        )];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].to, lib.join("Weird- Mods").join("FS25_Weird.zip"));
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].kind, WarningKind::CategorySanitized);
        assert!(plan.warnings[0].detail.contains("Weird: Mods"));
        assert!(plan.warnings[0].detail.contains("Weird- Mods"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn diverged_records_collide_and_plan_nothing() {
        let base = temp_root("collision");
        let lib = base.join("lib");
        let game = base.join("game");
        write_file(&lib.join("FS25_Twin.zip"), 100);
        write_file(&game.join("FS25_Twin.zip"), 300);
        let mods = vec![
            test_mod(
                lib.join("FS25_Twin.zip").to_str().unwrap(),
                &["Tractors"],
                100,
                "1.0.0.0",
                false,
            ),
            test_mod(
                game.join("FS25_Twin.zip").to_str().unwrap(),
                &["Tractors"],
                300,
                "2.0.0.0",
                false,
            ),
        ];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.mods, 1);
        assert!(plan.moves.is_empty());
        assert!(plan.links.is_empty());
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].kind, WarningKind::TargetCollision);
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- Idempotency against a migrated tree -------------

    #[cfg(unix)]
    #[test]
    fn migrated_tree_plans_nothing() {
        let base = temp_root("migrated");
        let lib = base.join("lib");
        let game = base.join("game");
        // Library in layout form.
        write_file(&lib.join("Tractors").join("FS25_Tractor.zip"), 100);
        std::fs::create_dir_all(lib.join("Scripts").join("FS25_FolderMod")).unwrap();
        write_file(&lib.join("Scripts").join("FS25_FolderMod").join("modDesc.xml"), 10);
        // Game folder as projections: a real hardlink
        // for the file mod, a symlink standing in for
        // the folder mod's junction.
        std::fs::create_dir_all(&game).unwrap();
        std::fs::hard_link(
            lib.join("Tractors").join("FS25_Tractor.zip"),
            game.join("FS25_Tractor.zip"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            lib.join("Scripts").join("FS25_FolderMod"),
            game.join("FS25_FolderMod"),
        )
        .unwrap();

        let mods = vec![
            test_mod(
                lib.join("Tractors").join("FS25_Tractor.zip").to_str().unwrap(),
                &["Tractors"],
                100,
                "1.0.0.0",
                false,
            ),
            test_mod(
                lib.join("Scripts").join("FS25_FolderMod").to_str().unwrap(),
                &["Scripts"],
                10,
                "1.0.0.0",
                true,
            ),
        ];
        let plan = build_plan(&lib, &game, &mods);
        assert_eq!(plan.summary.moves, 0);
        assert_eq!(plan.summary.hardlinks, 0);
        assert_eq!(plan.summary.junctions, 0);
        assert_eq!(plan.summary.links_satisfied, 2);
        assert_eq!(plan.summary.already_in_place, 2);
        assert_eq!(plan.summary.warnings, 0);
        // The links are still listed — Phase 2's contract
        // carries them as satisfied, not absent.
        assert_eq!(plan.links.len(), 2);
        assert!(plan.links.iter().all(|l| l.satisfied));
        let _ = std::fs::remove_dir_all(&base);
    }
}
