//! GitHub issue 連携モジュール。
//!
//! GitHub の検索クエリに一致する open issue を backlog タスクへ取り込む。
//! 認証は `gh` CLI のトークンを流用する（OAuth なし）。設計は `docs/github-import.md`。
//!
//! 公開 API（順次追加）:
//! - [`load_config`]: `~/.config/ytasky/config.json` から GitHub 設定を読む
//! - [`external_id`]: `github:{owner}/{repo}#{number}` 形式の external_id を生成
//! - `types`: Search API レスポンスの serde 型

// TODO(github M2): auth.rs（gh token 取得）, api.rs（search_events + Link pagination）を追加。
// TODO(github M3): import.rs（列所有権 upsert）, CLI import-github を追加。
//   それまで GithubConfig / load_config / external_id は bin から未使用のため警告を抑制する。
#![allow(dead_code)]

use anyhow::Result;
use serde::Deserialize;

pub mod api;
pub mod auth;
pub mod types;

/// 長大なレスポンス本文をエラーメッセージに乗せる際の安全な truncate。
// TODO(共通化): gcal にも同名の関数がある。M3 の sanitize_text 共通化とあわせて
//   src/text.rs 等へ統合する。
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}... ({} 文字省略)", s.chars().count() - max)
}

/// `~/.config/ytasky/config.json` の GitHub 関連設定。
/// GCal 設定（`GcalConfig`）と同じファイルを別構造体で部分デシリアライズする。
/// 注: `#[serde(deny_unknown_fields)]` を付けないこと（GCal のフィールドで失敗する）。
#[derive(Debug, Clone, Deserialize)]
pub struct GithubConfig {
    /// import 対象を表す GitHub 検索構文の配列（例: `"is:open assignee:@me"`）。
    #[serde(default)]
    pub github_import_queries: Vec<String>,
    /// TUI 起動時にバックグラウンドで import するか（default: true）。
    #[serde(default = "default_true")]
    pub github_auto_sync: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            github_import_queries: Vec::new(),
            github_auto_sync: true,
        }
    }
}

/// `~/.config/ytasky/config.json` から GitHub 設定を読む。無ければデフォルト。
pub fn load_config() -> Result<GithubConfig> {
    let path = crate::recurrence::config_dir()?.join("config.json");
    if !path.exists() {
        return Ok(GithubConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    let cfg: GithubConfig = serde_json::from_str(&raw).unwrap_or_default();
    Ok(cfg)
}

/// `github:{owner}/{repo}#{number}` 形式の external_id を生成する。
/// GCal の `gcal:` prefix と名前空間が分かれるため衝突しない。
pub fn external_id(owner: &str, repo: &str, number: u64) -> String {
    format!("github:{owner}/{repo}#{number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_id_format() {
        assert_eq!(
            external_id("ycookiey", "ytasky", 123),
            "github:ycookiey/ytasky#123"
        );
    }

    #[test]
    fn default_config() {
        let c = GithubConfig::default();
        assert!(c.github_import_queries.is_empty());
        assert!(c.github_auto_sync);
    }

    #[test]
    fn deserialize_partial_config() {
        let json = r#"{"github_import_queries": ["is:open assignee:@me"]}"#;
        let c: GithubConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.github_import_queries, vec!["is:open assignee:@me"]);
        // missing → default
        assert!(c.github_auto_sync);
    }

    #[test]
    fn deserialize_empty_object() {
        let c: GithubConfig = serde_json::from_str("{}").unwrap();
        assert!(c.github_import_queries.is_empty());
        assert!(c.github_auto_sync);
    }

    #[test]
    fn coexists_with_gcal_fields() {
        // GCal のフィールドが同居しても GitHub 設定の読み取りは成功する
        let json = r#"{
            "gcal_auto_sync": false,
            "gcal_auto_sync_days": 14,
            "github_import_queries": ["repo:o/r is:open"],
            "github_auto_sync": false
        }"#;
        let c: GithubConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.github_import_queries, vec!["repo:o/r is:open"]);
        assert!(!c.github_auto_sync);
    }
}
