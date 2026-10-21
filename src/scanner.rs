use crate::mod_manager::library_db::LibraryDb;
use crate::mod_manager::types::Mod;
use anyhow::Result;
use std::path::PathBuf;

pub struct ModScanner;

impl ModScanner {
    pub fn scan_directory(dir: PathBuf) -> Result<Vec<Mod>> {
        let mut mods = Vec::new();

        if !dir.exists() {
            return Ok(mods);
        }

        // Remembered mods come from the library database;
        // new or changed ones are parsed and recorded.
        let db = LibraryDb::load_default();
        let entries = std::fs::read_dir(&dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() || path.is_dir() {
                if let Some(mod_data) = db.get_or_parse(&path) {
                    mods.push(mod_data);
                }
            }
        }
        db.save();

        Ok(mods)
    }

    pub fn scan_directory_parallel(dir: PathBuf, parallel: usize) -> Result<Vec<Mod>> {
        use rayon::prelude::*;

        if !dir.exists() {
            return Ok(Vec::new());
        }

        let paths: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() || p.is_dir())
            .collect();

        let db = LibraryDb::load_default();
        let mods: Vec<Mod> = paths
            .par_iter()
            .with_min_len(parallel)
            .filter_map(|p| db.get_or_parse(p))
            .collect();
        db.save();

        Ok(mods)
    }
}
