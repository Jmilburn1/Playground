use super::keybindings::{ActionOwner, KbOwnerState};
use super::types::{Mod, ModState, ModType};
use crate::config::{Config, ModSources};
use crate::sources::catalog_miss::{CatalogMiss, Decision};
use crate::sources::catalog_other::{CatalogOther, OtherRow};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Move a file or folder, across drives if needed. `fs::rename`
/// refuses to move between volumes (Windows: "cannot move the file
/// to a different disk drive", os error 17) — and the whole point of
/// this app is an archive on one drive and the game folder on
/// another — so fall back to copy-then-delete.
fn move_path(source: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    if std::fs::rename(source, dest).is_ok() {
        return Ok(());
    }
    if source.is_dir() {
        copy_dir_all(source, dest)?;
        std::fs::remove_dir_all(source)?;
    } else {
        std::fs::copy(source, dest)?;
        std::fs::remove_file(source)?;
    }
    Ok(())
}

/// Unpack a zip into a folder, entry by entry (folder-mod
/// update installs). Entry names go through
/// `enclosed_name`, so a broken or hostile archive cannot
/// write outside the folder — entries that try are
/// skipped.
fn extract_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("Could not open the download {:?}", zip_path))?;
    let mut archive =
        zip::ZipArchive::new(file).context("The download is not a readable zip file")?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .context("The download's zip contents are unreadable")?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let target = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&target)
                .with_context(|| format!("Could not write {:?}", target))?;
            std::io::copy(&mut entry, &mut out)
                .context("Could not unpack a file from the download")?;
        }
    }
    Ok(())
}

