use super::types::{Mod, ModState, ModType};
use crate::config::Config;
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
        })
    }

    pub fn scan_all(&mut self) -> Result<()> {
        let (archive_dir, active_dir, parallel) = self.scan_params();
        self.mods = Self::scan_directories(archive_dir, active_dir, parallel)?;
        Ok(())
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
        let mut mods = Self::scan_directory(archive_dir, ModState::Archived, parallel)?;
        let active = Self::scan_directory(active_dir, ModState::Active, parallel)?;
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
    }

    fn scan_directory(dir: PathBuf, state: ModState, parallel: usize) -> Result<HashMap<String, Mod>> {
        if !dir.exists() {
            return Ok(HashMap::new());
        }

        let paths: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
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
            .collect();

        let mods: Vec<Mod> = paths
            .par_iter()
            .with_min_len(parallel)
            .filter_map(|p| Mod::new(p.clone()).ok())
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

    /// The GitHub repo ("owner/repo") this mod is linked to for
    /// update checks, if any.
    pub fn mod_link(&self, stem: &str) -> Option<&str> {
        self.config.settings.mod_links.get(stem).map(|s| s.as_str())
    }

    /// Link (or, with None, unlink) a mod stem to a GitHub repo;
    /// persists to the config file.
    pub fn set_mod_link(&mut self, stem: &str, repo: Option<String>) -> Result<()> {
        match repo {
            Some(r) => {
                self.config.settings.mod_links.insert(stem.to_string(), r);
            }
            None => {
                self.config.settings.mod_links.remove(stem);
            }
        }
        self.config.save()
    }

    /// Every linked mod (stem -> repo), for "check all" runs.
    pub fn mod_links(&self) -> Vec<(String, String)> {
        let mut links: Vec<(String, String)> = self
            .config
            .settings
            .mod_links
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        links.sort();
        links
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

    /// Install an update: replace a ZIP mod's file with a
    /// downloaded one, in place (same folder, same filename —
    /// "overwrite the outdated file"). The download is validated
    /// as a readable mod BEFORE the old file is touched; the old
    /// mod's favorite key and loadout entries are migrated to the
    /// new file's key, so stars and save loadouts survive the
    /// version bump. Returns "old -> new" version text.
    pub fn replace_mod_file(&mut self, mod_id: &str, downloaded: &std::path::Path) -> Result<String> {
        let old = self
            .mods
            .get(mod_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        if old.mod_type != ModType::Zip {
            anyhow::bail!(
                "Updates install over .zip mods only — {} is a folder mod.",
                old.name
            );
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

        let old_key = old.favorite_key();
        let old_version = old.version.clone();

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
        let mut fresh = Mod::new(old.path.clone())
            .context("The updated file would not re-read after installing")?;
        fresh.state = old.state;
        let new_key = fresh.favorite_key();
        let new_version = fresh.version.clone();
        self.mods.insert(fresh.id.clone(), fresh);

        // Carry favorites and loadout entries to the new key.
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

        // The staged download is consumed; report the version
        // change for the caller's message.
        Ok(format!("{} -> {}", old_version, new_version))
    }

    /// The game profile folder — the parent of the active folder when
    /// that folder is the game's `mods` directory. Savegame slots
    /// live there, as siblings of `mods`.
    pub fn profile_dir(&self) -> Option<PathBuf> {
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
}
