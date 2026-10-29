//! serde types for the GitHub Search API (`GET /search/issues`) response.
//!
//! Only the needed fields are defined; unknown fields are ignored.

use serde::Deserialize;

/// Body of the `GET /search/issues` response.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    /// Total number of matches (items stop at 1000).
    #[serde(default)]
    pub total_count: u64,
    /// True when the results are incomplete, e.g. because of a rate limit.
    #[serde(default)]
    pub incomplete_results: bool,
    #[serde(default)]
    pub items: Vec<Issue>,
}

/// One issue or PR. search/issues also returns PRs, told apart by `pull_request`.
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    /// In the form `https://api.github.com/repos/{owner}/{repo}`; owner/repo come from it.
    pub repository_url: String,
    /// Source of the due date; `None` when not set.
    #[serde(default)]
    pub milestone: Option<Milestone>,
    /// Present only for PRs, which the import skips. Only its presence is checked.
    #[serde(default)]
    pub pull_request: Option<PullRequestRef>,
}

impl Issue {
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }

    /// Extracts `(owner, repo)` from `repository_url`.
    /// Example: `https://api.github.com/repos/owner/repo` -> `("owner", "repo")`.
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

/// Milestone, the source of the due date.
#[derive(Debug, Clone, Deserialize)]
pub struct Milestone {
    /// ISO 8601 (e.g. `2026-06-15T07:00:00Z`); `None` when not set.
    #[serde(default)]
    pub due_on: Option<String>,
}

/// Marker for PR detection: only the presence of the `pull_request` field matters, not its content.
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
