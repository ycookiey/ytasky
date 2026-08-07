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
    /// upstream で closed 化を検出して `status="done"` にした件数。
    pub closed_done: usize,
    pub errors: Vec<String>,
}

/// 検索クエリ群に一致する open issue を import し、
/// 併せて既存の backlog GitHub タスクのうち upstream で closed 化したものを検出して done 化する。
pub fn import_queries(
    db: &mut Database,
    queries: &[String],
    opts: &ImportOptions,
) -> Result<ImportSummary> {
    let token = auth::get_token()?;
    let mut summary = ImportSummary::default();
    let mut seen: HashSet<String> = HashSet::new();

    // milestone.due_on の local date 変換用 tz (data_dir/_meta の tz: 行を参照)
    let tz = crate::db::data_dir()
        .ok()
        .map(|d| read_meta_tz(&d))
        .unwrap_or(chrono_tz::UTC);

    for query in queries {
        // 単一 query の失敗が全体を止めないよう ? ではなく errors に集約する。
        // 前 query で挿入済みの task はそのまま残り、detect_closed も後段で走る。
        match api::search_issues(&token, query) {
            Ok(outcome) => {
                // 1000 件超過・incomplete_results 等の non-fatal warning を errors に集約
                // (旧実装は eprintln で alt-screen TUI を破壊していた)。
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

    // seen に含まれなかった backlog GitHub タスクは upstream で閉じられた可能性がある。
    // 個別 API で state 確認し、closed なもののみ status="done" にする。
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
        .and_then(|d| due_on_to_date(d, tz));

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

/// `github:owner/repo#123` 形式の external_id を `(owner, repo, number)` に分解する。
/// 不正形式は `None`。
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

/// closed 検出の結果。closed リストと非致命エラーを分けて返す。
#[derive(Debug, Default, Clone)]
pub struct ClosedDetection {
    /// closed と判定できた (task_id, external_id) のリスト。
    pub closed: Vec<(i64, String)>,
    /// 個別 fetch のエラー（次回再試行で解消し得るので致命ではない）。
    pub errors: Vec<String>,
}

/// backlog の GitHub import タスクのうち、直近 search 結果 `seen` に含まれなかったものについて
/// `GET /repos/:owner/:repo/issues/:number` で state を確認し closed のみ列挙する。
/// - 404 (repo 削除・権限喪失等) は勝手に done 化しない
/// - open だった（クエリを狭めた等の理由）は無視
/// - HTTP エラーは `errors` に積んで次回リトライに回す
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

/// milestone.due_on (RFC3339 例 `2026-06-15T20:00:00Z`) を local tz の `YYYY-MM-DD` にする。
/// milestone は日粒度だが GitHub は UTC で保持するため、UTC 深夜だと JST では翌日にずれる。
/// `_meta` の `tz:` を参照して local date を求める（gcal と同じパターン）。
fn due_on_to_date(due_on: &str, tz: chrono_tz::Tz) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(due_on).ok()?;
    let local = dt.with_timezone(&tz);
    Some(local.date_naive().format("%Y-%m-%d").to_string())
}

/// `_meta` の `tz:` 行から chrono_tz::Tz を得る。無ければ UTC。
/// gcal::tz::read_meta_tz と同等のロジック（gcal feature 非依存で使うため複製）。
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
        // UTC 07:00 は UTC 上でも同日
        assert_eq!(
            due_on_to_date("2026-06-15T07:00:00Z", chrono_tz::UTC).as_deref(),
            Some("2026-06-15")
        );
        assert_eq!(due_on_to_date("not-a-date", chrono_tz::UTC), None);
        assert_eq!(due_on_to_date("", chrono_tz::UTC), None);
    }

    #[test]
    fn due_on_shifts_to_local_tz() {
        // UTC 20:00 → JST 翌日 05:00 → 翌日日付
        assert_eq!(
            due_on_to_date("2026-06-15T20:00:00Z", chrono_tz::Asia::Tokyo).as_deref(),
            Some("2026-06-16")
        );
        // UTC 00:00 → JST 09:00 → 同日
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

        // 初回 insert
        let mut seen = HashSet::new();
        handle_issue(&mut db, &issue(12, "旧タイトル", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;

        // ユーザーが category/duration を手で変更したと仮定
        crate::db::update_task_with_deadline(&mut db, id, "旧タイトル", "5", 90, None, None).unwrap();

        // 2回目 import: title が変わった
        let mut seen2 = HashSet::new();
        let mut summary2 = ImportSummary::default();
        handle_issue(&mut db, &issue(12, "新タイトル", None), &opts, chrono_tz::UTC, &mut seen2, &mut summary2).unwrap();
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
    fn reopened_issue_resets_status_to_todo() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut summary = ImportSummary::default();

        let mut seen = HashSet::new();
        handle_issue(&mut db, &issue(30, "reopen対象", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;

        // detect_closed 相当: closed 検出で status="done" にする
        crate::db::set_task_status_done(&mut db, id).unwrap();
        // load_backlog_tasks は done を除外する → 見えなくなる
        assert!(crate::db::load_backlog_tasks(&mut db).unwrap().is_empty());

        // upstream で reopen → 再度 import
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
        // repo に - や . を含んでも OK
        assert_eq!(
            parse_external_id("github:foo/bar-baz.rs#7"),
            Some(("foo".into(), "bar-baz.rs".into(), 7))
        );
    }

    #[test]
    fn parse_external_id_invalid() {
        assert!(parse_external_id("gcal:x").is_none()); // 別 prefix
        assert!(parse_external_id("github:foo#1").is_none()); // repo なし
        assert!(parse_external_id("github:foo/bar").is_none()); // #number なし
        assert!(parse_external_id("github:foo/bar#abc").is_none()); // number 非数値
        assert!(parse_external_id("github:/bar#1").is_none()); // owner 空
        assert!(parse_external_id("github:foo/#1").is_none()); // repo 空
        assert!(parse_external_id("").is_none());
    }

    #[test]
    fn list_backlog_github_only_github_backlog() {
        let (_t, mut db) = setup_db();
        let opts = ImportOptions::default();
        let mut seen = HashSet::new();
        let mut summary = ImportSummary::default();

        // GitHub backlog: 2件
        handle_issue(&mut db, &issue(10, "a", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        handle_issue(&mut db, &issue(11, "b", None), &opts, chrono_tz::UTC, &mut seen, &mut summary).unwrap();
        // 手動 backlog: 1件（external_id なし → 対象外）
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
        // schedule 済み (is_backlog=false) に移動 → 対象外
        let id = crate::db::load_backlog_tasks(&mut db).unwrap()[0].id;
        crate::db::set_backlog_flag(&mut db, id, false).unwrap();

        let list = crate::db::list_backlog_github_external_ids(&db).unwrap();
        assert!(list.is_empty(), "backlog を外れた task は対象外");
    }
}
