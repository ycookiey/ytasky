//! Imports GitHub issues as ytasky backlog tasks.
//!
//! - Runs search/issues per query and imports the open issues as backlog tasks
//! - Skips PRs; an issue found by several queries is deduplicated by external_id
//! - Column ownership: a new task gets every column; an existing task gets only title/deadline
//!   updated, so the columns the user sets (category, duration, placement) are kept

use std::collections::HashSet;

use anyhow::Result;
use ybasey::Database;

use crate::github::auth;
use crate::github::types::Issue;
use crate::github::{api, external_id};
use crate::text::sanitize_text;

/// Default duration in minutes, since an issue has none.
const DEFAULT_DURATION_MIN: i32 = 30;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Default category for newly inserted tasks (not applied to existing tasks).
    pub category_id: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            category_id: "3".into(), // 3 = the seeded work category
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    /// Number of tasks set to `status="done"` because their issue was closed upstream.
    pub closed_done: usize,
    pub errors: Vec<String>,
}

/// Imports the open issues matching the queries, and marks existing backlog GitHub tasks
/// done when their issue was closed upstream.
pub fn import_queries(
    db: &mut Database,
    queries: &[String],
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    let token = auth::get_token()?;
    let mut summary = ImportSummary::default();
    let mut seen: HashSet<String> = HashSet::new();

    // tz for converting milestone.due_on to a local date (the tz: line of data_dir/_meta)
    let tz = crate::db::data_dir()
        .ok()
        .map(|d| read_meta_tz(&d))
        .unwrap_or(chrono_tz::UTC);

    for query in queries {
        // Collect a failing query into errors instead of `?`, so it does not stop the rest:
        // tasks inserted by earlier queries stay, and detect_closed still runs.
        match api::search_issues(&token, query) {
            Ok(outcome) => {
                // Collect non-fatal warnings (over 1000 results, incomplete_results) into errors,
                // since eprintln would corrupt the TUI's alternate screen.
                summary.errors.extend(outcome.warnings);
                for issue in outcome.issues {
                    if let Err(e) = handle_issue(db, &issue, opts, tz, &mut seen, &mut summary) {
                        summary
                            .errors
                            .push(format!("issue #{} の取り込み失敗: {e}", issue.number));
                    }
                }
            }
            Err(e) => {
                summary
                    .errors
                    .push(format!("query \"{query}\" 失敗: {e}"));
            }
        }
    }

    // Backlog GitHub tasks missing from `seen` may have been closed upstream.
    // Check each one with the issue API and mark only the closed ones done.
    let detection = detect_closed(db, &token, &seen)?;
    for (task_id, ext) in &detection.closed {
        match crate::db::set_task_status_done(db, *task_id) {
            Ok(()) => summary.closed_done += 1,
            Err(e) => summary
                .errors
                .push(format!("done 遷移失敗 {ext}: {e}")),
        }
    }
    summary.errors.extend(detection.errors);

    Ok(summary)
}

fn handle_issue(
    db: &mut Database,
    issue: &Issue,
    opts: &ImportOptions,
    tz: chrono_tz::Tz,
    seen: &mut HashSet<String>,
    summary: &mut ImportSummary,
) -> Result<()> {
    if issue.is_pull_request() {
        summary.skipped += 1;
        return Ok(());
    }
    let Some((owner, repo)) = issue.owner_repo() else {
        summary.skipped += 1;
        return Ok(());
    };
    let ext = external_id(owner, repo, issue.number);
    // An issue hit by several queries is handled only once
    if !seen.insert(ext.clone()) {
        return Ok(());
    }

    let title = format!("#{} {}", issue.number, sanitize_text(&issue.title));
    let deadline = issue
        .milestone
        .as_ref()
        .and_then(|m| m.due_on.as_deref())
        .and_then(|d| due_on_to_date(d, tz));

    match crate::db::find_task_id_by_external_id(db, &ext)? {
        Some(id) => {
            // Existing task: update only the issue-owned columns
            crate::db::update_task_imported_fields(db, id, &title, deadline.as_deref())?;
            summary.updated += 1;
        }
        None => {
            crate::db::insert_backlog_task_imported(
                db,
                &title,
                &opts.category_id,
                DEFAULT_DURATION_MIN,
                deadline.as_deref(),
                &ext,
            )?;
            summary.created += 1;
        }
    }
    Ok(())
}

/// Splits an external_id `github:owner/repo#123` into `(owner, repo, number)`.
/// `None` for a malformed id.
pub fn parse_external_id(ext: &str) -> Option<(String, String, u64)> {
    let rest = ext.strip_prefix("github:")?;
    let (repo_part, num_part) = rest.rsplit_once('#')?;
    let (owner, repo) = repo_part.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let number: u64 = num_part.parse().ok()?;
    Some((owner.to_string(), repo.to_string(), number))
}

