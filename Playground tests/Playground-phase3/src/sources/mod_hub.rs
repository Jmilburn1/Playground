use crate::sources::{ModSource, ModSearchResult};
use std::path::PathBuf;

pub struct ModHubSource;

impl ModHubSource {
    pub fn new() -> Self {
        Self
    }
}

impl ModSource for ModHubSource {
    fn name(&self) -> &str {
        "Mod Hub"
    }

    fn search(&self, _query: &str, _game_version: &str) -> anyhow::Result<Vec<ModSearchResult>> {
        // TODO: Implement actual API call
        Ok(Vec::new())
    }

    fn download(&self, _mod_id: &str, _dest: PathBuf) -> anyhow::Result<PathBuf> {
        // TODO: Implement actual download
        Err(anyhow::anyhow!("Not implemented"))
    }

    fn get_mod_info(&self, _mod_id: &str) -> anyhow::Result<ModSearchResult> {
        // TODO: Implement actual API call
        Err(anyhow::anyhow!("Not implemented"))
    }

    fn check_updates(&self, _mod_id: &str, _current_version: &str) -> anyhow::Result<Option<ModSearchResult>> {
        // TODO: Implement actual check
        Ok(None)
    }
}