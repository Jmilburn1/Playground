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
    /// Settings Round 2 (owner, 2026-10-22): run the app's
    /// update check (the ModHub detection pass) as part of
    /// starting up. A user-pressed Rescan checks either way.
    #[serde(default = "default_true")]
    pub startup_update_check: bool,
    /// Settings Round 2: the app self-update's launch check
    /// (v1.5.0) may be switched off here. A manual "Check
    /// now" on the Settings page works either way.
    #[serde(default = "default_true")]
    pub self_update_enabled: bool,
    /// Settings Round 2: the Browse view a fresh launch
    /// opens with, stored as a key (see
    /// `browse_view_key` / the App's parsing): "all",
    /// "active", "archived", "favorites", "updates", or
    /// "category:<name>".
    #[serde(default = "default_browse_default_view")]
    pub browse_default_view: String,
    /// Settings Round 2: start the main window maximized
    /// (applied at the boot handover, once the ordinary
    /// window shape has landed).
    #[serde(default)]
    pub start_maximized: bool,
    /// Settings Round 2 (Library): when a download or
    /// install completes into a laid-out library, file the
    /// mod into its category home (and fix its projection)
    /// automatically.
    #[serde(default)]
    pub auto_file_new_mods: bool,
    /// Settings Round 2 (Maintenance): keep the Downloads
    /// page's finished history across restarts (stored as
    /// downloads-history.json in the data folder).
    #[serde(default = "default_true")]
    pub keep_download_history: bool,
    /// Settings Round 2 (Maintenance): how many scheduled-
    /// fill log files to keep in the data folder's logs/
    /// directory; older fills' logs are pruned after a fill.
    #[serde(default = "default_fill_log_keep")]
    pub fill_log_keep: usize,
    /// Settings Round 2 (Diagnostics): the Log Reader opens
    /// with its errors/warnings-only filter already on.
    #[serde(default)]
    pub log_errors_only_default: bool,
    /// Settings Round 2 (Diagnostics): how many trailing
    /// log.txt lines the Log Reader keeps for display.
    #[serde(default = "default_log_lines_cap")]
    pub log_lines_cap: usize,
    /// Settings Round 2 (General): the Launch Configuration
    /// — the game-start settings Playground keeps and the
    /// command they compose. Playground never runs the
    /// command (owner ruling, 2026-10-22: no launch button).
    #[serde(default)]
    pub launch: LaunchConfig,
}

/// Serde default for Settings::catalog_url.
fn default_catalog_url() -> String {
    crate::sources::catalog::DEFAULT_CATALOG_URL.to_string()
}

/// Serde default for the Round 2 switches that ship on.
fn default_true() -> bool {
    true
}

/// Serde default for Settings::browse_default_view.
fn default_browse_default_view() -> String {
    "all".to_string()
}

/// Serde default for Settings::fill_log_keep.
fn default_fill_log_keep() -> usize {
    10
}

/// Serde default for Settings::log_lines_cap.
fn default_log_lines_cap() -> usize {
    3000
}

/// The Launch Configuration (Settings, General — owner,
/// 2026-10-22): the game-start options Playground keeps,
/// in MT's shape but Playground's alone. The flags compose
/// the Launch Command Preview (see `launch_command_preview`);
/// nothing anywhere executes that command — there is no
/// launch button. The Developer Console is NOT here: it
/// acts through game.xml, and the page stages it beside
/// these (see ModManager's settings-page save).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// Savegame slot to auto-start (1–20); None = unset.
    #[serde(default)]
    pub slot: Option<u8>,
    /// -skipModUpdateDialog
    #[serde(default)]
    pub skip_mod_update_dialog: bool,
    /// -cheats
    #[serde(default)]
    pub cheats: bool,
    /// -autostart
    #[serde(default)]
    pub autostart: bool,
    /// -skipStartVideos
    #[serde(default)]
    pub skip_start_videos: bool,
}

/// The Launch Command Preview: the game exe name plus the
/// flags for the options that are set, in the owner's
/// reference order — cheats, skip dialog, skip videos,
/// autostart, savegame slot. A slot outside 1–20 composes
/// nothing (the page refuses to save one; this keeps a
/// hand-edited config honest too).
pub fn launch_command_preview(launch: &LaunchConfig) -> String {
    let mut cmd = String::from("FarmingSimulator2025.exe");
    if launch.cheats {
        cmd.push_str(" -cheats");
    }
    if launch.skip_mod_update_dialog {
        cmd.push_str(" -skipModUpdateDialog");
    }
    if launch.skip_start_videos {
        cmd.push_str(" -skipStartVideos");
    }
    if launch.autostart {
        cmd.push_str(" -autostart");
    }
    if let Some(slot) = launch.slot {
        if (1..=20).contains(&slot) {
            cmd.push_str(&format!(" -autoStartSavegameId {}", slot));
        }
    }
    cmd
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
                startup_update_check: true,
                self_update_enabled: true,
                browse_default_view: "all".to_string(),
                start_maximized: false,
                auto_file_new_mods: false,
                keep_download_history: true,
                fill_log_keep: 10,
                log_errors_only_default: false,
                log_lines_cap: 3000,
                launch: LaunchConfig::default(),
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
    fn round2_keys_default_when_absent() {
        // A config.toml written before Round 2 carries none
        // of the new keys; every one must land on its ruled
        // default so the old file loads untouched.
        let settings: Settings = toml::from_str(
            r#"
archive_directory = "/tmp/archive"
active_directory = "/tmp/active"
check_conflicts = true
parallel_scans = 2
cache_ttl_hours = 24
"#,
        )
        .unwrap();
        assert!(settings.startup_update_check);
        assert!(settings.self_update_enabled);
        assert_eq!(settings.browse_default_view, "all");
        assert!(!settings.start_maximized);
        assert!(!settings.auto_file_new_mods);
        assert!(settings.keep_download_history);
        assert_eq!(settings.fill_log_keep, 10);
        assert!(!settings.log_errors_only_default);
        assert_eq!(settings.log_lines_cap, 3000);
        assert_eq!(settings.launch.slot, None);
        assert!(!settings.launch.cheats);
        assert!(!settings.launch.skip_mod_update_dialog);
        assert!(!settings.launch.autostart);
        assert!(!settings.launch.skip_start_videos);
    }

    #[test]
    fn launch_preview_composes_only_what_is_set() {
        let bare = LaunchConfig::default();
        assert_eq!(launch_command_preview(&bare), "FarmingSimulator2025.exe");

        let mut some = LaunchConfig::default();
        some.cheats = true;
        some.skip_start_videos = true;
        assert_eq!(
            launch_command_preview(&some),
            "FarmingSimulator2025.exe -cheats -skipStartVideos"
        );

        // The owner's reference readout, fully loaded:
        // flags in the ruled order, slot last.
        let full = LaunchConfig {
            slot: Some(3),
            skip_mod_update_dialog: true,
            cheats: true,
            autostart: true,
            skip_start_videos: true,
        };
        assert_eq!(
            launch_command_preview(&full),
            "FarmingSimulator2025.exe -cheats -skipModUpdateDialog \
             -skipStartVideos -autostart -autoStartSavegameId 3"
        );

        // An out-of-range slot (hand-edited config) composes
        // no flag at all.
        let mut bad = LaunchConfig::default();
        bad.slot = Some(0);
        assert_eq!(launch_command_preview(&bad), "FarmingSimulator2025.exe");
        bad.slot = Some(21);
        assert_eq!(launch_command_preview(&bad), "FarmingSimulator2025.exe");
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