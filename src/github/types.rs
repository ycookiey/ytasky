//! GitHub Search API (`GET /search/issues`) レスポンスの serde 型。
//!
//! 必要なフィールドのみ定義する。未知フィールドは無視される。

// TODO(github M3): import 実装でこれらの型を消費し始めたら除去する。
//   それまでは bin から未使用のため dead_code 警告を抑制する。
#![allow(dead_code)]

use serde::Deserialize;

/// `GET /search/issues` のレスポンス本体。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    /// マッチ総数（最大 1000 までしか items は返らない）。
    #[serde(default)]
    pub total_count: u64,
    /// rate limit 等で結果が不完全な場合 true。
    #[serde(default)]
    pub incomplete_results: bool,
    #[serde(default)]
    pub items: Vec<Issue>,
}

/// issue（または PR）1件。search/issues は PR も返すため `pull_request` の有無で区別する。
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    /// `https://api.github.com/repos/{owner}/{repo}` 形式。owner/repo 抽出に使う。
    pub repository_url: String,
    /// 期限。設定されていなければ `None`。
    #[serde(default)]
    pub milestone: Option<Milestone>,
    /// 存在すれば対象は issue ではなく PR。import 時に skip する（有無のみ判定し中身は使わない）。
    #[serde(default)]
    pub pull_request: Option<PullRequestRef>,
}

impl Issue {
    /// PR か否か。
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }

    /// `repository_url` から `(owner, repo)` を取り出す。
    /// 例: `https://api.github.com/repos/ycookiey/ytasky` → `("ycookiey", "ytasky")`。
    pub fn owner_repo(&self) -> Option<(&str, &str)> {
        let rest = self.repository_url.split("/repos/").nth(1)?;
        let mut parts = rest.split('/');
        let owner = parts.next()?;
        let repo = parts.next()?;
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some((owner, repo))
    }
}

/// milestone（期限の取得元）。
#[derive(Debug, Clone, Deserialize)]
pub struct Milestone {
    /// ISO 8601（例 `2026-06-15T07:00:00Z`）。未設定なら `None`。
    #[serde(default)]
    pub due_on: Option<String>,
}

/// PR 判定用のマーカー。`pull_request` フィールドの有無のみ意味を持ち、中身は参照しない。
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestRef {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_search_result() {
        let json = r#"{
            "total_count": 2,
            "incomplete_results": false,
            "items": [
                {
                    "number": 12,
                    "title": "バグ修正",
                    "repository_url": "https://api.github.com/repos/ycookiey/ytasky",
                    "milestone": { "due_on": "2026-06-15T07:00:00Z" }
                },
                {
                    "number": 13,
                    "title": "PR です",
                    "repository_url": "https://api.github.com/repos/ycookiey/ytasky",
                    "pull_request": { "url": "https://api.github.com/repos/ycookiey/ytasky/pulls/13" }
                }
            ]
        }"#;
        let parsed: SearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.total_count, 2);
        assert_eq!(parsed.items.len(), 2);

        let issue = &parsed.items[0];
        assert_eq!(issue.number, 12);
        assert!(!issue.is_pull_request());
        assert_eq!(issue.owner_repo(), Some(("ycookiey", "ytasky")));
        assert_eq!(
            issue.milestone.as_ref().and_then(|m| m.due_on.as_deref()),
            Some("2026-06-15T07:00:00Z")
        );

        let pr = &parsed.items[1];
        assert!(pr.is_pull_request());
    }

    #[test]
    fn issue_without_milestone() {
        let json = r#"{
            "number": 1,
            "title": "t",
            "repository_url": "https://api.github.com/repos/o/r"
        }"#;
        let issue: Issue = serde_json::from_str(json).unwrap();
        assert!(issue.milestone.is_none());
        assert!(!issue.is_pull_request());
        assert_eq!(issue.owner_repo(), Some(("o", "r")));
    }

    #[test]
    fn owner_repo_malformed() {
        let issue = Issue {
            number: 1,
            title: "t".into(),
            repository_url: "https://example.com/no-repos-segment".into(),
            milestone: None,
            pull_request: None,
        };
        assert_eq!(issue.owner_repo(), None);
    }
}
