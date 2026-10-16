use super::types::{Mod, ModState};
use crate::config::Config;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ModManager {
    config: Config,
    mods: HashMap<String, Mod>,
    archive_dir: PathBuf,
    active_dir: PathBuf,
}

impl ModManager {
    pub fn new(config: Config) -> Result<Self> {
        let archive_dir = config.settings.archive_directory.clone();
        let active_dir = config.settings.active_directory.clone();

        // Ensure directories exist
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create archive directory: {:?}", archive_dir))?;
        std::fs::create_dir_all(&active_dir)
            .with_context(|| format!("Failed to create active directory: {:?}", active_dir))?;

        let mut manager = Self {
            config,
            mods: HashMap::new(),
            archive_dir,
            active_dir,
        };

        // Initial scan
        manager.scan_all()?;

        Ok(manager)
    }

    pub fn scan_all(&mut self) -> Result<()> {
        let archive_dir = self.archive_dir.clone();
        let active_dir = self.active_dir.clone();
        let parallel = self.config.settings.parallel_scans;

        let archive_mods = Self::scan_directory(archive_dir, ModState::Archived, parallel)?;
        let active_mods = Self::scan_directory(active_dir, ModState::Active, parallel)?;

        self.mods.clear();
        self.mods.extend(archive_mods);
        self.mods.extend(active_mods);

        Ok(())
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

        std::fs::rename(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Archived;

        Ok(())
    }

    pub fn move_to_active(&mut self, mod_id: &str) -> Result<()> {
        let mod_data = self.mods.get_mut(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        
        let source_path = mod_data.path.clone();
        let dest_path = self.active_dir.join(source_path.file_name().unwrap());

        std::fs::rename(&source_path, &dest_path)?;

        mod_data.path = dest_path;
        mod_data.state = ModState::Active;

        Ok(())
    }

    pub fn move_between_directories(&mut self, mod_id: &str, target_dir: &PathBuf) -> Result<()> {
        let mod_data = self.mods.get_mut(mod_id)
            .ok_or_else(|| anyhow::anyhow!("Mod not found: {}", mod_id))?;
        
        let source_path = mod_data.path.clone();
        let dest_path = target_dir.join(source_path.file_name().unwrap());

        std::fs::rename(&source_path, &dest_path)?;

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
    /// Expands a leading `~`, creates the folders if needed, persists
    /// them to the config file, and rescans.
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
        self.config.save()?;
        self.scan_all()
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
}