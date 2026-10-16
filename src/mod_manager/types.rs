use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use chrono::{DateTime, Utc};

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
}

impl Mod {
    pub fn new(path: PathBuf) -> Result<Self, std::io::Error> {
        let metadata = std::fs::metadata(&path)?;
        let size = metadata.len();
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

        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
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
        })
    }
}