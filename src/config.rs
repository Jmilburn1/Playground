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
    pub auto_update: bool,
    pub check_conflicts: bool,
    pub parallel_scans: usize,
    pub cache_ttl_hours: u64,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
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
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
        let config_dir = home.join(".config").join("fs-mod-manager");
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
                auto_update: true,
                check_conflicts: true,
                parallel_scans: 4,
                cache_ttl_hours: 24,
            },
        }
    }
}