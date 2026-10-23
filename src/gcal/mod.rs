//! Google Calendar import.

use anyhow::Result;
use serde::Deserialize;

pub mod api;
pub mod auth;
pub mod import;
pub mod rrule;
pub mod types;
pub mod tz;

/// Truncates a long response body before it goes into an error message.
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}... ({} 文字省略)", s.chars().count() - max)
}

#[derive(Debug, Clone, Deserialize)]
pub struct GcalConfig {
    /// Import in the background when the TUI starts.
    #[serde(default = "default_true")]
    pub gcal_auto_sync: bool,
    /// Number of days from today that the startup import covers.
    #[serde(default = "default_auto_days")]
    pub gcal_auto_sync_days: u32,
}

fn default_true() -> bool {
    true
}

fn default_auto_days() -> u32 {
    7
}

impl Default for GcalConfig {
    fn default() -> Self {
        Self {
            gcal_auto_sync: true,
            gcal_auto_sync_days: 7,
        }
    }
}

/// Reads the Google Calendar keys of `config.json` in the config dir; defaults when the file is missing.
pub fn load_config() -> Result<GcalConfig> {
    let path = crate::recurrence::config_dir()?.join("config.json");
    if !path.exists() {
        return Ok(GcalConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    let cfg: GcalConfig = serde_json::from_str(&raw).unwrap_or_default();
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = GcalConfig::default();
        assert!(c.gcal_auto_sync);
        assert_eq!(c.gcal_auto_sync_days, 7);
    }

    #[test]
    fn deserialize_partial_config() {
        let json = r#"{"gcal_auto_sync": false}"#;
        let c: GcalConfig = serde_json::from_str(json).unwrap();
        assert!(!c.gcal_auto_sync);
        // missing field → default
        assert_eq!(c.gcal_auto_sync_days, 7);
    }

    #[test]
    fn deserialize_empty_object() {
        let json = "{}";
        let c: GcalConfig = serde_json::from_str(json).unwrap();
        assert!(c.gcal_auto_sync);
        assert_eq!(c.gcal_auto_sync_days, 7);
    }
}
