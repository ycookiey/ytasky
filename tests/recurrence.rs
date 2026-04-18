//! recurrence.rs 統合テスト — Material 方式の展開ロジックを検証する

use std::path::Path;

// ---- setup helper ------------------------------------------------------------

fn setup_db(data_dir: &Path) -> ybasey::Database {
    std::fs::write(
        data_dir.join("_meta"),
        "tz: Asia/Tokyo\nview_sync: async\n",
    )
    .unwrap();
    let mut db = ybasey::Database::open(data_dir, Some("test-db")).unwrap();
    ytasky::init::apply_schema(&mut db).unwrap();
    db
}

/// horizon file を tempdir 内に作成するための環境変数設定ヘルパー。
/// テスト後に削除されるよう (TempDir, TempDir) を返す。
/// 並列実行時の競合を避けるため set_var は unsafe。テスト内で個別の config dir を渡すこと。
fn setup_horizon_env() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: テスト専用。並列テストとの競合に注意 (単一スレッドで実行)。
    unsafe {
        std::env::set_var("YTASKY_CONFIG_DIR", tmp.path().to_str().unwrap());
    }
    tmp
}

/// recurrence を DB に挿入して id を返す
fn insert_recurrence(
    db: &mut ybasey::Database,
    title: &str,
    pattern: &str,
    pattern_data: Option<&str>,
    start_date: &str,
    end_date: Option<&str>,
) -> i64 {
    ytasky::db::insert_recurrence(
        db, title, "1", 30, None, pattern, pattern_data, start_date, end_date,
    )
    .unwrap()
}

// ---- Sub-task 2: generate_dates_for_recurrence / build_task_records ----------

#[test]
fn test_generate_dates_daily_7days() {
    let rec = ytasky::model::Recurrence {
        id: 1,
        title: "daily".into(),
        category_id: "1".into(),
        duration_min: 30,
        fixed_start: None,
        pattern: "daily".into(),
        pattern_data: None,
        start_date: "2026-04-01".into(),
        end_date: None,
    };
    use chrono::NaiveDate;
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 7).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 7);
}

#[test]
fn test_generate_dates_weekly_14days() {
    // 月曜(1)・水曜(3) のみ
    let rec = ytasky::model::Recurrence {
        id: 1,
        title: "weekly".into(),
        category_id: "1".into(),
        duration_min: 30,
        fixed_start: None,
        pattern: "weekly".into(),
        pattern_data: Some(r#"{"days":[1,3]}"#.into()),
        start_date: "2026-04-01".into(),
        end_date: None,
    };
    use chrono::NaiveDate;
    // 2026-04-01 は水曜。14日間 → 月曜2回 + 水曜2回 = 4件
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 14).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 4);
}

#[test]
fn test_generate_dates_monthly_60days() {
    // 毎月15日
    let rec = ytasky::model::Recurrence {
        id: 1,
        title: "monthly".into(),
        category_id: "1".into(),
        duration_min: 30,
        fixed_start: None,
        pattern: "monthly".into(),
        pattern_data: Some(r#"{"days":[15]}"#.into()),
        start_date: "2026-04-01".into(),
        end_date: None,
    };
    use chrono::NaiveDate;
    // 2026-04-01 から 60日: 4/15, 5/15 → 2件
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 5, 30).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 2);
}

// ---- Sub-task 3: expand_recurrences_to_horizon --------------------------------

#[test]
fn test_expand_to_horizon_daily() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    // start_date = 今日
    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // 2年 = ~730日、今日分を含む
    assert!(result.total_inserted >= 700, "inserted: {}", result.total_inserted);
    assert!(result.total_inserted <= 740);
}

#[test]
fn test_expand_to_horizon_weekly() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    // 月曜(1)・水曜(3)
    insert_recurrence(&mut db, "weekly task", "weekly", Some(r#"{"days":[1,3]}"#), &start_str, None);

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // 2年 = ~104週、各週2回 = ~208件
    assert!(result.total_inserted >= 200, "inserted: {}", result.total_inserted);
    assert!(result.total_inserted <= 216);
}

#[test]
fn test_expand_to_horizon_incremental() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);

    let result1 = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    assert!(result1.total_inserted > 0);

    // 2回目: horizon 更新済みなので追加 0 件
    let result2 = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    assert_eq!(result2.total_inserted, 0);
}