/// Result of closed detection, with closed tasks and non-fatal errors kept apart.
#[derive(Debug, Default, Clone)]
pub struct ClosedDetection {
    /// (task_id, external_id) of the tasks found closed.
    pub closed: Vec<(i64, String)>,
    /// Errors of single fetches; not fatal, since the next run retries them.
    pub errors: Vec<String>,
}

/// For backlog GitHub tasks missing from the latest search results `seen`, checks the state
/// with `GET /repos/:owner/:repo/issues/:number` and lists only the closed ones.
/// - 404 (repo deleted, access lost) does not mark the task done
/// - A still open issue (e.g. the query was narrowed) is ignored
/// - HTTP errors go into `errors` and are retried on the next run
pub fn detect_closed(
    db: &Database,
    token: &auth::GithubToken,
    seen: &HashSet<String>,
) -> Result<ClosedDetection> {
    let backlog_github = crate::db::list_backlog_github_external_ids(db)?;
    let mut result = ClosedDetection::default();
    for (id, ext) in backlog_github {
        if seen.contains(&ext) {
            continue;
        }
        let Some((owner, repo, number)) = parse_external_id(&ext) else {
            result
                .errors
                .push(format!("external_id パース失敗 (skip): {ext}"));
            continue;
        };
        match api::fetch_issue_state(token, &owner, &repo, number) {
            Ok(Some(api::IssueState::Closed)) => result.closed.push((id, ext)),
            Ok(Some(api::IssueState::Open)) => {}
            Ok(None) => {}
            Err(e) => result
                .errors
                .push(format!("issue #{number} の state 取得失敗: {e}")),
        }
    }
    Ok(result)
}

/// Converts milestone.due_on (RFC 3339, e.g. `2026-06-15T20:00:00Z`) to `YYYY-MM-DD` in the local tz.
/// Milestones are day-granular, but GitHub stores them in UTC, so a late UTC time is the next day in zones east of UTC.
/// The local tz is the `tz:` line of `_meta`, as in the gcal import.
fn due_on_to_date(due_on: &str, tz: chrono_tz::Tz) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(due_on).ok()?;
    let local = dt.with_timezone(&tz);
    Some(local.date_naive().format("%Y-%m-%d").to_string())
}

