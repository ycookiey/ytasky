//! Google Calendar 連携モジュール。
//!
//! 公開 API:
//! - `auth::get_valid_token`: 必要なら refresh して有効な access token を返す
//! - `auth::login`: OAuth フローを実行して token を取得する
//! - `auth::logout`: 保存済み token を削除する
//! - `api`: Calendar API v3 (events.list / events.instances / calendarList.list)
//! - `tz`: RFC3339 ↔ (NaiveDate, fixed_start_min)
//! - `types`: API レスポンスの serde 型
//! - [`load_config`]: `~/.config/ytasky/config.json` から設定を読む

use anyhow::Result;
use serde::Deserialize;

pub mod api;
pub mod auth;
pub mod import;
pub mod rrule;
pub mod types;
pub mod tz;

#[derive(Debug, Clone, Deserialize)]
pub struct GcalConfig {
    /// TUI 起動時にバックグラウンドで GCal を同期するか (default: true)
    #[serde(default = "default_true")]
    pub gcal_auto_sync: bool,
    /// auto sync の対象期間 (今日からの日数, default: 7)
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

/// `~/.config/ytasky/config.json` から設定を読む。無ければデフォルト。
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