#[test]
fn test_expand_to_horizon_with_end_date() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    // end_date = 今日から 3ヶ月
    let end = today + chrono::Months::new(3);
    let end_str = end.format("%Y-%m-%d").to_string();
    insert_recurrence(&mut db, "daily task", "daily", None, &start_str, Some(&end_str));

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // 約 90 日分 (±1)
    assert!(result.total_inserted >= 88, "inserted: {}", result.total_inserted);
    assert!(result.total_inserted <= 93);
}

#[test]
fn test_expand_to_horizon_with_exceptions() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    let rec_id = insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);

    // 今日と明日を exception に追加
    let tomorrow = today + chrono::Duration::days(1);
    ytasky::db::add_recurrence_exception(&mut db, rec_id, &today.format("%Y-%m-%d").to_string()).unwrap();
    ytasky::db::add_recurrence_exception(&mut db, rec_id, &tomorrow.format("%Y-%m-%d").to_string()).unwrap();

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // 2年分 - 2日 = ~728件
    assert!(result.total_inserted >= 698, "inserted: {}", result.total_inserted);
    assert!(result.total_inserted <= 738);
}

// ---- Sub-task 4: replace_future_tasks ----------------------------------------

#[test]
fn test_replace_future_tasks() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    // daily で展開
    let rec_id = insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);
    ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();

    // pattern を weekly(月曜のみ) に変更
    let rec = ytasky::db::load_recurrences(&db)
        .unwrap()
        .into_iter()
        .find(|r| r.id == rec_id)
        .unwrap();
    ytasky::db::update_recurrence(
        &mut db,
        rec_id,
        &rec.title,
        &rec.category_id,
        rec.duration_min,
        rec.fixed_start,
        "weekly",
        Some(r#"{"days":[1]}"#),
        &rec.start_date,
        rec.end_date.as_deref(),
    )
    .unwrap();

    let result = ytasky::recurrence::replace_future_tasks(&mut db, rec_id).unwrap();
    // 削除: 未来の daily 分 (多数)
    assert!(result.deleted > 100, "deleted: {}", result.deleted);
    // 再生成: 週1回 × 2年 = ~104件
    assert!(result.inserted >= 100, "inserted: {}", result.inserted);
    assert!(result.inserted <= 108);
}

#[test]
fn test_replace_future_tasks_preserves_completed() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let start_str = yesterday.format("%Y-%m-%d").to_string();
    let rec_id = insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);
    ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();

    // 昨日のタスクに actual_start をセット (完了扱い)
    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();
    let tasks = ytasky::db::load_tasks(&mut db, &yesterday_str).unwrap();
    let completed = tasks
        .iter()
        .find(|t| t.recurrence_id == Some(rec_id));
    if let Some(t) = completed {
        ytasky::db::update_actual(&mut db, t.id, Some(480), Some(510)).unwrap();
    }

    let result = ytasky::recurrence::replace_future_tasks(&mut db, rec_id).unwrap();
    // 昨日の完了済みタスクは削除されていない (過去 + actual_start あり)
    let tasks_yesterday = ytasky::db::load_tasks(&mut db, &yesterday_str).unwrap();
    let still_exists = tasks_yesterday
        .iter()
        .any(|t| t.recurrence_id == Some(rec_id) && t.actual_start.is_some());
    assert!(still_exists, "completed task should be preserved");
    // 未来タスクは再生成
    assert!(result.inserted > 0 || result.deleted > 0);
}

// ---- Sub-task 6: UndoManager max_size ----------------------------------------

#[test]
fn test_undo_manager_max_size() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());
    let mut manager = ytasky::history::UndoManager::with_max_size(100);

    // 101回 execute → stack は 100 件 (最古 1 件破棄)
    for i in 0..101 {
        let date = format!("2026-04-{:02}", (i % 28) + 1);
        let cmd = Box::new(ytasky::history::AddTaskCommand::new(
            date,
            format!("task {i}"),
            "1".into(),
            30,
            None,
        ));
        manager.execute_command(&mut db, cmd).unwrap();
    }
    assert_eq!(manager.undo_len(), 100);
}
