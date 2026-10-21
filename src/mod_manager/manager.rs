use super::types::{Mod, ModState, ModType};
use crate::config::Config;
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
        })
    }

    pub fn scan_all(&mut self) -> Result<()> {
        let (archive_dir, active_dir, parallel) = self.scan_params();
        self.mods = Self::scan_directories(archive_dir, active_dir, parallel)?;
        self.migrate_other_links();
        Ok(())
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
            self.config.settings.parallel_scans,
        )
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
        let archive_paths = Self::enumerate_candidates(&archive_dir);
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
        // One library, two doors: the active folder is a hardlink
        // projection of the archive, so the same file in both
        // folders is ONE mod wearing two paths (each copy hashes
        // its own path into a distinct id, so both loaded). The
        // twin test is the favorite-key test — EXACT file name
        // (characters and case included), size and version: a
        // match is the same file, so load the active copy and
        // ignore its archive twin. Same name but different size
        // or version means the copies diverged: both load, and
        // the conflict detector reports them. Keying on the
        // modDesc TITLE instead would swallow different mods
        // that share a title (John's Bale Storage family,
        // 2026-10-19).
        let active_files: HashSet<(String, u64, String)> = active
            .values()
            .map(|m| (m.file_name(), m.size, m.version.clone()))
            .collect();
        mods.retain(|_, m| {
            !active_files.contains(&(m.file_name(), m.size, m.version.clone()))
        });
        mods.extend(active);
        Ok(mods)
    }

    /// Replace the loaded mods with the result of a background scan.
    pub fn install_mods(&mut self, mods: HashMap<String, Mod>) {
        self.mods = mods;
        self.migrate_other_links();
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

    pub fn move_to_archive(&mut self, mod_id: &str) -> Result<()> {
        let mod_data = self.mods.get_mut(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        
        let source_path = mod_data.path.clone();
        let dest_path = self.archive_dir.join(source_path.file_name().unwrap());

        move_path(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Archived;

        Ok(())
    }

    pub fn move_to_active(&mut self, mod_id: &str) -> Result<()> {
        let mod_data = self.mods.get_mut(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        
        let source_path = mod_data.path.clone();
        let dest_path = self.active_dir.join(source_path.file_name().unwrap());

        move_path(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Active;

        Ok(())
    }

    pub fn move_between_directories(&mut self, mod_id: &str, target_dir: &PathBuf) -> Result<()> {
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

    /// The remembered game install folder (Settings tab), where
    /// game.xml lives. None until the user picks it.
    pub fn game_directory(&self) -> Option<PathBuf> {
        self.config.settings.game_directory.clone()
    }

    /// Remember the game install folder; persists to the config.
    pub fn set_game_directory(&mut self, dir: PathBuf) -> Result<()> {
        let dir = crate::config::expand_tilde(&dir);
        self.config.settings.game_directory = Some(dir);
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
        
        if mod_data.path.exists() {
            if mod_data.path.is_dir() {
                std::fs::remove_dir_all(&mod_data.path)?;
            } else {
                std::fs::remove_file(&mod_data.path)?;
            }
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
        match old.mod_type {
            ModType::Folder => return self.replace_folder_mod(&old, downloaded),
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
        self.finish_replace(&old, fresh)
    }

    /// The folder-mod half of `replace_mod_file`: the
    /// download (a zip, as every source serves) replaces the
    /// folder's whole contents. The old folder is renamed
    /// aside first and only deleted once the new contents
    /// re-read as a mod — an unpack or parse failure puts
    /// the old folder back exactly as it was.
    fn replace_folder_mod(&mut self, old: &Mod, downloaded: &std::path::Path) -> Result<String> {
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
                self.finish_replace(old, fresh)
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
    fn finish_replace(&mut self, old: &Mod, mut fresh: Mod) -> Result<String> {
        fresh.state = old.state.clone();
        let old_key = old.favorite_key();
        let old_version = old.version.clone();
        let new_key = fresh.favorite_key();
        let new_version = fresh.version.clone();
        self.mods.insert(fresh.id.clone(), fresh);

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
                auto_update: false,
                check_conflicts: true,
                parallel_scans: 2,
                cache_ttl_hours: 24,
                favorites: vec![],
                save_loadouts: HashMap::new(),
                game_directory: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: String::new(),
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
                auto_update: false,
                check_conflicts: true,
                parallel_scans: 2,
                cache_ttl_hours: 24,
                favorites: vec![],
                save_loadouts: HashMap::new(),
                game_directory: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: String::new(),
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