fn copy_dir_all(source: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(source).unwrap_or(entry.path());
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Everything the Settings page stages, committed in one
/// config write by `ModManager::save_settings_page`
/// (Round 2: the page is tabbed; the save model is still
/// Round 1's — one explicit save for the whole page).
pub struct PageSettings {
    pub active_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub check_conflicts: bool,
    pub parallel_scans: usize,
    pub cache_ttl_hours: u64,
    pub catalog_url: String,
    pub mod_sources: ModSources,
    pub startup_update_check: bool,
    pub self_update_enabled: bool,
    pub browse_default_view: String,
    pub start_maximized: bool,
    pub auto_file_new_mods: bool,
    pub keep_download_history: bool,
    pub fill_log_keep: usize,
    pub log_errors_only_default: bool,
    pub log_lines_cap: usize,
    pub launch: crate::config::LaunchConfig,
}

pub struct ModManager {
    config: Config,
    mods: HashMap<String, Mod>,
    archive_dir: PathBuf,
    active_dir: PathBuf,
    /// The "other sites" link store (catalog-other.json),
    /// loaded at construction and read at launch. The
    /// store wins over config.toml's mod_links; the config
    /// links remain as fallback (see `mod_link`).
    other: CatalogOther,
    /// The config -> store GitHub link migration runs at
    /// most once per manager, after the first scan lands.
    other_migrated: bool,
    /// The ModHub miss store (catalog-miss.json): mods a
    /// live ModHub search already found nothing for. A miss
    /// is remembered like a link, so no search is ever
    /// burned twice (owner, 2026-10-20).
    miss: CatalogMiss,
    /// The Keybindings page's action -> owner join:
    /// action name -> the owning mod's title, the category
    /// its modDesc declared for the action (if any), and
    /// the mod's state (the page's active-only default and
    /// purge read the state). Built from the loaded mods'
    /// declared actions wherever the mod map is installed
    /// — never per render, and with no file IO of its own
    /// (the scan already parsed every modDesc).
    action_owners: HashMap<String, ActionOwner>,
}

impl ModManager {
    pub fn new(config: Config) -> Result<Self> {
        let mut manager = Self::new_unscanned(config)?;
        manager.scan_all()?;
        Ok(manager)
    }

    /// Like `new`, but skips the initial scan. The GUI uses this and
    /// loads the library in a background task, so the window opens
    /// immediately instead of freezing on a large library.
    pub fn new_unscanned(config: Config) -> Result<Self> {
        let archive_dir = config.settings.archive_directory.clone();
        let active_dir = config.settings.active_directory.clone();

        // Ensure directories exist
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create archive directory: {:?}", archive_dir))?;
        std::fs::create_dir_all(&active_dir)
            .with_context(|| format!("Failed to create active directory: {:?}", active_dir))?;

        Ok(Self {
            config,
            mods: HashMap::new(),
            archive_dir,
            active_dir,
            other: CatalogOther::load_default(),
            other_migrated: false,
            miss: CatalogMiss::load_default(),
            action_owners: HashMap::new(),
        })
    }

    pub fn scan_all(&mut self) -> Result<()> {
        let (archive_dir, active_dir, parallel) = self.scan_params();
        self.mods = Self::scan_directories(archive_dir, active_dir, parallel)?;
        self.migrate_other_links();
        self.rebuild_action_owners();
        Ok(())
    }

    /// Rebuild the action -> owner join from the loaded
    /// mods. Mods are visited in title order so the owner
    /// of a multiply-declared action is deterministic;
    /// the first declaration wins.
    fn rebuild_action_owners(&mut self) {
        let mut mods: Vec<&Mod> = self.mods.values().collect();
        mods.sort_by(|a, b| a.name.cmp(&b.name));
        let mut map: HashMap<String, ActionOwner> = HashMap::new();
        for m in mods {
            let state = match m.state {
                ModState::Active => KbOwnerState::Active,
                ModState::Archived => KbOwnerState::Archived,
                ModState::Conflict | ModState::Error => KbOwnerState::Other,
            };
            for declared in &m.declared_actions {
                map.entry(declared.name.clone()).or_insert_with(|| {
                    ActionOwner {
                        title: m.name.clone(),
                        category: declared.category.clone(),
                        state,
                    }
                });
            }
            for referenced in &m.action_binding_refs {
                map.entry(referenced.clone()).or_insert_with(|| {
                    ActionOwner {
                        title: m.name.clone(),
                        category: None,
                        state,
                    }
                });
            }
        }
        self.action_owners = map;
    }

    /// Who owns this input action, per the loaded mods'
    /// modDesc declarations: the mod's title, the action's
    /// declared category, and the mod's state. None = no
    /// loaded mod claims it — the Keybindings page groups
    /// those under Base Game.
    pub fn action_owner(&self, action: &str) -> Option<&ActionOwner> {
        self.action_owners.get(action)
    }

    /// The one-time migration of config.toml's GitHub links
    /// into the catalog-other store. Runs after the first
    /// scan lands (whichever way the mods arrived — `new`,
    /// `scan_all`, or the GUI's background scan installed
    /// with `install_mods`), at most once per manager: the
    /// exact file name for a migrated row comes from the
    /// loaded library when a mod with that stem is loaded.
    /// The store's own rows always win; config links the
    /// store doesn't know are appended, graded "manual",
    /// and the store is persisted.
    fn migrate_other_links(&mut self) {
        if self.other_migrated {
            return;
        }
        self.other_migrated = true;
        if self.config.settings.mod_links.is_empty() {
            return;
        }
        let mut file_names: HashMap<String, String> = HashMap::new();
        for m in self.mods.values() {
            let stem = Self::mod_stem(&m.path);
            if let Some(name) = m.path.file_name() {
                file_names
                    .entry(stem)
                    .or_insert_with(|| name.to_string_lossy().to_string());
            }
        }
        if self
            .other
            .migrate(&self.config.settings.mod_links, &file_names)
        {
            self.other.save();
        }
    }

    /// (archive dir, active dir, parallel scans) — everything a scan
    /// job needs, for running one away from this manager.
    pub fn scan_params(&self) -> (PathBuf, PathBuf, usize) {
        (
            self.archive_dir.clone(),
            self.active_dir.clone(),
            crate::config::clamp_parallel_scans(self.config.settings.parallel_scans),
        )
    }

    /// Whether conflict detection runs in the scan/load
    /// pipeline (Settings page, Library section).
    pub fn check_conflicts(&self) -> bool {
        self.config.settings.check_conflicts
    }

    /// The configured scan parallelism, held to the
    /// Settings page's 1..=16 range.
    pub fn parallel_scans(&self) -> usize {
        crate::config::clamp_parallel_scans(self.config.settings.parallel_scans)
    }

    /// How long a cached catalog or decoded icon may live
    /// before it is stale (Settings page, Library
    /// section), held to at least one hour.
    pub fn cache_ttl_hours(&self) -> u64 {
        crate::config::clamp_cache_ttl_hours(self.config.settings.cache_ttl_hours)
    }

    /// The configured mod sources (Settings page, Catalog
    /// & Sources section): the entries catalog-other
    /// absorbs, with their enabled flags.
    pub fn mod_sources(&self) -> ModSources {
        self.config.mod_sources.clone()
    }

    /// Save the whole Settings page in one write: the two
    /// folders (created if needed, a leading `~` expanded)
    /// plus every value the page stages, across all its
    /// tabs. This is the page's one save model — its Save
    /// button commits all of it through the ordinary config
    /// save, and the caller starts the rescan that applies
    /// the folders and the conflict switch to the library.
    pub fn save_settings_page(&mut self, page: PageSettings) -> Result<()> {
        let active_dir = crate::config::expand_tilde(&page.active_dir);
        let archive_dir = crate::config::expand_tilde(&page.archive_dir);
        std::fs::create_dir_all(&active_dir)
            .with_context(|| format!("Failed to create active directory: {:?}", active_dir))?;
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create archive directory: {:?}", archive_dir))?;
        self.active_dir = active_dir.clone();
        self.archive_dir = archive_dir.clone();
        self.config.settings.active_directory = active_dir;
        self.config.settings.archive_directory = archive_dir;
        self.config.settings.check_conflicts = page.check_conflicts;
        self.config.settings.parallel_scans =
            crate::config::clamp_parallel_scans(page.parallel_scans);
        self.config.settings.cache_ttl_hours =
            crate::config::clamp_cache_ttl_hours(page.cache_ttl_hours);
        self.config.settings.catalog_url = page.catalog_url.trim().to_string();
        self.config.settings.startup_update_check = page.startup_update_check;
        self.config.settings.self_update_enabled = page.self_update_enabled;
        self.config.settings.browse_default_view = page.browse_default_view;
        self.config.settings.start_maximized = page.start_maximized;
        self.config.settings.auto_file_new_mods = page.auto_file_new_mods;
        self.config.settings.keep_download_history = page.keep_download_history;
        self.config.settings.fill_log_keep = page.fill_log_keep.max(1);
        self.config.settings.log_errors_only_default = page.log_errors_only_default;
        self.config.settings.log_lines_cap = page.log_lines_cap.max(1);
        self.config.settings.launch = page.launch;
        self.config.mod_sources = page.mod_sources;
        self.config.save()
    }

    /// Settings Round 2 accessors: the values the tabbed
    /// page stages from, and the features the switches
    /// govern read back.

    /// General > Updates: check for mod updates at start-up.
    pub fn startup_update_check(&self) -> bool {
        self.config.settings.startup_update_check
    }

    /// General > Updates: the app self-update's launch
    /// check is allowed to run.
    pub fn self_update_enabled(&self) -> bool {
        self.config.settings.self_update_enabled
    }

    /// General > Behavior: the stored Browse default-view
    /// key ("all", "active", ..., "category:<name>").
    pub fn browse_default_view(&self) -> String {
        self.config.settings.browse_default_view.clone()
    }

    /// General > Behavior: start the main window maximized.
    pub fn start_maximized(&self) -> bool {
        self.config.settings.start_maximized
    }

    /// Library > Auto-Fill: file completed downloads into
    /// the layout automatically.
    pub fn auto_file_new_mods(&self) -> bool {
        self.config.settings.auto_file_new_mods
    }

    /// Maintenance > Downloads: keep finished download
    /// history across restarts.
    pub fn keep_download_history(&self) -> bool {
        self.config.settings.keep_download_history
    }

    /// Maintenance > Fill log: how many fill logs to keep.
    pub fn fill_log_keep(&self) -> usize {
        self.config.settings.fill_log_keep.max(1)
    }

    /// Diagnostics > Log Reader: open with the
    /// errors/warnings-only filter on.
    pub fn log_errors_only_default(&self) -> bool {
        self.config.settings.log_errors_only_default
    }

    /// Diagnostics > Log Reader: trailing lines kept.
    pub fn log_lines_cap(&self) -> usize {
        self.config.settings.log_lines_cap.max(1)
    }

    /// General > Launch Configuration (the flags half; the
    /// Developer Console half lives in game.xml).
    pub fn launch_config(&self) -> crate::config::LaunchConfig {
        self.config.settings.launch.clone()
    }

    /// Accounts > GitHub: a token resolves from the config's
    /// GitHub source entry.
    pub fn github_connected(&self) -> bool {
        self.github_endpoint().1.is_some()
    }

    /// How many library copies already sit at their layout
    /// target — the organized count the Settings Library
    /// tab's Layout summary shows. Computed the census way
    /// (see layout::build_plan's already_in_place), from
    /// the scanned records alone: a record whose library
    /// path IS its category home counts.
    pub fn organized_count(&self) -> usize {
        self.mods
            .values()
            .filter(|m| {
                self.is_library_path(&m.path)
                    && m.path == self.library_home_for(m)
            })
            .count()
    }

    /// Auto-Fill (Settings > Library): file one completed
    /// mod into its layout home. Narrow and guarded, per
    /// the ruling: the switch must be on, the library must
    /// be in layout form, the mod's record must be its
    /// library copy, and its home must be free — anything
    /// else is a silent no-op (Ok(None)). On success the
    /// copy moves (same library tree; copy-then-delete
    /// across volumes) and a proven game-folder projection
    /// is re-pointed at the new home; the record follows.
    /// Returns a short note for the status line.
    pub fn auto_file_mod(&mut self, mod_id: &str) -> Result<Option<String>> {
        if !self.config.settings.auto_file_new_mods {
            return Ok(None);
        }
        let Some(m) = self.mods.get(mod_id).cloned() else {
            return Ok(None);
        };
        if !self.is_library_path(&m.path) || !self.library_in_layout_form() {
            return Ok(None);
        }
        let home = self.library_home_for(&m);
        if m.path == home || home.exists() {
            return Ok(None);
        }
        let was_projection = self.game_entry_is_projection(&m);
        if let Some(parent) = home.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create category folder: {:?}", parent)
            })?;
        }
        move_path(&m.path, &home)
            .with_context(|| format!("Failed to file {:?} into {:?}", m.path, home))?;
        let mut fresh = m.clone();
        fresh.path = home.clone();
        // A proven projection must not be left reading the
        // old home (for a hardlink the move kept the file,
        // so it may already prove; for a junction it cannot).
        if was_projection {
            self.refresh_projection(&fresh);
        }
        let note = format!(
            "Auto-Fill: {} filed under {}.",
            fresh.file_name(),
            home.parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        self.mods.insert(fresh.id.clone(), fresh);
        Ok(Some(note))
    }

    /// Scan both folders into a fresh mod map without touching any
    /// manager state — the GUI runs this in a background task and
    /// installs the result with `install_mods`.
    pub fn scan_directories(
        archive_dir: PathBuf,
        active_dir: PathBuf,
        parallel: usize,
    ) -> Result<HashMap<String, Mod>> {
        Self::scan_directories_with_progress(archive_dir, active_dir, parallel, &|_, _| {})
    }

    /// `scan_directories` with progress: the callback receives
    /// (processed, total) candidate counts. Both folders are
    /// enumerated BEFORE any parsing starts, so the total is the
    /// real number of mod files on disk from the first tick —
    /// the launch splash's progress bar is driven by this, and
    /// a fake or estimated total would read as broken.
    pub fn scan_directories_with_progress(
        archive_dir: PathBuf,
        active_dir: PathBuf,
        parallel: usize,
        on_progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<HashMap<String, Mod>> {
        // The library database rides along on the scan:
        // remembered mods are served without opening their
        // archives, new or changed ones are parsed and
        // recorded. It is saved once, after both folders.
        let db = super::library_db::LibraryDb::load_default();
        Self::scan_directories_with_db(archive_dir, active_dir, parallel, &db, on_progress)
    }

    fn scan_directories_with_db(
        archive_dir: PathBuf,
        active_dir: PathBuf,
        parallel: usize,
        db: &super::library_db::LibraryDb,
        on_progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<HashMap<String, Mod>> {
        let archive_paths = Self::enumerate_library_candidates(&archive_dir);
        let active_paths = Self::enumerate_candidates(&active_dir);
        let total = archive_paths.len() + active_paths.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        on_progress(0, total);
        let mut mods = Self::scan_paths(
            archive_paths,
            ModState::Archived,
            parallel,
            db,
            &done,
            total,
            on_progress,
        )?;
        let active = Self::scan_paths(
            active_paths,
            ModState::Active,
            parallel,
            db,
            &done,
            total,
            on_progress,
        )?;
        db.save();
        // One library, two doors: the game folder is a
        // projection of the library (hardlinks and
        // junctions since the Library Layout; plain twin
        // copies before it), so the same file in both
        // folders is ONE mod wearing two paths. The twin
        // test is the favorite-key test — EXACT file name
        // (characters and case included), size and
        // version: a match is the same file. The surviving
        // record is the LIBRARY copy — its path, its id,
        // its library-DB fingerprint — and the game-folder
        // entry only marks it Active: in the layout world
        // the game folder holds no data of its own, so a
        // record keyed there would point every operation
        // at a link (Roadmap #2, Phase 3). Same name but
        // different size or version means the copies
        // diverged — a stray real copy, not a projection:
        // both load, and the conflict detector reports
        // them, exactly as before. A game-folder entry
        // with no library twin stays a standalone Active
        // record. Keying on the modDesc TITLE instead
        // would swallow different mods that share a title
        // (John's Bale Storage family, 2026-10-19).
        let mut library_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for m in mods.values() {
            library_by_name
                .entry(m.file_name())
                .or_default()
                .push(m.id.clone());
        }
        for ids in library_by_name.values_mut() {
            ids.sort_by(|a, b| mods[a].path.cmp(&mods[b].path));
        }
        let mut game_records: Vec<Mod> = active.into_values().collect();
        game_records.sort_by(|a, b| a.path.cmp(&b.path));
        let mut consumed: HashSet<String> = HashSet::new();
        for game_rec in game_records {
            let twin = library_by_name
                .get(&game_rec.file_name())
                .and_then(|ids| {
                    ids.iter().find(|id| {
                        !consumed.contains(*id)
                            && mods[*id].size == game_rec.size
                            && mods[*id].version == game_rec.version
                    })
                })
                .cloned();
            match twin {
                Some(id) => {
                    consumed.insert(id.clone());
                    if let Some(lib_rec) = mods.get_mut(&id) {
                        lib_rec.state = ModState::Active;
                    }
                }
                None => {
                    mods.insert(game_rec.id.clone(), game_rec);
                }
            }
        }
        Ok(mods)
    }

    /// Replace the loaded mods with the result of a background scan.
    pub fn install_mods(&mut self, mods: HashMap<String, Mod>) {
        self.mods = mods;
        self.migrate_other_links();
        self.rebuild_action_owners();
    }

    /// List the LIBRARY folder's mod candidates: the root
    /// level plus one category level down (the Library
    /// Layout's shape), without opening any of them. The
    /// listing rule mirrors the layout engine's
    /// (`layout::list_library`), tightened so a flat
    /// library enumerates exactly what the depth-1 listing
    /// always produced:
    ///
    /// - a `.zip` file at the root is a candidate, as are
    ///   a folder mod's own files one level down; a `.dlc`
    ///   pack (GIANTS' official DLC: one encrypted file,
    ///   no readable modDesc) is a file-mod candidate at
    ///   both levels too — `Mod::new` gives it the same
    ///   filename-based fallback record an unparseable
    ///   zip gets, with the one exception that its
    ///   category is the kind-assigned "DLC's" (owner
    ///   ruling, 2026-10-21: the layout homes it under
    ///   that folder), and a library holding one must not
    ///   lose it from the list when it is laid out
    ///   (owner's strawHarvestPack.dlc, 2026-10-21);
    /// - a root folder holding a `modDesc.xml` at its top
    ///   is a folder mod — a candidate itself;
    /// - a root folder without one whose children include
    ///   mod-shaped entries (folders or `.zip` files) is a
    ///   category container: its children are the
    ///   candidates, never the folder itself — a category
    ///   folder must not appear as a phantom "mod";
    /// - a root folder without one and with no mod-shaped
    ///   children (only loose files) keeps the pre-layout
    ///   treatment: the folder itself is the candidate;
    /// - an empty folder holds nothing and is skipped.
    fn enumerate_library_candidates(dir: &PathBuf) -> Vec<PathBuf> {
        if !dir.exists() {
            return Vec::new();
        }
        let mut top: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(_) => return Vec::new(),
        };
        top.sort();
        let is_mod_file = |p: &std::path::Path| {
            p.is_file()
                && p.extension().map_or(false, |e| {
                    e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("dlc")
                })
        };
        let mut out: Vec<PathBuf> = Vec::new();
        for path in top {
            if path.is_file() {
                if is_mod_file(&path) {
                    out.push(path);
                }
            } else if path.is_dir() {
                if path.join("modDesc.xml").exists() {
                    out.push(path);
                    continue;
                }
                let mut children: Vec<PathBuf> = match std::fs::read_dir(&path) {
                    Ok(rd) => rd
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .collect(),
                    Err(_) => Vec::new(),
                };
                children.sort();
                let mod_children: Vec<PathBuf> = children
                    .iter()
                    .filter(|c| c.is_dir() || is_mod_file(c))
                    .cloned()
                    .collect();
                if !mod_children.is_empty() {
                    out.extend(mod_children);
                } else if !children.is_empty() {
                    out.push(path);
                }
            }
        }
        out
    }

    /// List a folder's mod candidates (folders and .zip files,
    /// one level deep) without opening any of them — cheap
    /// enough to run up front so a scan's total is known before
    /// the expensive per-mod work begins.
    fn enumerate_candidates(dir: &PathBuf) -> Vec<PathBuf> {
        if !dir.exists() {
            return Vec::new();
        }

        walkdir::WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            // Depth 0 is the scanned directory itself — without this the
            // archive/active folders themselves show up as "mods".
            .filter(|e| e.depth() > 0)
            .map(|e| e.path().to_path_buf())
            // A mod is a folder or a .zip file; stray files are ignored.
            .filter(|p| {
                p.is_dir()
                    || (p.is_file()
                        && p.extension()
                            .map_or(false, |e| e.eq_ignore_ascii_case("zip")))
            })
            .collect()
    }

    /// Parse (or serve from the library database) an enumerated
    /// candidate list, ticking `on_progress` after every file —
    /// hit or parse, success or failure, the file was processed.
    fn scan_paths(
        paths: Vec<PathBuf>,
        state: ModState,
        parallel: usize,
        db: &super::library_db::LibraryDb,
        done: &std::sync::atomic::AtomicUsize,
        total: usize,
        on_progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<HashMap<String, Mod>> {
        let mods: Vec<Mod> = paths
            .par_iter()
            .with_min_len(parallel)
            .filter_map(|p| {
                let parsed = db.get_or_parse(p);
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                on_progress(n, total);
                parsed
            })
            .collect();

        let mut result = HashMap::new();
        for mut mod_data in mods {
            mod_data.state = state.clone();
            result.insert(mod_data.id.clone(), mod_data);
        }

        Ok(result)
    }

    // -----------------------------------------------------------------
    // Library Layout steady state (Roadmap #2, Phase 3).
    //
    // There is no mode flag: every decision below is derived
    // from the tree in front of the manager, per mod. A mod
    // whose library copy sits in a category subfolder of the
    // library, with the library and game folders on one
    // volume, lives in the layout world — activation projects
    // it, deactivation unprojects it, and the library copy
    // never moves. Anything else (a flat library, a
    // cross-volume pair per Ruling F, a game-folder-only mod)
    // takes the pre-layout move code paths unchanged.
    // -----------------------------------------------------------------

    /// Is this path one of the library's own copies (as
    /// opposed to a game-folder entry)?
    fn is_library_path(&self, path: &std::path::Path) -> bool {
        path.starts_with(&self.archive_dir)
    }

    /// Does this library copy sit in a category subfolder —
    /// `<library>/<Category>/<file>` — rather than at the
    /// library root?
    fn in_category_subfolder(&self, library_path: &std::path::Path) -> bool {
        library_path.parent().map_or(false, |parent| {
            parent != self.archive_dir.as_path()
                && parent.parent() == Some(self.archive_dir.as_path())
        })
    }

    /// The layout is in effect for this library copy: filed
    /// in a category subfolder, and the two folders share a
    /// volume so projections can exist (Ruling F — when the
    /// volume cannot even be proven shared, the safe answer
    /// is the copy/move behavior, never a failed link).
    fn layout_in_effect_for(&self, library_path: &std::path::Path) -> bool {
        self.is_library_path(library_path)
            && self.in_category_subfolder(library_path)
            && super::layout_exec::same_volume(&self.archive_dir, &self.active_dir)
                == Some(true)
    }

    /// Is the library as a whole in layout form — any known
    /// library copy filed in a category subfolder? New files
    /// entering the library (update installs of game-only
    /// mods) land sorted only when it is.
    fn library_in_layout_form(&self) -> bool {
        self.mods
            .values()
            .any(|m| self.is_library_path(&m.path) && self.in_category_subfolder(&m.path))
    }

    /// The projection kind a mod takes in the game folder:
    /// folder mods project as junctions, file mods as
    /// hardlinks (Ruling D).
    fn projection_kind(m: &Mod) -> super::layout::LinkKind {
        match m.mod_type {
            ModType::Folder => super::layout::LinkKind::Junction,
            _ => super::layout::LinkKind::Hardlink,
        }
    }

    /// Where a mod's file belongs in a laid-out library:
    /// `<library>/<card category>/<file name>`, the
    /// category sanitized for the filesystem, `Other` when
    /// the mod has none (Rulings A and B, via the layout
    /// engine's own rule).
    pub fn library_home_for(&self, m: &Mod) -> PathBuf {
        self.archive_dir
            .join(super::layout::sanitize_category(
                &super::layout::layout_category(m),
            ))
            .join(m.file_name())
    }

    /// Does the game folder currently hold a PROVEN
    /// projection of this library-resident mod — proven by
    /// the executor's own satisfaction check, never by the
    /// mere presence of a same-named entry?
    fn game_entry_is_projection(&self, m: &Mod) -> bool {
        if !self.is_library_path(&m.path) {
            return false;
        }
        let entry = self.active_dir.join(m.file_name());
        std::fs::symlink_metadata(&entry).is_ok()
            && super::layout_exec::projection_proven(
                &m.path,
                &entry,
                Self::projection_kind(m),
            )
    }

    pub fn move_to_archive(&mut self, mod_id: &str) -> Result<()> {
        let snapshot = self
            .mods
            .get(mod_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;

        if self.is_library_path(&snapshot.path) {
            // The record is keyed to the library copy (the
            // scan keeps it there); deactivation is about
            // the game-folder entry, never about moving the
            // library copy.
            let entry = self.active_dir.join(snapshot.file_name());
            if std::fs::symlink_metadata(&entry).is_ok() {
                if self.game_entry_is_projection(&snapshot) {
                    // A proven projection is the library
                    // file wearing a second name — removing
                    // it loses nothing, in any layout state.
                    super::layout_exec::remove_projected_entry(&entry)?;
                } else {
                    // A real file, not a projection of this
                    // copy: the pre-layout move semantics —
                    // the game copy moves onto the library
                    // home (for a flat library that IS the
                    // archive-root destination it always
                    // was), replacing it.
                    move_path(&entry, &snapshot.path)?;
                }
            }
            let mod_data = self.mods.get_mut(mod_id).unwrap();
            mod_data.state = ModState::Archived;
            return Ok(());
        }

        let mod_data = self.mods.get_mut(mod_id).unwrap();

        let source_path = mod_data.path.clone();
        let dest_path = self.archive_dir.join(source_path.file_name().unwrap());

        move_path(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Archived;

        Ok(())
    }

    pub fn move_to_active(&mut self, mod_id: &str) -> Result<()> {
        let snapshot = self
            .mods
            .get(mod_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;

        if self.is_library_path(&snapshot.path)
            && self.layout_in_effect_for(&snapshot.path)
        {
            // Layout world: activation is creating the
            // projection. The library copy does not move;
            // only the record's state changes.
            let dest = self.active_dir.join(snapshot.file_name());
            if std::fs::symlink_metadata(&dest).is_ok() {
                if self.game_entry_is_projection(&snapshot) {
                    // Already projected — the state was
                    // simply stale.
                    self.mods.get_mut(mod_id).unwrap().state = ModState::Active;
                    return Ok(());
                }
                anyhow::bail!(
                    "Cannot activate {}: a different file already sits in the game folder under that name.",
                    snapshot.file_name()
                );
            }
            super::layout_exec::create_projection(
                &snapshot.path,
                &dest,
                Self::projection_kind(&snapshot),
            )?;
            self.mods.get_mut(mod_id).unwrap().state = ModState::Active;
            return Ok(());
        }

        let mod_data = self.mods.get_mut(mod_id).unwrap();

        let source_path = mod_data.path.clone();
        let dest_path = self.active_dir.join(source_path.file_name().unwrap());

        move_path(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Active;

        Ok(())
    }

    pub fn move_between_directories(&mut self, mod_id: &str, target_dir: &PathBuf) -> Result<()> {
        // The CLI's `move` verb names the two real folders:
        // a move "to" the game folder IS activation and a
        // move "to" the library IS deactivation, so both
        // route through those ops (link operations in the
        // layout world, moves in the flat one). Any other
        // target keeps the raw move below.
        if target_dir == &self.active_dir {
            return self.move_to_active(mod_id);
        }
        if target_dir == &self.archive_dir {
            return self.move_to_archive(mod_id);
        }
        let mod_data = self.mods.get_mut(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;

        let source_path = mod_data.path.clone();
        let dest_path = target_dir.join(source_path.file_name().unwrap());

        move_path(&source_path, &dest_path)?;

        mod_data.path = dest_path;

        // Update state based on target directory
        if target_dir == &self.active_dir {
            mod_data.state = ModState::Active;
        } else if target_dir == &self.archive_dir {
            mod_data.state = ModState::Archived;
        }

        Ok(())
    }

    /// Point the manager at new active/archive folders (Settings tab).
    /// Expands a leading `~`, creates the folders if needed, and
    /// persists them to the config file. Does NOT rescan — the GUI
    /// starts a background scan when this returns; other callers
    /// should call `scan_all` themselves if they need fresh mods.
    pub fn set_directories(&mut self, active_dir: PathBuf, archive_dir: PathBuf) -> Result<()> {
        let active_dir = crate::config::expand_tilde(&active_dir);
        let archive_dir = crate::config::expand_tilde(&archive_dir);
        std::fs::create_dir_all(&active_dir)
            .with_context(|| format!("Failed to create active directory: {:?}", active_dir))?;
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create archive directory: {:?}", archive_dir))?;
        self.active_dir = active_dir.clone();
        self.archive_dir = archive_dir.clone();
        self.config.settings.active_directory = active_dir;
        self.config.settings.archive_directory = archive_dir;
        self.config.save()
    }

    /// The remembered profile folder (Settings tab), where
    /// game.xml lives. None until the user picks it.
    pub fn game_directory(&self) -> Option<PathBuf> {
        self.config.settings.game_directory.clone()
    }

    /// Remember the profile folder; persists to the config.
    pub fn set_game_directory(&mut self, dir: PathBuf) -> Result<()> {
        let dir = crate::config::expand_tilde(&dir);
        self.config.settings.game_directory = Some(dir);
        self.config.save()
    }

    /// The remembered true install folder (Game Screens):
    /// the folder holding the game's `shared` folder. None
    /// until the user picks it — it is never guessed from
    /// the profile folder above.
    pub fn game_install_dir(&self) -> Option<PathBuf> {
        self.config.settings.game_install_dir.clone()
    }

    /// Remember the true install folder; persists to the
    /// config. Callers validate first (see
    /// gamescreens::validate_install_pick).
    pub fn set_game_install_dir(&mut self, dir: PathBuf) -> Result<()> {
        let dir = crate::config::expand_tilde(&dir);
        self.config.settings.game_install_dir = Some(dir);
        self.config.save()
    }

    pub fn add_directory(&mut self, path: PathBuf) -> Result<()> {
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
        }

        self.scan_all()?;
        Ok(())
    }

    pub fn remove_mod(&mut self, mod_id: &str) -> Result<()> {
        let mod_data = self.mods.remove(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;

        // A library-resident mod's game-folder projection
        // is the same file wearing a second name, not a
        // copy: deleting only the library path would leave
        // the mod fully alive in the game. When the entry
        // is a PROVEN projection of this copy (checked
        // before the copy goes — the proof reads it), it is
        // removed too. A real file in the game folder is
        // not the record's copy and is left alone, as
        // before.
        let projection_entry = if self.game_entry_is_projection(&mod_data) {
            Some(self.active_dir.join(mod_data.file_name()))
        } else {
            None
        };

        if mod_data.path.exists() {
            if mod_data.path.is_dir() {
                std::fs::remove_dir_all(&mod_data.path)?;
            } else {
                std::fs::remove_file(&mod_data.path)?;
            }
        }
        if let Some(entry) = projection_entry {
            let _ = super::layout_exec::remove_projected_entry(&entry);
        }

        Ok(())
    }

    pub fn get_mods(&self) -> Vec<&Mod> {
        self.mods.values().collect()
    }

    pub fn get_mod(&self, id: &str) -> Option<&Mod> {
        self.mods.get(id)
    }

    pub fn get_mods_by_state(&self, state: ModState) -> Vec<&Mod> {
        self.mods.values()
            .filter(|m| m.state == state)
            .collect()
    }

    pub fn search_mods(&self, query: &str) -> Vec<&Mod> {
        let query_lower = query.to_lowercase();
        self.mods.values()
            .filter(|m| {
                m.name.to_lowercase().contains(&query_lower) ||
                m.author.as_ref().map_or(false, |a| a.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    pub fn get_archive_dir(&self) -> &PathBuf {
        &self.archive_dir
    }

    /// The Library Layout plan (Roadmap #2, Phase 1):
    /// a read-only census of both folders against the
    /// scanned records — moves into category subfolders
    /// and game-folder projections a migration would
    /// make. Planning changes nothing on disk.
    pub fn layout_plan(&self) -> super::layout::LayoutPlan {
        let mods: Vec<Mod> = self.mods.values().cloned().collect();
        super::layout::build_plan(&self.archive_dir, &self.active_dir, &mods)
    }

    pub fn get_active_dir(&self) -> &PathBuf {
        &self.active_dir
    }

    pub fn is_favorite(&self, favorite_key: &str) -> bool {
        self.config
            .settings
            .favorites
            .iter()
            .any(|f| f == favorite_key)
    }

    /// Toggle a favorite by its key; persists to the config file.
    /// Returns the new favorite state.
    pub fn toggle_favorite(&mut self, favorite_key: &str) -> Result<bool> {
        let favs = &mut self.config.settings.favorites;
        let now_favorite = if let Some(pos) = favs.iter().position(|f| f == favorite_key) {
            favs.remove(pos);
            false
        } else {
            favs.push(favorite_key.to_string());
            true
        };
        self.config.save()?;
        Ok(now_favorite)
    }

    pub fn favorites_count(&self) -> usize {
        self.config.settings.favorites.len()
    }

    /// The file stem that keys a mod's update link and audit
    /// identity: the file/folder name, lowercase, ".zip" removed.
    pub fn mod_stem(path: &std::path::Path) -> String {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        name.strip_suffix(".zip")
            .map(|s| s.to_string())
            .unwrap_or(name)
    }

    /// The catalog-other store's row for this stem, if the
    /// store knows the mod's home (GitHub, KingMods or
    /// itch.io). The store is read at launch; rows surface
    /// in the details panel and feed db-fill.
    pub fn other_row(&self, stem: &str) -> Option<&OtherRow> {
        self.other.by_stem(stem)
    }

    /// The catalog-other store's row for this exact file
    /// name (characters and case included), if any.
    pub fn other_row_for_file(&self, file_name: &str) -> Option<&OtherRow> {
        self.other.by_file_name(file_name)
    }

    /// Step 1 of the detection run order (owner,
    /// 2026-10-20): reconcile the miss store against
    /// catalog-other — a mod that gained a link there
    /// loses its miss record. Both detect paths call this
    /// when they open, whether or not a search follows.
    /// Returns how many miss records were forgotten.
    pub fn prune_misses(&mut self) -> usize {
        let removed = self.miss.prune_against(&self.other);
        if removed > 0 {
            self.miss.save();
        }
        removed
    }

    /// The live-search decision for one unlinked mod, by
    /// the run order: Stevie (never searched) ->
    /// catalog-other -> the ModHub catalog -> the miss
    /// store -> only then a live search.
    pub fn detect_decision(&self, file_name: &str, stem: &str, in_catalog: bool) -> Decision {
        crate::sources::catalog_miss::decide(file_name, stem, &self.other, in_catalog, &self.miss)
    }

    /// Remember that a live ModHub search found nothing
    /// for this file, so it is never searched again.
    /// Persists immediately, like links do. Stevie files
    /// are refused inside the store.
    pub fn record_miss(&mut self, file_name: &str, stem: &str) {
        if self.miss.record_miss(file_name, stem) {
            self.miss.save();
        }
    }

    /// Forget a remembered miss (the mod got linked after
    /// all). Persists when anything changed.
    pub fn forget_miss(&mut self, file_name: &str, stem: &str) {
        if self.miss.forget(file_name, stem) {
            self.miss.save();
        }
    }

    /// The GitHub repo ("owner/repo") this mod is linked to for
    /// update checks, if any. The catalog-other store wins;
    /// config.toml's mod_links are the fallback while the
    /// transition settles, so nothing already linked breaks.
    pub fn mod_link(&self, stem: &str) -> Option<String> {
        if let Some(row) = self.other.github_by_stem(stem) {
            if let Some(repo) = crate::sources::github::normalize_repo(&row.source_ref) {
                return Some(repo);
            }
        }
        self.config.settings.mod_links.get(stem).cloned()
    }

    /// Link (or, with None, unlink) a mod stem to a GitHub repo.
    /// The link now lives in the catalog-other store (that is
    /// where GitHub links live from here on); the config entry
    /// is still written, as the fallback copy older paths read.
    pub fn set_mod_link(&mut self, stem: &str, repo: Option<String>) -> Result<()> {
        match &repo {
            Some(r) => {
                self.config
                    .settings
                    .mod_links
                    .insert(stem.to_string(), r.clone());
                let file_name = self
                    .mods
                    .values()
                    .find(|m| Self::mod_stem(&m.path) == stem)
                    .and_then(|m| m.path.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| stem.to_string());
                if self.other.upsert_github(stem, &file_name, r) {
                    self.other.save();
                }
            }
            None => {
                self.config.settings.mod_links.remove(stem);
                if self.other.remove_github(stem) {
                    self.other.save();
                }
            }
        }
        self.config.save()
    }

    /// Every GitHub-linked mod (stem -> repo), for "check all"
    /// runs: the store's GitHub rows merged over the config
    /// fallback, the store winning per stem.
    pub fn mod_links(&self) -> Vec<(String, String)> {
        let mut merged: std::collections::BTreeMap<String, String> = self
            .config
            .settings
            .mod_links
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for row in self.other.rows() {
            if row.source_kind == "github" {
                if let Some(repo) = crate::sources::github::normalize_repo(&row.source_ref) {
                    merged.insert(row.stem.clone(), repo);
                }
            }
        }
        merged.into_iter().collect()
    }

    /// The GIANTS ModHub mod_id this mod is linked to, if any.
    pub fn modhub_link(&self, stem: &str) -> Option<&str> {
        self.config
            .settings
            .modhub_links
            .get(stem)
            .map(|s| s.as_str())
    }

    /// The catalog URL from settings (the published ModHub
    /// index the app reads; empty disables fetching).
    pub fn catalog_url(&self) -> String {
        self.config.settings.catalog_url.clone()
    }

    /// Link (or, with None, unlink) a mod stem to a ModHub
    /// mod_id; persists to the config file.
    pub fn set_modhub_link(&mut self, stem: &str, mod_id: Option<String>) -> Result<()> {
        match mod_id {
            Some(id) => {
                self.config.settings.modhub_links.insert(stem.to_string(), id);
            }
            None => {
                self.config.settings.modhub_links.remove(stem);
            }
        }
        self.config.save()
    }

    /// Every ModHub-linked mod (stem -> mod_id).
    pub fn modhub_links(&self) -> Vec<(String, String)> {
        let mut links: Vec<(String, String)> = self
            .config
            .settings
            .modhub_links
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        links.sort();
        links
    }

    /// The GitHub API endpoint + token from the config's GitHub
    /// source entry: (base url, token when one is set). The base
    /// is configurable so tests can use a local mock server.
    pub fn github_endpoint(&self) -> (String, Option<String>) {
        let entry = self.config.mod_sources.github.first();
        let base = entry
            .map(|e| e.url.trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "https://api.github.com".to_string());
        let token = entry
            .and_then(|e| e.api_key.clone())
            .filter(|k| !k.trim().is_empty());
        (base, token)
    }

    /// Install an update: replace a mod's content with a
    /// downloaded newer version, in place (same folder, same
    /// name — "overwrite the outdated file"). ZIP mods swap
    /// the file; FOLDER mods swap the folder's contents with
    /// the downloaded zip's contents, the old folder held
    /// aside as a backup until the new one re-reads clean.
    /// Either way the download is validated as a readable
    /// mod BEFORE the old content is touched, and the old
    /// mod's favorite key and loadout entries are migrated
    /// to the new content's key, so stars and save loadouts
    /// survive the version bump. Returns "old -> new"
    /// version text.
    pub fn replace_mod_file(&mut self, mod_id: &str, downloaded: &std::path::Path) -> Result<String> {
        let old = self
            .mods
            .get(mod_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        // Imports land sorted (Phase 3): a mod that lives
        // ONLY in the game folder has no library home yet,
        // so its update is the moment a new file enters the
        // library. When the library is in layout form (and
        // the folders share a volume, so the mod can be
        // projected back), the download lands at the mod's
        // category home and the game entry becomes its
        // projection. In a flat library the install stays
        // exactly what it always was: in place, below.
        if !self.is_library_path(&old.path)
            && old.state == ModState::Active
            && self.library_in_layout_form()
            && super::layout_exec::same_volume(&self.archive_dir, &self.active_dir)
                == Some(true)
        {
            return self.replace_into_library(&old, downloaded);
        }
        // Ruling E, captured BEFORE anything is replaced:
        // does the game folder hold a proven projection of
        // this library copy? If so, the install must leave
        // that entry reading the NEW file (see
        // `finish_replace`).
        let was_projection = self.game_entry_is_projection(&old);
        match old.mod_type {
            ModType::Folder => return self.replace_folder_mod(&old, downloaded, was_projection),
            ModType::Zip => {}
            _ => anyhow::bail!(
                "Updates install over .zip or folder mods — {} is neither.",
                old.name
            ),
        }
        // Validate the download under a .zip name next to the mod,
        // so a bad download never costs the working file.
        let parent = old
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Mod path has no parent folder"))?
            .to_path_buf();
        let probe = parent.join(".playground-update-probe.zip");
        std::fs::copy(downloaded, &probe)
            .context("Could not stage the downloaded file for checking")?;
        let probe_result = Mod::new(probe.clone());
        let _ = std::fs::remove_file(&probe);
        probe_result
            .map_err(|_| anyhow::anyhow!("The downloaded file is not a readable mod — keeping the old file."))?;

        // The swap: same folder, same filename.
        let staged = parent.join(".playground-update-new.zip");
        std::fs::copy(downloaded, &staged).context("Could not stage the update")?;
        std::fs::remove_file(&old.path).context("Could not remove the outdated file")?;
        if let Err(e) = std::fs::rename(&staged, &old.path) {
            let _ = std::fs::remove_file(&staged);
            return Err(e).context("Could not put the updated file in place");
        }

        // Re-read the new file at the old path; the id is a hash
        // of the path, so it survives the swap unchanged.
        let fresh = Mod::new(old.path.clone())
            .context("The updated file would not re-read after installing")?;
        self.finish_replace(&old, fresh, was_projection)
    }

    /// The folder-mod half of `replace_mod_file`: the
    /// download (a zip, as every source serves) replaces the
    /// folder's whole contents. The old folder is renamed
    /// aside first and only deleted once the new contents
    /// re-read as a mod — an unpack or parse failure puts
    /// the old folder back exactly as it was.
    fn replace_folder_mod(
        &mut self,
        old: &Mod,
        downloaded: &std::path::Path,
        was_projection: bool,
    ) -> Result<String> {
        let folder = old.path.clone();
        let parent = folder
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Mod folder has no parent folder"))?
            .to_path_buf();
        // Validate the download under a .zip name BEFORE the
        // folder is touched, same as the zip path.
        let probe = parent.join(".playground-update-probe.zip");
        std::fs::copy(downloaded, &probe)
            .context("Could not stage the downloaded file for checking")?;
        let probe_result = Mod::new(probe.clone());
        let _ = std::fs::remove_file(&probe);
        probe_result.map_err(|_| {
            anyhow::anyhow!(
                "The downloaded file is not a readable mod — keeping the old folder."
            )
        })?;

        let folder_name = folder
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "mod".to_string());
        let backup = parent.join(format!(".playground-update-backup-{}", folder_name));
        if backup.exists() {
            std::fs::remove_dir_all(&backup)
                .context("Could not clear a stale update backup")?;
        }
        std::fs::rename(&folder, &backup)
            .context("Could not move the old folder aside")?;

        // Unpack into a fresh folder at the old path.
        let unpacked = (|| -> Result<()> {
            std::fs::create_dir_all(&folder)?;
            extract_zip(downloaded, &folder)
        })();
        if let Err(e) = unpacked {
            let _ = std::fs::remove_dir_all(&folder);
            let _ = std::fs::rename(&backup, &folder);
            return Err(e)
                .context("Could not unpack the update — the old folder was restored");
        }
        match Mod::new(folder.clone()) {
            Ok(fresh) => {
                let _ = std::fs::remove_dir_all(&backup);
                self.finish_replace(old, fresh, was_projection)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&folder);
                let _ = std::fs::rename(&backup, &folder);
                Err(e).context(
                    "The unpacked update would not re-read as a mod — the old folder was restored",
                )
            }
        }
    }

    /// Shared tail of an update install, once the new
    /// content stands at the old path and has been re-read:
    /// install it in the map (the id is a hash of the path,
    /// so it survives the swap) and carry favorites and
    /// loadout entries from the old favorite key to the new
    /// one. Returns "old -> new" version text.
    fn finish_replace(
        &mut self,
        old: &Mod,
        mut fresh: Mod,
        was_projection: bool,
    ) -> Result<String> {
        fresh.state = old.state.clone();
        let old_key = old.favorite_key();
        let old_version = old.version.clone();
        let new_key = fresh.favorite_key();
        let new_version = fresh.version.clone();
        // Ruling E: the replaced library copy had a game
        // projection — the entry must not be left reading
        // the stranded old file. (A sorted-landing install
        // passes false: it builds its projection itself.)
        if was_projection {
            self.refresh_projection(&fresh);
        }
        self.mods.insert(fresh.id.clone(), fresh);
        // The replacement's modDesc may declare a
        // different action set than the file it replaced.
        self.rebuild_action_owners();

        let mut dirty = false;
        let favs = &mut self.config.settings.favorites;
        if let Some(pos) = favs.iter().position(|f| f == &old_key) {
            favs[pos] = new_key.clone();
            dirty = true;
        }
        for keys in self.config.settings.save_loadouts.values_mut() {
            for k in keys.iter_mut() {
                if *k == old_key {
                    *k = new_key.clone();
                    dirty = true;
                }
            }
        }
        if dirty {
            self.config.save()?;
        }

        Ok(format!("{} -> {}", old_version, new_version))
    }

    /// Ruling E's repair: after an update replaced the
    /// library copy at `m.path`, make sure the game-folder
    /// entry reads the NEW file. When the replace preserved
    /// the file's identity the projection already carries
    /// the new content (and a junction follows its target
    /// path by construction) — the executor's proof says
    /// so and there is nothing to do. When the replace
    /// created a new file (the zip swap removes and
    /// renames), the old entry still names the stranded
    /// file: remove it and project the new copy. If even
    /// that fails, fall back to a real copy so the game
    /// keeps an updated, working mod rather than none.
    fn refresh_projection(&self, m: &Mod) {
        let entry = self.active_dir.join(m.file_name());
        if std::fs::symlink_metadata(&entry).is_err() {
            return;
        }
        let kind = Self::projection_kind(m);
        if super::layout_exec::projection_proven(&m.path, &entry, kind) {
            return;
        }
        if super::layout_exec::remove_projected_entry(&entry).is_err() {
            return;
        }
        if super::layout_exec::create_projection(&m.path, &entry, kind).is_err() {
            match m.mod_type {
                ModType::Folder => {
                    let _ = copy_dir_all(&m.path, &entry);
                }
                _ => {
                    let _ = std::fs::copy(&m.path, &entry);
                }
            }
        }
    }

    /// The sorted landing for an update install whose mod
    /// lives only in the game folder (see the dispatch in
    /// `replace_mod_file`): the download is validated
    /// exactly as the in-place installs validate theirs,
    /// placed at the mod's category home in the library,
    /// and projected back into the game folder — the mod
    /// gains a library home and stays active, as a link.
    /// Every failure before the game side is touched
    /// leaves the installed mod exactly as it was.
    fn replace_into_library(
        &mut self,
        old: &Mod,
        downloaded: &std::path::Path,
    ) -> Result<String> {
        let game_path = old.path.clone();
        let parent = game_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Mod path has no parent folder"))?
            .to_path_buf();
        // Validate the download under a .zip name BEFORE
        // anything is touched, same as the in-place paths,
        // and keep the probe's record: its categories pick
        // the library home.
        let probe = parent.join(".playground-update-probe.zip");
        std::fs::copy(downloaded, &probe)
            .context("Could not stage the downloaded file for checking")?;
        let probe_result = Mod::new(probe.clone());
        let _ = std::fs::remove_file(&probe);
        let probe_mod = probe_result.map_err(|_| {
            anyhow::anyhow!(
                "The downloaded file is not a readable mod — keeping the installed one."
            )
        })?;

        let file_name = old.file_name();
        let home = self
            .archive_dir
            .join(super::layout::sanitize_category(
                &super::layout::layout_category(&probe_mod),
            ))
            .join(&file_name);
        let category_dir = home
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Library home has no parent folder"))?
            .to_path_buf();
        if home.exists() {
            anyhow::bail!(
                "A file already sits at the library home {} — not overwriting it.",
                home.display()
            );
        }
        std::fs::create_dir_all(&category_dir)
            .context("Could not create the category folder in the library")?;

        match old.mod_type {
            ModType::Folder => {
                // Unpack the download into a fresh folder
                // at the home; the game folder is touched
                // only once the new folder re-reads clean.
                std::fs::create_dir_all(&home)
                    .context("Could not create the mod's library home")?;
                let unpacked = extract_zip(downloaded, &home);
                let fresh = unpacked.and_then(|_| {
                    Mod::new(home.clone())
                        .context("The unpacked update would not re-read as a mod")
                });
                let fresh = match fresh {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&home);
                        return Err(e.context(
                            "Could not install the update into the library — the installed folder was left as it was",
                        ));
                    }
                };
                // The game side: the old folder aside,
                // then the junction in its place. If the
                // junction cannot be made, the new folder
                // simply moves into the game folder and the
                // install completes in place instead.
                let backup =
                    parent.join(format!(".playground-update-backup-{}", file_name));
                if backup.exists() {
                    std::fs::remove_dir_all(&backup)
                        .context("Could not clear a stale update backup")?;
                }
                std::fs::rename(&game_path, &backup)
                    .context("Could not move the old folder aside")?;
                match super::layout_exec::create_projection(
                    &home,
                    &game_path,
                    super::layout::LinkKind::Junction,
                ) {
                    Ok(()) => {
                        let _ = std::fs::remove_dir_all(&backup);
                        self.mods.remove(&old.id);
                        self.finish_replace(old, fresh, false)
                    }
                    Err(_) => match std::fs::rename(&home, &game_path) {
                        Ok(()) => {
                            let _ = std::fs::remove_dir_all(&backup);
                            let fresh =
                                Mod::new(game_path.clone()).context(
                                    "The updated file would not re-read after installing",
                                )?;
                            self.finish_replace(old, fresh, false)
                        }
                        Err(e) => {
                            let _ = std::fs::rename(&backup, &game_path);
                            Err(e).context(
                                "Could not put the updated folder in place — the old folder was restored",
                            )
                        }
                    },
                }
            }
            ModType::Zip => {
                // Stage the new file beside its home, move
                // it in, re-read — only then swap the game
                // entry for a hardlink.
                let staged = category_dir.join(".playground-update-new.zip");
                std::fs::copy(downloaded, &staged)
                    .context("Could not stage the update in the library")?;
                if let Err(e) = std::fs::rename(&staged, &home) {
                    let _ = std::fs::remove_file(&staged);
                    return Err(e)
                        .context("Could not put the updated file in the library");
                }
                let fresh = match Mod::new(home.clone()) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = std::fs::remove_file(&home);
                        return Err(e).context(
                            "The updated file would not re-read after installing — the installed file was left as it was",
                        );
                    }
                };
                if std::fs::remove_file(&game_path).is_ok() {
                    if super::layout_exec::create_projection(
                        &home,
                        &game_path,
                        super::layout::LinkKind::Hardlink,
                    )
                    .is_err()
                    {
                        // Still updated, just not linked:
                        // a real copy where the link belongs.
                        let _ = std::fs::copy(&home, &game_path);
                    }
                }
                self.mods.remove(&old.id);
                self.finish_replace(old, fresh, false)
            }
            _ => anyhow::bail!(
                "Updates install over .zip or folder mods — {} is neither.",
                old.name
            ),
        }
    }

    /// The game's profile folder, resolved DIRECTLY (John,
    /// 2026-10-21): Documents/My Games/FarmingSimulator2025,
    /// where log.txt and the savegame folders actually live.
    /// The old derivation — the parent of the Active folder
    /// when it is named "mods" — survives only as a fallback:
    /// with Active at D:\mods it read D:\ as the profile and
    /// sent the Log Reader hunting D:\Log.txt, and the saves
    /// with it.
    pub fn profile_dir(&self) -> Option<PathBuf> {
        if let Some(documents) = dirs::document_dir() {
            let profile = documents.join("My Games").join("FarmingSimulator2025");
            if profile.is_dir() {
                return Some(profile);
            }
        }
        let active = &self.active_dir;
        if active
            .file_name()
            .map_or(false, |n| n.eq_ignore_ascii_case("mods"))
        {
            active.parent().map(|p| p.to_path_buf())
        } else {
            None
        }
    }

    /// Save slots found in the profile folder (empty when the active
    /// folder isn't a game `mods` folder).
    pub fn save_slots(&self) -> Vec<super::saves::SaveSlot> {
        match self.profile_dir() {
            Some(profile) => super::saves::list_save_slots(&profile),
            None => Vec::new(),
        }
    }

    /// The stored loadout for a save slot (favorite_keys), if any.
    pub fn save_loadout(&self, slot_dir_name: &str) -> Vec<String> {
        self.config
            .settings
            .save_loadouts
            .get(slot_dir_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Store a save slot's loadout; persists to the config file.
    pub fn set_save_loadout(&mut self, slot_dir_name: &str, keys: Vec<String>) -> Result<()> {
        self.config
            .settings
            .save_loadouts
            .insert(slot_dir_name.to_string(), keys);
        self.config.save()
    }

    /// Stage a loadout: make the Active folder hold exactly the mods
    /// whose favorite_key is in `keys`. Active mods not in the set
    /// move to Archive; archived mods in the set move to Active.
    /// Returns (moved to archive, moved to active).
    pub fn apply_loadout(&mut self, keys: &[String]) -> Result<(usize, usize)> {
        let wanted: std::collections::HashSet<&str> =
            keys.iter().map(|k| k.as_str()).collect();
        // Snapshot first: moves mutate the map's entries.
        let snapshot: Vec<(String, String, ModState)> = self
            .mods
            .values()
            .map(|m| (m.id.clone(), m.favorite_key(), m.state.clone()))
            .collect();
        let mut to_archive = 0usize;
        let mut to_active = 0usize;
        for (id, key, state) in snapshot {
            let in_set = wanted.contains(key.as_str());
            match state {
                ModState::Active if !in_set => {
                    self.move_to_archive(&id)?;
                    to_archive += 1;
                }
                ModState::Archived if in_set => {
                    self.move_to_active(&id)?;
                    to_active += 1;
                }
                _ => {}
            }
        }
        Ok((to_archive, to_active))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, GameDirectories, ModSources, Settings};

    fn write_mod(dir: &std::path::Path, folder: &str, title: &str) {
        let mod_dir = dir.join(folder);
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(
            mod_dir.join("modDesc.xml"),
            format!(
                "<modDesc><author>T</author><title><en>{}</en></title><version>1.0.0.0</version></modDesc>",
                title
            ),
        )
        .unwrap();
    }

    #[test]
    fn apply_loadout_stages_the_right_mods() {
        let base = std::env::temp_dir().join(format!("mm-loadout-test-{}", std::process::id()));
        let active = base.join("profile").join("mods");
        let archive = base.join("archive");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        write_mod(&active, "ModA", "Mod A");
        write_mod(&active, "ModB", "Mod B");
        write_mod(&archive, "ModC", "Mod C");

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
                archive_directory: archive.clone(),
                active_directory: active.clone(),
                check_conflicts: true,
                parallel_scans: 2,
                cache_ttl_hours: 24,
                favorites: vec![],
                save_loadouts: HashMap::new(),
                game_directory: None,
                game_install_dir: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: String::new(),
                startup_update_check: true,
                self_update_enabled: true,
                browse_default_view: "all".to_string(),
                start_maximized: false,
                auto_file_new_mods: false,
                keep_download_history: true,
                fill_log_keep: 10,
                log_errors_only_default: false,
                log_lines_cap: 3000,
                launch: crate::config::LaunchConfig::default(),
            },
        };
        let mut manager = ModManager::new(config).unwrap();
        assert_eq!(manager.get_mods().len(), 3);

        let key_of = |manager: &ModManager, folder: &str| {
            manager
                .get_mods()
                .into_iter()
                .find(|m| m.path.ends_with(folder))
                .unwrap()
                .favorite_key()
        };
        // Loadout = A (already active) + C (archived). B must leave.
        let loadout = vec![key_of(&manager, "ModA"), key_of(&manager, "ModC")];
        let (to_archive, to_active) = manager.apply_loadout(&loadout).unwrap();
        assert_eq!((to_archive, to_active), (1, 1));
        assert!(active.join("ModA").exists());
        assert!(archive.join("ModB").exists());
        assert!(active.join("ModC").exists());

        // Profile dir = parent of the "mods" folder.
        assert_eq!(
            manager.profile_dir(),
            Some(base.join("profile"))
        );

        std::fs::remove_dir_all(&base).ok();
    }

    fn test_config(archive: std::path::PathBuf, active: std::path::PathBuf) -> Config {
        Config {
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
                game_install_dir: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: String::new(),
                startup_update_check: true,
                self_update_enabled: true,
                browse_default_view: "all".to_string(),
                start_maximized: false,
                auto_file_new_mods: false,
                keep_download_history: true,
                fill_log_keep: 10,
                log_errors_only_default: false,
                log_lines_cap: 3000,
                launch: crate::config::LaunchConfig::default(),
            },
        }
    }

    fn write_download_zip(path: &std::path::Path, version: &str) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("modDesc.xml", opts).unwrap();
        zip.write_all(
            format!(
                "<modDesc><author>T</author><title><en>Folder Mod</en></title><version>{}</version></modDesc>",
                version
            )
            .as_bytes(),
        )
        .unwrap();
        zip.start_file("newfile.txt", opts).unwrap();
        zip.write_all(b"new content").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn folder_mod_update_replaces_contents_and_carries_the_favorite() {
        let base =
            std::env::temp_dir().join(format!("mm-folder-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let archive = base.join("archive");
        let active = base.join("active");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        // The installed folder mod: v1.0.0.0 plus a stale
        // file the new version no longer ships.
        let folder = archive.join("FolderMod");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("modDesc.xml"),
            "<modDesc><author>T</author><title><en>Folder Mod</en></title><version>1.0.0.0</version></modDesc>",
        )
        .unwrap();
        std::fs::write(folder.join("stale.txt"), "old content").unwrap();
        // The downloaded update: v2.0.0.0, no stale.txt.
        let download = base.join("download.zip");
        write_download_zip(&download, "2.0.0.0");

        let mut manager =
            ModManager::new(test_config(archive.clone(), active)).unwrap();
        let installed = manager
            .get_mods()
            .into_iter()
            .find(|m| m.path == folder)
            .cloned()
            .unwrap();
        assert_eq!(installed.mod_type, ModType::Folder);
        let id = installed.id.clone();
        let old_key = installed.favorite_key();
        manager.toggle_favorite(&old_key).unwrap();
        assert!(manager.is_favorite(&old_key));

        // The library DB remembers the OLD folder; its
        // record must self-heal after the replace.
        let db =
            crate::mod_manager::library_db::LibraryDb::load_from(base.join("library-db.json"));
        assert_eq!(db.get_or_parse(&folder).unwrap().version, "1.0.0.0");

        let change = manager.replace_mod_file(&id, &download).unwrap();
        assert_eq!(change, "1.0.0.0 -> 2.0.0.0");

        // The folder was fully replaced, not merged.
        assert!(folder.join("newfile.txt").exists());
        assert!(
            !folder.join("stale.txt").exists(),
            "the old folder's leftovers must not survive the replace"
        );
        // The backup is gone once the new folder reads clean.
        assert!(!archive.join(".playground-update-backup-FolderMod").exists());

        // The manager now holds the new version, and the
        // favorite rode over to the new key.
        let fresh = manager.get_mod(&id).unwrap();
        assert_eq!(fresh.version, "2.0.0.0");
        assert!(!manager.is_favorite(&old_key));
        assert!(manager.is_favorite(&fresh.favorite_key()));

        // Self-heal: the stored record's fingerprint no
        // longer matches the replaced folder, so the next
        // ask re-parses and serves the new version.
        assert_eq!(db.get_or_parse(&folder).unwrap().version, "2.0.0.0");

        std::fs::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------
    // Phase 3 (steady state) fixtures. The modDesc shapes
    // mirror the ones the Phase 1/2 rigs proved against
    // the real binary: a plain desc lands in Scripts, a
    // <maps> desc in Maps, a store-item desc in its
    // bucket, and no desc at all in Uncategorized (the
    // layout's "Other").
    // -----------------------------------------------------------------

    fn desc_plain(version: &str, title: &str) -> String {
        format!(
            "<modDesc><version>{}</version><title><en>{}</en></title></modDesc>",
            version, title
        )
    }

    fn desc_map(version: &str, title: &str) -> String {
        format!(
            "<modDesc><version>{}</version><title><en>{}</en></title><maps><map id=\"1\"/></maps></modDesc>",
            version, title
        )
    }

    fn desc_tractor(version: &str, title: &str) -> String {
        format!(
            "<modDesc><version>{}</version><title><en>{}</en></title><storeItems><storeItem xmlFilename=\"tractor.xml\"/></storeItems></modDesc>",
            version, title
        )
    }

    const TRACTOR_XML: &str = "<vehicle type=\"tractor\"><storeData><category>tractorsSmall</category></storeData></vehicle>";

    /// Build a zip in memory (Stored, so byte lengths are
    /// exact — the twin fixtures depend on it).
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn write_zip_file(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, zip_bytes(entries)).unwrap();
    }

    /// A .dlc pack: opaque bytes, deliberately not a
    /// zip — GIANTS' DLC files are encrypted, so no
    /// modDesc is readable and the scan record is the
    /// filename-based fallback `Mod::new` builds.
    fn write_dlc_file(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"\x7fDLC opaque pack bytes, not a zip").unwrap();
    }

    fn write_folder_mod(dir: &std::path::Path, desc: &str, files: &[(&str, &[u8])]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("modDesc.xml"), desc).unwrap();
        for (name, data) in files {
            std::fs::write(dir.join(name), data).unwrap();
        }
    }

    /// Project a file the way the layout does: a hardlink
    /// where the platform supports the test, a plain copy
    /// elsewhere (the twin test cannot tell them apart,
    /// which is the point).
    fn project_file(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::fs::hard_link(src, dst).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(src, dst).unwrap();
    }

    fn project_folder(src: &std::path::Path, dst: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(src, dst).unwrap();
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(dst).unwrap();
            for e in std::fs::read_dir(src).unwrap() {
                let e = e.unwrap();
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }

    #[cfg(unix)]
    fn dev_ino(path: &std::path::Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(path).unwrap();
        (md.dev(), md.ino())
    }

    fn scan_scratch(
        base: &std::path::Path,
        archive: &std::path::Path,
        active: &std::path::Path,
    ) -> Vec<Mod> {
        let db = crate::mod_manager::library_db::LibraryDb::load_from(
            base.join("library-db.json"),
        );
        let mods = ModManager::scan_directories_with_db(
            archive.to_path_buf(),
            active.to_path_buf(),
            2,
            &db,
            &|_, _| {},
        )
        .unwrap();
        let mut mods: Vec<Mod> = mods.into_values().collect();
        mods.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then(a.path.cmp(&b.path)));
        mods
    }

    #[test]
    fn migrated_tree_scans_as_one_library() {
        let base =
            std::env::temp_dir().join(format!("mm-p3-census-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();

        // The migrated library: one copy of each mod,
        // filed by category.
        write_folder_mod(
            &lib.join("Maps").join("FS25_CensusMap"),
            &desc_map("1.0.0.0", "Census Map"),
            &[("data.bin", b"map-data")],
        );
        write_zip_file(
            &lib.join("Scripts").join("FS25_CensusScript.zip"),
            &[
                ("modDesc.xml", desc_plain("1.0.0.0", "Census Script").as_bytes()),
                ("data.bin", b"script-data"),
            ],
        );
        write_zip_file(
            &lib.join("Other").join("FS25_CensusPlain.zip"),
            &[("data.txt", b"no moddesc here")],
        );
        write_zip_file(
            &lib.join("Scripts").join("FS25_CensusDiverged.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Census Diverged").as_bytes())],
        );
        // Projections for the active two.
        project_folder(
            &lib.join("Maps").join("FS25_CensusMap"),
            &game.join("FS25_CensusMap"),
        );
        project_file(
            &lib.join("Scripts").join("FS25_CensusScript.zip"),
            &game.join("FS25_CensusScript.zip"),
        );
        // A diverged real copy in the game folder: same
        // name, different version — NOT a projection.
        write_zip_file(
            &game.join("FS25_CensusDiverged.zip"),
            &[
                ("modDesc.xml", desc_plain("2.0.0.0", "Census Diverged").as_bytes()),
                ("extra.bin", b"a longer payload"),
            ],
        );
        // A game-only mod.
        write_zip_file(
            &game.join("FS25_CensusGameOnly.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Census GameOnly").as_bytes())],
        );

        let mods = scan_scratch(&base, &lib, &game);

        // The census: 5 library/game mods + the diverged
        // twin counted twice = 6 records, and NO phantom
        // record for any category folder.
        let names: Vec<String> = mods.iter().map(|m| m.file_name()).collect();
        assert_eq!(
            names,
            vec![
                "FS25_CensusDiverged.zip",
                "FS25_CensusDiverged.zip",
                "FS25_CensusGameOnly.zip",
                "FS25_CensusMap",
                "FS25_CensusPlain.zip",
                "FS25_CensusScript.zip",
            ]
        );
        assert!(mods.iter().all(|m| {
            m.path != lib.join("Maps") && m.path != lib.join("Scripts") && m.path != lib.join("Other")
        }));

        let by_name = |n: &str| mods.iter().filter(|m| m.file_name() == n).collect::<Vec<_>>();
        let map = &by_name("FS25_CensusMap")[0];
        assert_eq!(map.path, lib.join("Maps").join("FS25_CensusMap"));
        assert_eq!(map.state, ModState::Active);
        assert!(map.categories.iter().any(|c| c == "Maps"));

        let script = &by_name("FS25_CensusScript.zip")[0];
        assert_eq!(script.path, lib.join("Scripts").join("FS25_CensusScript.zip"));
        assert_eq!(script.state, ModState::Active);

        let plain = &by_name("FS25_CensusPlain.zip")[0];
        assert_eq!(plain.path, lib.join("Other").join("FS25_CensusPlain.zip"));
        assert_eq!(plain.state, ModState::Archived);

        let diverged = by_name("FS25_CensusDiverged.zip");
        assert_eq!(diverged.len(), 2);
        let lib_copy = diverged.iter().find(|m| m.path.starts_with(&lib)).unwrap();
        let game_copy = diverged.iter().find(|m| m.path.starts_with(&game)).unwrap();
        assert_eq!(lib_copy.state, ModState::Archived);
        assert_eq!(lib_copy.version, "1.0.0.0");
        assert_eq!(game_copy.state, ModState::Active);
        assert_eq!(game_copy.version, "2.0.0.0");

        let game_only = &by_name("FS25_CensusGameOnly.zip")[0];
        assert_eq!(game_only.path, game.join("FS25_CensusGameOnly.zip"));
        assert_eq!(game_only.state, ModState::Active);

        // Conflicts: only the diverged pair — projections
        // produce none.
        let conflicts =
            crate::mod_manager::conflict::ConflictDetector::detect_conflicts(&mods);
        assert!(!conflicts.is_empty());
        assert!(conflicts
            .iter()
            .all(|c| c.mod1 == "FS25_CensusDiverged.zip" || c.mod2 == "FS25_CensusDiverged.zip"));

        std::fs::remove_dir_all(&base).ok();
    }

    fn write_entries_zip(path: &std::path::Path, entries: &[(&str, &str)]) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn map_detected_from_maps_folder_not_paperwork() {
        // Owner ruling, 2026-10-22: a top-level maps/ folder
        // in the zip makes the mod a map even when the modDesc
        // declares no <maps> section — and Maps is its only
        // category, even though the mod bundles a placeable
        // store item (the old code bucketed it by that item).
        let base =
            std::env::temp_dir().join(format!("mm-mapfolder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let zip_path = base.join("FS25_MapNoDeclare.zip");
        write_entries_zip(
            &zip_path,
            &[
                (
                    "modDesc.xml",
                    "<modDesc><author>T</author><title><en>Map</en></title><version>1.0.0.0</version><storeItems><storeItem xmlFilename=\"barn.xml\"/></storeItems></modDesc>",
                ),
                (
                    "barn.xml",
                    "<placeable><storeData><category>husbandry</category></storeData></placeable>",
                ),
                ("maps/testmap/map.xml", "<map/>"),
            ],
        );
        let m = crate::mod_manager::types::Mod::new(zip_path).unwrap();
        assert_eq!(m.categories, vec!["Maps".to_string()]);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn categories_use_the_games_names_with_sizes() {
        // Owner ruling, 2026-10-22: the store item's declared
        // category, named the game's way — tractorsS is Small
        // Tractors (the size class is preserved, not flattened
        // into "Tractors"), and an id with no special name
        // displays cleaned (slurryTanks -> "Slurry Tanks"),
        // never reinterpreted into an invented bucket.
        let base =
            std::env::temp_dir().join(format!("mm-gamecat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let small = base.join("FS25_SmallTractor.zip");
        write_entries_zip(
            &small,
            &[
                (
                    "modDesc.xml",
                    "<modDesc><author>T</author><title><en>Tractor</en></title><version>1.0.0.0</version><storeItems><storeItem xmlFilename=\"tractor.xml\"/></storeItems></modDesc>",
                ),
                (
                    "tractor.xml",
                    "<vehicle type=\"tractor\"><storeData><category>tractorsS</category></storeData></vehicle>",
                ),
            ],
        );
        let m = crate::mod_manager::types::Mod::new(small).unwrap();
        assert_eq!(m.categories, vec!["Small Tractors".to_string()]);

        let tank = base.join("FS25_SlurryTank.zip");
        write_entries_zip(
            &tank,
            &[
                (
                    "modDesc.xml",
                    "<modDesc><author>T</author><title><en>Tank</en></title><version>1.0.0.0</version><storeItems><storeItem xmlFilename=\"tank.xml\"/></storeItems></modDesc>",
                ),
                (
                    "tank.xml",
                    "<vehicle type=\"trailer\"><storeData><category>slurryTanks</category></storeData></vehicle>",
                ),
            ],
        );
        let m = crate::mod_manager::types::Mod::new(tank).unwrap();
        assert_eq!(m.categories, vec!["Slurry Tanks".to_string()]);

        std::fs::remove_dir_all(&base).ok();
    }

    /// THE acceptance scenario (owner, 2026-10-21, after
    /// the v1.3.0 field test): the migrated library's mod
    /// list must be indistinguishable from the flat
    /// library's — same filenames, same categories, same
    /// active flags — with the ONLY change being that the
    /// active mods are projections in the game folder.
    #[test]
    fn layout_list_equality_flat_vs_migrated() {
        let build = |tag: &str, migrated: bool| {
            let base = std::env::temp_dir().join(format!(
                "mm-p3-equal-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            let lib = base.join("lib");
            let game = base.join("game");
            std::fs::create_dir_all(&game).unwrap();
            // File a library copy: at the root when flat,
            // in its category folder when migrated.
            let at = |lib: &std::path::Path, cat: &str, name: &str| {
                if migrated {
                    lib.join(cat).join(name)
                } else {
                    lib.join(name)
                }
            };
            write_zip_file(
                &at(&lib, "Tractors", "FS25_EqTractor.zip"),
                &[
                    ("modDesc.xml", desc_tractor("1.0.0.0", "Eq Tractor").as_bytes()),
                    ("tractor.xml", TRACTOR_XML.as_bytes()),
                ],
            );
            write_zip_file(
                &at(&lib, "Maps", "FS25_EqMap.zip"),
                &[("modDesc.xml", desc_map("1.0.0.0", "Eq Map").as_bytes())],
            );
            write_zip_file(
                &at(&lib, "Scripts", "FS25_EqScript.zip"),
                &[("modDesc.xml", desc_plain("1.0.0.0", "Eq Script").as_bytes())],
            );
            write_zip_file(
                &at(&lib, "Other", "FS25_EqNoDesc.zip"),
                &[("data.txt", b"no moddesc here")],
            );
            write_folder_mod(
                &at(&lib, "Scripts", "FS25_EqFolder"),
                &desc_plain("1.0.0.0", "Eq Folder"),
                &[("data.bin", b"folder-data")],
            );
            // DLC packs (owner's strawHarvestPack.dlc,
            // 2026-10-21): one at the root in BOTH worlds
            // — a root pack must list identically before
            // and after a migration pass homes it — and
            // one filed under DLC's when migrated (its
            // kind-assigned home since the DLC rider), at
            // the root when flat. Both must list in both
            // worlds, or the migrated list loses records
            // the flat list has.
            write_dlc_file(&lib.join("EqDlcPack.dlc"));
            write_dlc_file(&at(&lib, "DLC's", "EqFiledDlc.dlc"));
            // Active set: the tractor and the folder mod,
            // plus a game-only mod — the same in both
            // worlds (links after migration, copies
            // before; the scan must not care).
            if migrated {
                project_file(
                    &at(&lib, "Tractors", "FS25_EqTractor.zip"),
                    &game.join("FS25_EqTractor.zip"),
                );
                project_folder(
                    &at(&lib, "Scripts", "FS25_EqFolder"),
                    &game.join("FS25_EqFolder"),
                );
            } else {
                std::fs::copy(
                    at(&lib, "Tractors", "FS25_EqTractor.zip"),
                    game.join("FS25_EqTractor.zip"),
                )
                .unwrap();
                write_folder_mod(
                    &game.join("FS25_EqFolder"),
                    &desc_plain("1.0.0.0", "Eq Folder"),
                    &[("data.bin", b"folder-data")],
                );
            }
            write_zip_file(
                &game.join("FS25_EqGameOnly.zip"),
                &[("modDesc.xml", desc_plain("1.0.0.0", "Eq GameOnly").as_bytes())],
            );
            (base, lib, game)
        };

        let shape = |mods: &[Mod]| {
            let mut v: Vec<(String, String, bool)> = mods
                .iter()
                .map(|m| {
                    (
                        m.file_name(),
                        m.categories.join(","),
                        m.state == ModState::Active,
                    )
                })
                .collect();
            v.sort();
            v
        };

        let (flat_base, flat_lib, flat_game) = build("flat", false);
        let flat_mods = scan_scratch(&flat_base, &flat_lib, &flat_game);
        let (mig_base, mig_lib, mig_game) = build("mig", true);
        let mig_mods = scan_scratch(&mig_base, &mig_lib, &mig_game);

        assert_eq!(flat_mods.len(), 8, "flat census: {:?}", flat_mods.iter().map(|m| m.file_name()).collect::<Vec<_>>());
        assert_eq!(
            shape(&flat_mods),
            shape(&mig_mods),
            "the migrated mod list must equal the flat one"
        );
        // And the trees really do differ underneath: the
        // migrated records point into category folders —
        // all but the root DLC pack, which the fixture
        // plants at the root in both worlds (a migration
        // pass would home it under DLC's; the scan must
        // not care where it sits).
        assert!(mig_mods
            .iter()
            .filter(|m| m.path.starts_with(&mig_lib))
            .filter(|m| m.file_name() != "EqDlcPack.dlc")
            .all(|m| m.path.parent() != Some(mig_lib.as_path())));

        // The DLC packs list in both worlds with the
        // fallback record shape — the file name for a
        // name, the default version, Archived, type
        // Unknown — except the category, which is the
        // kind-assigned DLC's (owner ruling, 2026-10-21),
        // never the Uncategorized fallback.
        for mods in [&flat_mods, &mig_mods] {
            for name in ["EqDlcPack.dlc", "EqFiledDlc.dlc"] {
                let rec = mods
                    .iter()
                    .find(|m| m.file_name() == name)
                    .unwrap_or_else(|| panic!("{} missing from the scan", name));
                assert_eq!(rec.name, name);
                assert_eq!(rec.version, "1.0.0");
                assert_eq!(rec.categories, vec!["DLC's".to_string()]);
                assert_eq!(rec.state, ModState::Archived);
                assert_eq!(rec.mod_type, ModType::Unknown);
            }
        }

        std::fs::remove_dir_all(&flat_base).ok();
        std::fs::remove_dir_all(&mig_base).ok();
    }

    #[test]
    fn layout_activate_deactivate_use_links() {
        let base =
            std::env::temp_dir().join(format!("mm-p3-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        write_zip_file(
            &lib.join("Scripts").join("FS25_LinkScript.zip"),
            &[
                ("modDesc.xml", desc_plain("1.0.0.0", "Link Script").as_bytes()),
                ("data.bin", b"script-bytes"),
            ],
        );
        write_folder_mod(
            &lib.join("Maps").join("FS25_LinkMap"),
            &desc_map("1.0.0.0", "Link Map"),
            &[("data.bin", b"map-bytes")],
        );
        let script_home = lib.join("Scripts").join("FS25_LinkScript.zip");
        let script_bytes = std::fs::read(&script_home).unwrap();

        let mut manager = ModManager::new(test_config(lib.clone(), game.clone())).unwrap();
        let find = |manager: &ModManager, name: &str| {
            manager
                .get_mods()
                .into_iter()
                .find(|m| m.file_name() == name)
                .unwrap()
                .clone()
        };

        // Activate the zip: a projection appears, the
        // library file is untouched, the record stays
        // keyed to the library copy.
        let script = find(&manager, "FS25_LinkScript.zip");
        assert_eq!(script.state, ModState::Archived);
        manager.move_to_active(&script.id).unwrap();
        let entry = game.join("FS25_LinkScript.zip");
        assert!(entry.exists());
        #[cfg(unix)]
        assert_eq!(dev_ino(&entry), dev_ino(&script_home), "the game entry is the library file (hardlink)");
        assert_eq!(std::fs::read(&script_home).unwrap(), script_bytes);
        let script = find(&manager, "FS25_LinkScript.zip");
        assert_eq!(script.state, ModState::Active);
        assert_eq!(script.path, script_home);

        // Deactivate: the link goes, the library bytes stay.
        manager.move_to_archive(&script.id).unwrap();
        assert!(!entry.exists());
        assert_eq!(std::fs::read(&script_home).unwrap(), script_bytes);
        assert_eq!(find(&manager, "FS25_LinkScript.zip").state, ModState::Archived);

        // The folder mod projects as a link too.
        let map = find(&manager, "FS25_LinkMap");
        manager.move_to_active(&map.id).unwrap();
        let map_entry = game.join("FS25_LinkMap");
        assert!(map_entry.join("modDesc.xml").exists());
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(&map_entry)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the folder projection is a link, not a copy"
        );
        manager.move_to_archive(&map.id).unwrap();
        assert!(std::fs::symlink_metadata(&map_entry).is_err());
        assert!(lib.join("Maps").join("FS25_LinkMap").join("data.bin").exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn deactivate_against_a_real_file_uses_move_semantics() {
        // Same name, same size, same version — but a REAL
        // game copy with different bytes (not a hardlink).
        // Deactivation must not just unlink it (that would
        // silently keep the library's variant); the game
        // copy moves onto the library home, as it always
        // did — proven by whose bytes survive.
        let base =
            std::env::temp_dir().join(format!("mm-p3-twinmove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let desc = desc_plain("1.0.0.0", "Twin Move");
        let bytes_a = zip_bytes(&[
            ("modDesc.xml", desc.as_bytes()),
            ("pad.bin", &[b'A'; 64]),
        ]);
        let bytes_b = zip_bytes(&[
            ("modDesc.xml", desc.as_bytes()),
            ("pad.bin", &[b'B'; 64]),
        ]);
        assert_eq!(bytes_a.len(), bytes_b.len());
        let home = lib.join("Scripts").join("FS25_TwinMove.zip");
        std::fs::create_dir_all(home.parent().unwrap()).unwrap();
        std::fs::write(&home, &bytes_a).unwrap();
        let entry = game.join("FS25_TwinMove.zip");
        std::fs::write(&entry, &bytes_b).unwrap();

        let mut manager = ModManager::new(test_config(lib.clone(), game.clone())).unwrap();
        let twin = manager
            .get_mods()
            .into_iter()
            .find(|m| m.file_name() == "FS25_TwinMove.zip")
            .unwrap()
            .clone();
        // One record (the twin test passed), Active, keyed
        // to the library copy.
        assert_eq!(twin.state, ModState::Active);
        assert_eq!(twin.path, home);
        #[cfg(unix)]
        assert_ne!(dev_ino(&entry), dev_ino(&home), "fixture: a real copy, not a link");

        manager.move_to_archive(&twin.id).unwrap();
        assert!(!entry.exists());
        assert_eq!(
            std::fs::read(&home).unwrap(),
            bytes_b,
            "the game copy's bytes moved onto the library home"
        );
        assert_eq!(
            manager.get_mod(&twin.id).unwrap().state,
            ModState::Archived
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn flat_tree_scan_and_moves_behave_as_before() {
        let base =
            std::env::temp_dir().join(format!("mm-p3-flat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        write_zip_file(
            &lib.join("FS25_FlatA.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Flat A").as_bytes())],
        );
        write_zip_file(
            &lib.join("FS25_FlatT.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Flat T").as_bytes())],
        );
        std::fs::copy(lib.join("FS25_FlatT.zip"), game.join("FS25_FlatT.zip")).unwrap();
        write_zip_file(
            &game.join("FS25_FlatB.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Flat B").as_bytes())],
        );

        let mut manager = ModManager::new(test_config(lib.clone(), game.clone())).unwrap();
        let find = |manager: &ModManager, name: &str| {
            manager
                .get_mods()
                .into_iter()
                .filter(|m| m.file_name() == name)
                .map(|m| m.clone())
                .collect::<Vec<_>>()
        };
        // The flat census: A archived at the root, B
        // game-only, T one twin record.
        let a = find(&manager, "FS25_FlatA.zip");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].state, ModState::Archived);
        assert_eq!(a[0].path, lib.join("FS25_FlatA.zip"));
        let b = find(&manager, "FS25_FlatB.zip");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].state, ModState::Active);
        assert_eq!(b[0].path, game.join("FS25_FlatB.zip"));
        let t = find(&manager, "FS25_FlatT.zip");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].state, ModState::Active);

        // Activation MOVES the flat library's file, as
        // always: the archive copy leaves, a real file
        // lands in the game folder.
        let a_id = a[0].id.clone();
        manager.move_to_active(&a_id).unwrap();
        assert!(!lib.join("FS25_FlatA.zip").exists());
        assert!(game.join("FS25_FlatA.zip").exists());
        let a = find(&manager, "FS25_FlatA.zip");
        assert_eq!(a[0].path, game.join("FS25_FlatA.zip"));
        assert_eq!(a[0].state, ModState::Active);
        // And back.
        manager.move_to_archive(&a_id).unwrap();
        assert!(lib.join("FS25_FlatA.zip").exists());
        assert!(!game.join("FS25_FlatA.zip").exists());
        let a = find(&manager, "FS25_FlatA.zip");
        assert_eq!(a[0].path, lib.join("FS25_FlatA.zip"));
        assert_eq!(a[0].state, ModState::Archived);

        // Deactivating the flat twin: the game copy moves
        // onto the archive copy; one copy remains, at the
        // root.
        let t_id = t[0].id.clone();
        let t_bytes = std::fs::read(lib.join("FS25_FlatT.zip")).unwrap();
        manager.move_to_archive(&t_id).unwrap();
        assert!(!game.join("FS25_FlatT.zip").exists());
        assert_eq!(std::fs::read(lib.join("FS25_FlatT.zip")).unwrap(), t_bytes);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn update_install_keeps_the_projection_current() {
        let base =
            std::env::temp_dir().join(format!("mm-p3-updlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let home = lib.join("Scripts").join("FS25_UpdScript.zip");
        write_zip_file(
            &home,
            &[
                ("modDesc.xml", desc_plain("1.0.0.0", "Upd Script").as_bytes()),
                ("data.bin", b"v1"),
            ],
        );
        let entry = game.join("FS25_UpdScript.zip");
        project_file(&home, &entry);

        let mut manager = ModManager::new(test_config(lib.clone(), game.clone())).unwrap();
        let rec = manager
            .get_mods()
            .into_iter()
            .find(|m| m.file_name() == "FS25_UpdScript.zip")
            .unwrap()
            .clone();
        assert_eq!(rec.state, ModState::Active);
        assert_eq!(rec.path, home);

        let download = base.join("download.zip");
        write_zip_file(
            &download,
            &[
                ("modDesc.xml", desc_plain("2.0.0.0", "Upd Script").as_bytes()),
                ("data.bin", b"v2-with-more-bytes"),
            ],
        );
        let change = manager.replace_mod_file(&rec.id, &download).unwrap();
        assert_eq!(change, "1.0.0.0 -> 2.0.0.0");

        // The entry the game reads now carries the NEW
        // file — not the stranded old one (Ruling E).
        assert_eq!(std::fs::read(&entry).unwrap(), std::fs::read(&home).unwrap());
        assert_eq!(Mod::new(entry.clone()).unwrap().version, "2.0.0.0");
        #[cfg(unix)]
        assert_eq!(dev_ino(&entry), dev_ino(&home), "the entry was re-projected onto the new file");
        let rec = manager.get_mod(&rec.id).unwrap();
        assert_eq!(rec.version, "2.0.0.0");
        assert_eq!(rec.path, home);
        assert_eq!(rec.state, ModState::Active);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn game_only_update_lands_sorted_and_projects() {
        let base =
            std::env::temp_dir().join(format!("mm-p3-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = base.join("lib");
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        // The anchor: a filed library mod — the library
        // is in layout form.
        write_zip_file(
            &lib.join("Scripts").join("FS25_Anchor.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Anchor").as_bytes())],
        );
        // Two game-only mods: one with a category, one
        // without.
        write_zip_file(
            &game.join("FS25_ImpScript.zip"),
            &[("modDesc.xml", desc_plain("1.0.0.0", "Imp Script").as_bytes())],
        );
        write_zip_file(
            &game.join("FS25_ImpPlain.zip"),
            &[("data.txt", b"v1")],
        );

        let mut manager = ModManager::new(test_config(lib.clone(), game.clone())).unwrap();
        let id_of = |manager: &ModManager, name: &str| {
            manager
                .get_mods()
                .into_iter()
                .find(|m| m.file_name() == name)
                .unwrap()
                .id
                .clone()
        };

        // The categorized one lands in Scripts/.
        let dl = base.join("dl-script.zip");
        write_zip_file(
            &dl,
            &[("modDesc.xml", desc_plain("2.0.0.0", "Imp Script").as_bytes())],
        );
        let id = id_of(&manager, "FS25_ImpScript.zip");
        let change = manager.replace_mod_file(&id, &dl).unwrap();
        assert_eq!(change, "1.0.0.0 -> 2.0.0.0");
        let home = lib.join("Scripts").join("FS25_ImpScript.zip");
        assert!(home.exists());
        let rec = manager
            .get_mods()
            .into_iter()
            .find(|m| m.file_name() == "FS25_ImpScript.zip")
            .unwrap();
        assert_eq!(rec.path, home);
        assert_eq!(rec.state, ModState::Active);
        let entry = game.join("FS25_ImpScript.zip");
        assert_eq!(std::fs::read(&entry).unwrap(), std::fs::read(&home).unwrap());
        #[cfg(unix)]
        assert_eq!(dev_ino(&entry), dev_ino(&home), "projected after placement");

        // The category-less one lands in Other/, and its
        // game entry reads the new bytes through the link.
        let dl2 = base.join("dl-plain.zip");
        write_zip_file(&dl2, &[("data.txt", b"v2-longer")]);
        let id = id_of(&manager, "FS25_ImpPlain.zip");
        manager.replace_mod_file(&id, &dl2).unwrap();
        let home2 = lib.join("Other").join("FS25_ImpPlain.zip");
        assert!(home2.exists());
        let entry2 = game.join("FS25_ImpPlain.zip");
        let through = {
            let f = std::fs::File::open(&entry2).unwrap();
            let mut z = zip::ZipArchive::new(f).unwrap();
            let mut s = String::new();
            use std::io::Read as _;
            z.by_name("data.txt").unwrap().read_to_string(&mut s).unwrap();
            s
        };
        assert_eq!(through, "v2-longer");
        #[cfg(unix)]
        assert_eq!(dev_ino(&entry2), dev_ino(&home2));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_progress_counts_every_candidate_and_ends_at_total() {
        let base =
            std::env::temp_dir().join(format!("mm-progress-test-{}", std::process::id()));
        let active = base.join("active");
        let archive = base.join("archive");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        write_mod(&archive, "ModA", "Mod A");
        write_mod(&archive, "ModB", "Mod B");
        write_mod(&active, "ModC", "Mod C");
        // A stray non-mod file is NOT a candidate and must not
        // be counted in the total.
        std::fs::write(archive.join("notes.txt"), "not a mod").unwrap();

        // A scratch library database, so the test never touches
        // the real one's file.
        let db = crate::mod_manager::library_db::LibraryDb::load_from(
            base.join("library-db.json"),
        );
        let ticks = std::sync::Mutex::new(Vec::new());
        let mods = ModManager::scan_directories_with_db(
            archive.clone(),
            active.clone(),
            2,
            &db,
            &|done, total| ticks.lock().unwrap().push((done, total)),
        )
        .unwrap();

        assert_eq!(mods.len(), 3);
        let ticks = ticks.lock().unwrap();
        // First tick announces the total before any work; every
        // later tick keeps that same total, processed counts
        // never go backwards, and the last tick lands exactly
        // on the total — the splash bar's contract.
        assert_eq!(ticks.first(), Some(&(0, 3)));
        assert!(ticks.iter().all(|&(_, total)| total == 3));
        assert!(ticks.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(ticks.last(), Some(&(3, 3)));

        std::fs::remove_dir_all(&base).ok();
    }
}
