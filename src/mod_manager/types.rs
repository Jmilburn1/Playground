use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use chrono::{DateTime, Utc};

use super::moddesc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModType {
    Zip,
    Folder,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModState {
    Active,
    Archived,
    Conflict,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub mod_type: ModType,
    pub state: ModState,
    pub path: PathBuf,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub game_versions: Vec<String>,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<String>,
    pub source: Option<String>,
    pub download_url: Option<String>,
    pub hash: Option<String>,
    #[serde(default)]
    pub icon_filename: Option<String>,
}

impl Mod {
    pub fn new(path: PathBuf) -> Result<Self, std::io::Error> {
        let metadata = std::fs::metadata(&path)?;
        let size = if metadata.is_dir() {
            // metadata.len() on a directory is the directory entry, not the
            // mod's real size — sum the files inside instead.
            walkdir::WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        } else {
            metadata.len()
        };
        let modified_at = metadata.modified()
            .map(|t| t.into())
            .unwrap_or_else(|_| Utc::now());
        let created_at = metadata.created()
            .map(|t| t.into())
            .unwrap_or_else(|_| Utc::now());

        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mod_type = if path.is_dir() {
            ModType::Folder
        } else if path.extension().map_or(false, |e| e == "zip") {
            ModType::Zip
        } else {
            ModType::Unknown
        };

        // Stable ID: a hash of the canonical path. The same mod at the same
        // location gets the same ID on every scan — favorites and the CLI
        // both depend on that (a fresh random ID per scan breaks both).
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string_lossy().as_bytes());
        let id = hex::encode(hasher.finalize());

        let mut m = Self {
            id,
            name,
            version: "1.0.0".to_string(),
            author: None,
            description: None,
            mod_type,
            state: ModState::Active,
            path,
            size,
            created_at,
            modified_at,
            game_versions: vec![],
            dependencies: vec![],
            conflicts: vec![],
            source: None,
            download_url: None,
            hash: None,
            icon_filename: None,
        };

        // Real metadata from modDesc.xml, when the mod has one. Missing or
        // unreadable data just leaves the filename-based defaults in place.
        if let Some(info) = moddesc::read_metadata(&m.path) {
            if let Some(title) = info.title {
                if !title.trim().is_empty() {
                    m.name = title;
                }
            }
            if let Some(version) = info.version {
                m.version = version;
            }
            if let Some(author) = info.author {
                m.author = Some(author);
            }
            if let Some(description) = info.description {
                m.description = Some(description);
            }
            m.icon_filename = info.icon_filename;
        }

        Ok(m)
    }

    /// Key used to remember this mod as a favorite. Deliberately NOT the
    /// path-based `id`: moving a mod between archive and active changes its
    /// path (and therefore its id), but filename + size + version survive
    /// the move, so the favorite does too.
    pub fn favorite_key(&self) -> String {
        let file_name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.name)
            .to_string();
        format!("{}|{}|{}", file_name, self.size, self.version)
    }
}