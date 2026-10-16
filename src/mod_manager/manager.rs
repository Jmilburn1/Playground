use super::types::{Mod, ModState};
use crate::config::Config;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
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
        mods.extend(Self::scan_directory(active_dir, ModState::Active, parallel)?);
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
