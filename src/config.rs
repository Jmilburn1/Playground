use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub game_directories: GameDirectories,
    pub mod_sources: ModSources,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameDirectories {
    pub fs17: Option<PathBuf>,
    pub fs19: Option<PathBuf>,
    pub fs22: Option<PathBuf>,
    pub fs25: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSources {
    pub mod_hub: Vec<SourceConfig>,
    pub github: Vec<SourceConfig>,
    pub kingmods: Vec<SourceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub archive_directory: PathBuf,
    pub active_directory: PathBuf,
    pub check_conflicts: bool,
    pub parallel_scans: usize,
    pub cache_ttl_hours: u64,
    /// Favorite mods, keyed by Mod::favorite_key (filename|size|version) so a
    /// favorite survives the mod being moved between archive and active.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Per-savegame mod loadouts (Configure Save): savegame folder
    /// name ("savegame1") -> favorite_keys of the mods that save uses.
    #[serde(default)]
    pub save_loadouts: std::collections::HashMap<String, Vec<String>>,
    /// The game's install folder (where game.xml lives), picked in
    /// the Settings tab. Powers the console on/off toggle.
    #[serde(default)]
    pub game_directory: Option<PathBuf>,
    /// Update-check links (GitHub): mod file stem (lowercase, no
    /// .zip) -> "owner/repo". Set per mod in the details panel;
    /// keyed by stem so the link survives moves and version bumps.
    #[serde(default)]
    pub mod_links: std::collections::HashMap<String, String>,
    /// Update-check links (GIANTS ModHub): mod file stem ->
    /// ModHub mod_id (digits). Same keying as the GitHub links.
    #[serde(default)]
    pub modhub_links: std::collections::HashMap<String, String>,
    /// The published ModHub catalog the app reads update and
    /// link answers from (see sources::catalog). Defaults to
    /// the production catalog; empty disables fetching (a
    /// previously downloaded cache is still used).
    #[serde(default = "default_catalog_url")]
    pub catalog_url: String,
}

/// Serde default for Settings::catalog_url.
fn default_catalog_url() -> String {
    crate::sources::catalog::DEFAULT_CATALOG_URL.to_string()
}

/// Clamp the scan-parallelism setting to the range the
/// Settings page offers: at least one scan worker, and no
/// more than sixteen.
pub fn clamp_parallel_scans(value: usize) -> usize {
    value.clamp(1, 16)
}

/// Clamp the cache lifetime setting: a cache must be
/// allowed at least one hour before it can go stale.
pub fn clamp_cache_ttl_hours(value: u64) -> u64 {
    value.max(1)
}

/// The one staleness decision every cache in the app
/// shares (the downloaded catalog and the decoded-icon
/// cache): an entry whose age has reached its TTL is
/// stale. The TTL is clamped to at least an hour, so a
/// zero in a hand-edited config cannot make every entry
/// permanently stale.
pub fn is_stale(age_secs: u64, ttl_hours: u64) -> bool {
    age_secs >= clamp_cache_ttl_hours(ttl_hours).saturating_mul(3600)
}

/// Expand a leading `~` in a configured path. Configs written by other
/// tools sometimes carry the tilde literally, which otherwise becomes a
/// real folder named "~" under whatever directory the app launched from.
pub fn expand_tilde(path: &PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| path.clone());
    }
    for prefix in ["~/", "~\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        }
    }
    path.clone()
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let mut config: Config = toml::from_str(&content)?;
            config.settings.archive_directory =
                expand_tilde(&config.settings.archive_directory);
            config.settings.active_directory =
                expand_tilde(&config.settings.active_directory);
            for dir in [
                &mut config.game_directories.fs17,
                &mut config.game_directories.fs19,
                &mut config.game_directories.fs22,
                &mut config.game_directories.fs25,
            ] {
                if let Some(d) = dir {
                    *d = expand_tilde(d);
                }
            }
            if let Some(d) = &mut config.settings.game_directory {
                *d = expand_tilde(d);
            }
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path()?;
        let content = toml::to_string(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }

    fn config_path() -> anyhow::Result<PathBuf> {
        let config_dir = crate::paths::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("config.toml"))
    }

    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        
        Config {
            game_directories: GameDirectories {
                fs17: None,
                fs19: None,
                fs22: None,
                fs25: None,
            },
            mod_sources: ModSources {
                mod_hub: vec![SourceConfig {
                    name: "Mod Hub".to_string(),
                    url: "https://modhub.farming-simulator.com".to_string(),
                    enabled: true,
                    api_key: Some("".to_string()),
                }],
                github: vec![SourceConfig {
                    name: "GitHub".to_string(),
                    url: "https://api.github.com".to_string(),
                    enabled: true,
                    api_key: Some("".to_string()),
                }],
                kingmods: vec![SourceConfig {
                    name: "KingMods".to_string(),
                    url: "https://www.kingmods.com".to_string(),
                    enabled: true,
                    api_key: Some("".to_string()),
                }],
            },
            settings: Settings {
                archive_directory: home.join("FSMods").join("Archive"),
                active_directory: home.join("FSMods").join("Active"),
                check_conflicts: true,
                parallel_scans: 4,
                cache_ttl_hours: 24,
                favorites: Vec::new(),
                save_loadouts: std::collections::HashMap::new(),
                game_directory: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: crate::sources::catalog::DEFAULT_CATALOG_URL.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_auto_update_key_is_ignored() {
        // Config files written before the dead `auto_update`
        // flag was deleted carry the key; serde's default
        // unknown-field behavior must keep them loading.
        let settings: Settings = toml::from_str(
            r#"
archive_directory = "/tmp/archive"
active_directory = "/tmp/active"
auto_update = true
check_conflicts = true
parallel_scans = 2
cache_ttl_hours = 24
"#,
        )
        .unwrap();
        assert!(settings.check_conflicts);
        assert_eq!(settings.parallel_scans, 2);
        assert_eq!(settings.cache_ttl_hours, 24);
    }

    #[test]
    fn settings_clamps_hold_their_ranges() {
        assert_eq!(clamp_parallel_scans(0), 1);
        assert_eq!(clamp_parallel_scans(4), 4);
        assert_eq!(clamp_parallel_scans(99), 16);
        assert_eq!(clamp_cache_ttl_hours(0), 1);
        assert_eq!(clamp_cache_ttl_hours(24), 24);
    }

    #[test]
    fn staleness_turns_at_the_ttl_boundary() {
        assert!(!is_stale(0, 1));
        assert!(!is_stale(3599, 1));
        assert!(is_stale(3600, 1));
        assert!(!is_stale(86_399, 24));
        assert!(is_stale(86_400, 24));
        // A zero TTL clamps to one hour rather than making
        // everything stale at age zero.
        assert!(!is_stale(0, 0));
        assert!(is_stale(3600, 0));
    }
}