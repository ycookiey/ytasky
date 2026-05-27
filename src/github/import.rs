//! GitHub issue → ytasky backlog タスクの import オーケストレータ。
//!
//! - 検索クエリごとに search/issues を呼び、open issue を backlog タスクへ取り込む
//! - PR は skip。複数クエリで重複した issue は external_id で排除
//! - 列所有権分割: 新規は全列 insert、既存は title/deadline のみ update
//!   （category/duration/配置などユーザー設定列は保持）

use std::collections::HashSet;

use anyhow::Result;
use ybasey::Database;

use crate::github::auth;
use crate::github::types::Issue;
use crate::github::{api, external_id};
use crate::text::sanitize_text;

/// issue から取れない duration のデフォルト（分）。
const DEFAULT_DURATION_MIN: i32 = 30;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// 初回 insert 時のデフォルトカテゴリ（既存タスクには適用しない）。
    pub category_id: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            category_id: "3".into(), // 3 = 開発・仕事
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// 検索クエリ群に一致する open issue を import する。
pub fn import_queries(
    db: &mut Database,
    queries: &[String],
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    let token = auth::get_token()?;
    let mut summary = ImportSummary::default();
    let mut seen: HashSet<String> = HashSet::new();

    for query in queries {
        let issues = api::search_issues(&token, query)?;
        for issue in issues {
            if let Err(e) = handle_issue(db, &issue, opts, &mut seen, &mut summary) {
                summary
                    .errors
                    .push(format!("issue #{} の取り込み失敗: {e}", issue.number));
            }
        }
    }
    Ok(summary)
}

fn handle_issue(
    db: &mut Database,
    issue: &Issue,
    opts: &ImportOptions,
    seen: &mut HashSet<String>,
    summary: &mut ImportSummary,
) -> Result<()> {
    // PR は対象外
    if issue.is_pull_request() {
        summary.skipped += 1;
        return Ok(());
    }
    let Some((owner, repo)) = issue.owner_repo() else {
        summary.skipped += 1;
        return Ok(());
    };
    let ext = external_id(owner, repo, issue.number);
    // 複数クエリで重複ヒットした issue は最初の1件のみ処理
    if !seen.insert(ext.clone()) {
        return Ok(());
    }

    let title = format!("#{} {}", issue.number, sanitize_text(&issue.title));
    let deadline = issue
        .milestone
        .as_ref()
        .and_then(|m| m.due_on.as_deref())
        .and_then(due_on_to_date);

    match crate::db::find_task_id_by_external_id(db, &ext)? {
        Some(id) => {
            // 既存: issue 所有列のみ更新（列所有権分割）
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

/// milestone.due_on (ISO8601 例 `2026-06-15T07:00:00Z`) を `YYYY-MM-DD` にする。
/// milestone は日粒度のため日付部分のみ採る（時刻は切り捨て）。
fn due_on_to_date(due_on: &str) -> Option<String> {
    let date = due_on.split('T').next()?;
    let b = date.as_bytes();
    // YYYY-MM-DD 形式の簡易検証
    if b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && date.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        Some(date.to_string())
    } else {
        None
    }
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
        assert_eq!(
            due_on_to_date("2026-06-15T07:00:00Z").as_deref(),
            Some("2026-06-15")
        );
        assert_eq!(due_on_to_date("not-a-date"), None);
        assert_eq!(due_on_to_date(""), None);
    }

    #[test]
    fn new_issue_inserts_backlog_task() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        handle_issue(&mut db, &issue(12, "バグ修正", Some("2026-06-15T00:00:00Z")), &opts, &mut seen, &mut summary).unwrap();
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

        // 初回 insert
        let mut seen = HashSet::new();
        handle_issue(&mut db, &issue(12, "旧タイトル", None), &opts, &mut seen, &mut summary).unwrap();
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;

        // ユーザーが category/duration を手で変更したと仮定
        crate::db::update_task_with_deadline(&mut db, id, "旧タイトル", "5", 90, None, None).unwrap();

        // 2回目 import: title が変わった
        let mut seen2 = HashSet::new();
        let mut summary2 = ImportSummary::default();
        handle_issue(&mut db, &issue(12, "新タイトル", None), &opts, &mut seen2, &mut summary2).unwrap();
        assert_eq!(summary2.created, 0);
        assert_eq!(summary2.updated, 1);

        let tasks = crate::db::load_backlog_tasks(&mut db).unwrap();
        assert_eq!(tasks.len(), 1, "重複 insert されていない");
        // issue 所有列 (title) は更新される
        assert_eq!(tasks[0].title, "#12 新タイトル");
        // ユーザー所有列 (category/duration) は保持される
        assert_eq!(tasks[0].category_id, "5");
        assert_eq!(tasks[0].duration_min, 90);
    }

    #[test]
    fn pull_request_is_skipped() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        let mut pr: Issue = issue(99, "PR", None);
        pr.pull_request = Some(serde_json::from_str("{}").unwrap());
        handle_issue(&mut db, &pr, &opts, &mut seen, &mut summary).unwrap();
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

        handle_issue(&mut db, &issue(12, "x", None), &opts, &mut seen, &mut summary).unwrap();
        handle_issue(&mut db, &issue(12, "x", None), &opts, &mut seen, &mut summary).unwrap();
        assert_eq!(summary.created, 1);
        assert_eq!(crate::db::load_backlog_tasks(&mut db).unwrap().len(), 1);
    }
}
