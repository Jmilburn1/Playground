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

/// Decides when a streaming download reports its progress:
/// when the whole-percent figure steps (total known), when
/// ~150 ms have passed since the last report, and always at
/// completion. The UI's message loop hears real movement as
/// it happens without being flooded a message per chunk.
pub struct ProgressGate {
    last_report: std::time::Instant,
    last_percent: Option<u64>,
}

impl ProgressGate {
    pub fn new() -> Self {
        Self {
            // Backdated past one window, so the first chunk
            // reports at once: progress shows movement
            // immediately, not one throttle window in.
            last_report: std::time::Instant::now()
                - std::time::Duration::from_millis(200),
            last_percent: None,
        }
    }

    /// True when this (downloaded, total) sample should be
    /// passed to the caller's progress reporter.
    pub fn should_report(&mut self, downloaded: u64, total: Option<u64>) -> bool {
        let percent = total
            .filter(|t| *t > 0)
            .map(|t| downloaded.saturating_mul(100) / t);
        let complete = matches!(total, Some(t) if t > 0 && downloaded >= t);
        let stepped = match (percent, self.last_percent) {
            (Some(p), Some(last)) => p > last,
            (Some(_), None) => true,
            _ => false,
        };
        if complete
            || stepped
            || self.last_report.elapsed() >= std::time::Duration::from_millis(150)
        {
            self.last_report = std::time::Instant::now();
            if percent.is_some() {
                self.last_percent = percent;
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::ProgressGate;

    #[test]
    fn progress_gate_reports_first_sample_and_percent_steps() {
        let mut gate = ProgressGate::new();
        // First sample always reports (the gate starts
        // backdated one window).
        assert!(gate.should_report(0, Some(1000)));
        // Same whole percent, inside the window: quiet.
        assert!(!gate.should_report(5, Some(1000)));
        // A whole-percent step reports at once.
        assert!(gate.should_report(10, Some(1000)));
        assert!(!gate.should_report(15, Some(1000)));
        // Completion always reports.
        assert!(gate.should_report(1000, Some(1000)));
    }

    #[test]
    fn progress_gate_without_total_reports_first_then_throttles() {
        let mut gate = ProgressGate::new();
        assert!(gate.should_report(500, None));
        // No total, no time passed: the next chunk is quiet.
        assert!(!gate.should_report(900, None));
    }
}

pub trait ModSource: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, query: &str, game_version: &str) -> anyhow::Result<Vec<ModSearchResult>>;
    fn download(&self, mod_id: &str, dest: PathBuf) -> anyhow::Result<PathBuf>;
    fn get_mod_info(&self, mod_id: &str) -> anyhow::Result<ModSearchResult>;
    fn check_updates(&self, mod_id: &str, current_version: &str) -> anyhow::Result<Option<ModSearchResult>>;
}