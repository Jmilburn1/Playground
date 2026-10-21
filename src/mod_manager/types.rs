use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
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
    /// Player-facing category buckets (Tractors, Maps, Scripts, ...)
    /// detected from the mod's store items at scan time.
    #[serde(default)]
    pub categories: Vec<String>,
}

/// A mod's stable ID: a hash of its canonical path. The same
/// mod at the same location gets the same ID on every scan —
/// favorites and the CLI both depend on that (a fresh random
/// ID per scan breaks both). Shared with the library
/// database, which re-derives the id when a remembered mod
/// is served at a path (a hardlink twin in the other folder,
/// or a file moved between folders).
pub fn id_for_path(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())
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

        // Stable ID: a hash of the canonical path (see
        // `id_for_path` — the library database recomputes the
        // same id when it serves a remembered mod at a path).
        let id = id_for_path(&path);

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
            categories: Vec::new(),
        };

        // Real metadata from modDesc.xml, when the mod has one. Missing or
        // unreadable data just leaves the filename-based defaults in place.
        if let Some(info) = moddesc::read_metadata(&m.path) {
            // Categories first: the fields below are moved out of
            // `info` one by one, and detection still needs to read it.
            m.categories = super::categories::detect_categories(&m.path, Some(&info));
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
        } else {
            m.categories = vec![super::categories::UNCATEGORIZED.to_string()];
        }

        // A .dlc pack's category is assigned by file kind
        // (owner ruling, 2026-10-21): GIANTS' DLC files are
        // encrypted, so there is never a modDesc to detect
        // from — the record carries the DLC's category
        // instead of the no-category fallback. (This also
        // overrides the modDesc branch defensively: a .dlc
        // never yields one, but the kind rules regardless.)
        if m.path
            .extension()
            .map_or(false, |e| e.eq_ignore_ascii_case("dlc"))
        {
            m.categories = vec![super::categories::DLC_CATEGORY.to_string()];
        }

        Ok(m)
    }

    /// Key used to remember this mod as a favorite. Deliberately NOT the
    /// path-based `id`: moving a mod between archive and active changes its
    /// path (and therefore its id), but filename + size + version survive
    /// the move, so the favorite does too.
    pub fn favorite_key(&self) -> String {
        format!("{}|{}|{}", self.file_name(), self.size, self.version)
    }

    /// The mod's identity: its exact file name on disk, every
    /// character and case included. `name` is the modDesc
    /// TITLE — a display label different mods freely share
    /// (whole families ship as "Bale Storage") — so anything
    /// that decides whether two entries are the same mod
    /// (duplicate detection, the scan's twin collapse) keys
    /// on this, never on the title (John, 2026-10-19).
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.name)
            .to_string()
    }
}