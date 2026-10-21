pub mod mod_hub;
pub mod throttle;
pub mod github;
pub mod kingmods;
pub mod itch;
pub mod catalog;
pub mod catalog_other;
pub mod catalog_miss;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSearchResult {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub download_url: String,
    pub source: String,
    pub game_versions: Vec<String>,
    pub size: Option<u64>,
    pub rating: Option<f32>,
    pub downloads: Option<u64>,
    pub upload_date: Option<String>,
}

pub trait ModSource: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, query: &str, game_version: &str) -> anyhow::Result<Vec<ModSearchResult>>;
    fn download(&self, mod_id: &str, dest: PathBuf) -> anyhow::Result<PathBuf>;
    fn get_mod_info(&self, mod_id: &str) -> anyhow::Result<ModSearchResult>;
    fn check_updates(&self, mod_id: &str, current_version: &str) -> anyhow::Result<Option<ModSearchResult>>;
}