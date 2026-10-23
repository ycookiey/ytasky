//! GitHub issue import.
//!
//! Imports open issues matching GitHub search queries as backlog tasks.
//! Authentication reuses the token of the `gh` CLI, so ytasky needs no OAuth flow.

use anyhow::Result;
use serde::Deserialize;

pub mod api;
pub mod auth;
pub mod import;
pub mod types;

/// Truncates a long response body before it goes into an error message.
// TODO: identical to gcal::truncate_for_log; merge both into src/text.rs.
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}... ({} 文字省略)", s.chars().count() - max)
}

/// GitHub settings in `config.json` in the config dir.
/// The same file also holds `GcalConfig`; each struct deserializes only its own fields.
/// Do not add `#[serde(deny_unknown_fields)]`: the GCal fields would make parsing fail.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubConfig {
    /// GitHub search queries to import (e.g. `"is:open assignee:@me"`).
    #[serde(default)]
    pub github_import_queries: Vec<String>,
    /// Whether the TUI imports in the background at startup.
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

/// Reads the GitHub settings from `config.json`; defaults when the file is missing.
/// A JSON parse error is returned instead of falling back to the defaults, which would
/// turn a typo into "no queries" and hide the cause.
pub fn load_config() -> Result<GithubConfig> {
    use anyhow::Context;
    let path = crate::recurrence::config_dir()?.join("config.json");
    if !path.exists() {
        return Ok(GithubConfig::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("config.json 読込失敗: {}", path.display()))?;
    let cfg: GithubConfig = serde_json::from_str(&raw)
        .with_context(|| format!("config.json パース失敗: {}", path.display()))?;
    Ok(cfg)
}

/// Builds the external_id `github:{owner}/{repo}#{number}`.
/// The `github:` prefix keeps these ids apart from the `gcal:` ones.
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
        // GCal fields in the same file do not break reading the GitHub settings
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