/// Reads the `tz:` line of `_meta` as chrono_tz::Tz; UTC when missing.
/// Duplicates gcal::tz::read_meta_tz so that this module works without the gcal feature.
fn read_meta_tz(data_dir: &std::path::Path) -> chrono_tz::Tz {
    let path = data_dir.join("_meta");
    if let Ok(raw) = std::fs::read_to_string(&path) {
        for line in raw.lines() {
            if let Some(rest) = line.trim().strip_prefix("tz:") {
                if let Ok(tz) = rest.trim().parse::<chrono_tz::Tz>() {
                    return tz;
                }
            }
        }
    }
    chrono_tz::UTC
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> (tempfile::TempDir, Database) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("_meta"),
            "tz: Asia/Tokyo\nview_sync: async\n",
        )
        .unwrap();
        let mut db = Database::open(tmp.path(), Some("test-github-import")).unwrap();
        crate::init::apply_schema(&mut db).unwrap();
        (tmp, db)
    }

    fn issue(number: u64, title: &str, due_on: Option<&str>) -> Issue {
        let json = serde_json::json!({
            "number": number,
            "title": title,
            "repository_url": "https://api.github.com/repos/ycookiey/ytasky",
            "milestone": due_on.map(|d| serde_json::json!({ "due_on": d })),
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn due_on_conversion() {
        // 07:00 UTC stays on the same date
        assert_eq!(
            due_on_to_date("2026-06-15T07:00:00Z", chrono_tz::UTC).as_deref(),
            Some("2026-06-15")
        );
        assert_eq!(due_on_to_date("not-a-date", chrono_tz::UTC), None);
        assert_eq!(due_on_to_date("", chrono_tz::UTC), None);
    }

    #[test]
    fn due_on_shifts_to_local_tz() {
        // 20:00 UTC -> 05:00 next day in JST -> next date
        assert_eq!(
            due_on_to_date("2026-06-15T20:00:00Z", chrono_tz::Asia::Tokyo).as_deref(),
            Some("2026-06-16")
        );
        // 00:00 UTC -> 09:00 JST -> same date
        assert_eq!(
            due_on_to_date("2026-06-15T00:00:00Z", chrono_tz::Asia::Tokyo).as_deref(),
            Some("2026-06-15")
        );
    }

    #[test]
    fn new_issue_inserts_backlog_task() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        handle_issue(&mut db, &issue(12, "バグ修正", Some("2026-06-15T00:00:00Z")), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(summary.updated, 0);

        let tasks = crate::db::load_backlog_tasks(&mut db).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "#12 バグ修正");
        assert_eq!(tasks[0].category_id, "3");
        assert_eq!(tasks[0].duration_min, 30);
        assert_eq!(tasks[0].deadline.as_deref(), Some("2026-06-15"));
        assert_eq!(
            tasks[0].external_id.as_deref(),
            Some("github:ycookiey/ytasky#12")
        );
    }

    #[test]
    fn existing_issue_updates_only_issue_owned_fields() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut summary = ImportSummary::default();

        // First insert
        let mut seen = HashSet::new();
        handle_issue(&mut db, &issue(12, "旧タイトル", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;

        // Simulate the user changing category/duration by hand
        crate::db::update_task_with_deadline(&mut db, id, "旧タイトル", "5", 90, None, None).unwrap();

        // Second import: the title changed
        let mut seen2 = HashSet::new();
        let mut summary2 = ImportSummary::default();
        handle_issue(&mut db, &issue(12, "新タイトル", None), &opts, chrono_tz::UTC, &mut seen2, &mut summary2).unwrap();
        assert_eq!(summary2.created, 0);
        assert_eq!(summary2.updated, 1);

        let tasks = crate::db::load_backlog_tasks(&mut db).unwrap();
        assert_eq!(tasks.len(), 1, "重複 insert されていない");
        // The issue-owned column (title) is updated
        assert_eq!(tasks[0].title, "#12 新タイトル");
        // The user-owned columns (category/duration) are kept
        assert_eq!(tasks[0].category_id, "5");
        assert_eq!(tasks[0].duration_min, 90);
    }

    #[test]
    fn reopened_issue_resets_status_to_todo() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut summary = ImportSummary::default();

        let mut seen = HashSet::new();
        handle_issue(&mut db, &issue(30, "reopen対象", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;

        // As detect_closed does: mark the closed issue done
        crate::db::set_task_status_done(&mut db, id).unwrap();
        // load_backlog_tasks excludes done tasks, so it disappears
        assert!(crate::db::load_backlog_tasks(&mut db).unwrap().is_empty());

        // Reopened upstream: import again
        let mut seen2 = HashSet::new();
        let mut summary2 = ImportSummary::default();
        handle_issue(&mut db, &issue(30, "reopen対象", None), &opts, chrono_tz::UTC, &mut seen2, &mut summary2).unwrap();
        assert_eq!(summary2.updated, 1);
        let tasks = crate::db::load_backlog_tasks(&mut db).unwrap();
        assert_eq!(tasks.len(), 1, "reopen された issue が backlog に復帰する");
        assert_eq!(tasks[0].id, id);
    }

    #[test]
    fn pull_request_is_skipped() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        let mut pr: Issue = issue(99, "PR", None);
        pr.pull_request = Some(serde_json::from_str("{}").unwrap());
        handle_issue(&mut db, &pr, &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.created, 0);
        assert!(crate::db::load_backlog_tasks(&mut db).unwrap().is_empty());
    }

    #[test]
    fn duplicate_across_queries_deduped() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        handle_issue(&mut db, &issue(12, "x", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        handle_issue(&mut db, &issue(12, "x", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(crate::db::load_backlog_tasks(&mut db).unwrap().len(), 1);
    }

    #[test]
    fn parse_external_id_ok() {
        assert_eq!(
            parse_external_id("github:ycookiey/ytasky#123"),
            Some(("ycookiey".into(), "ytasky".into(), 123))
        );
        // A repo name may contain - and .
        assert_eq!(
            parse_external_id("github:foo/bar-baz.rs#7"),
            Some(("foo".into(), "bar-baz.rs".into(), 7))
        );
    }

    #[test]
    fn parse_external_id_invalid() {
        assert!(parse_external_id("gcal:x").is_none()); // other prefix
        assert!(parse_external_id("github:foo#1").is_none()); // no repo
        assert!(parse_external_id("github:foo/bar").is_none()); // no #number
        assert!(parse_external_id("github:foo/bar#abc").is_none()); // non-numeric number
        assert!(parse_external_id("github:/bar#1").is_none()); // empty owner
        assert!(parse_external_id("github:foo/#1").is_none()); // empty repo
        assert!(parse_external_id("").is_none());
    }

    #[test]
    fn list_backlog_github_only_github_backlog() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        // GitHub backlog: 2 tasks
        handle_issue(&mut db, &issue(10, "a", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        handle_issue(&mut db, &issue(11, "b", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        // Manual backlog: 1 task (no external_id, so not a target)
        crate::db::insert_backlog_task(&mut db, "手動", "3", 30, None).unwrap();

        let mut exts: Vec<String> = crate::db::list_backlog_github_external_ids(&db)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        exts.sort();
        assert_eq!(
            exts,
            vec![
                "github:ycookiey/ytasky#10".to_string(),
                "github:ycookiey/ytasky#11".to_string(),
            ]
        );
    }

    #[test]
    fn list_backlog_github_excludes_scheduled() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        handle_issue(&mut db, &issue(20, "y", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        // Moved to the schedule (is_backlog=false), so not a target
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;
        crate::db::set_backlog_flag(&mut db, id, false).unwrap();

        let list = crate::db::list_backlog_github_external_ids(&db).unwrap();
        assert!(list.is_empty(), "backlog を外れた task は対象外");
    }
}
