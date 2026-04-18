//! db.rs 統合テスト — ybasey Database API 経由の全 pub 関数を検証する
//!
//! 各テストは tempdir に独立した ybasey DB を作り、相互干渉なし。

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

// ---- load_categories ---------------------------------------------------------

#[test]
fn test_load_categories_returns_9() {
    let tmp = tempfile::tempdir().unwrap();
    let db = setup_db(tmp.path());
    let cats = ytasky::db::load_categories(&db).unwrap();
    assert_eq!(cats.len(), 9);
    // id は "1".."9" の文字列
    assert!(cats.iter().any(|c| c.id == "1"));
    assert!(cats.iter().any(|c| c.id == "9"));
}

// ---- load_tasks / insert_task / delete_task ----------------------------------

#[test]
fn test_insert_and_load_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "テスト", "1", 30, None).unwrap();
    assert!(id > 0);

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "テスト");
    assert_eq!(tasks[0].category_id, "1");
    assert_eq!(tasks[0].duration_min, 30);
    assert!(!tasks[0].is_backlog);
}

#[test]
fn test_load_tasks_empty_day() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());
    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-20").unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn test_load_tasks_filter_is_backlog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    // backlog task は load_tasks で出てこない
    ytasky::db::insert_backlog_task(&mut db, "backlog task", "1", 30, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "schedule task", "1", 30, None).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "schedule task");
}

#[test]
fn test_delete_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "削除対象", "1", 30, None).unwrap();
    ytasky::db::delete_task(&mut db, id).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn test_update_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "元タイトル", "1", 30, None).unwrap();
    ytasky::db::update_task(&mut db, id, "新タイトル", "2", 60, None).unwrap();

    let task = ytasky::db::load_task_by_id(&db, id).unwrap().unwrap();
    assert_eq!(task.title, "新タイトル");
    assert_eq!(task.category_id, "2");
    assert_eq!(task.duration_min, 60);
}

// ---- insert_task_with_deadline -----------------------------------------------

#[test]
fn test_insert_task_with_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task_with_deadline(
        &mut db,
        "2026-04-19",
        "期限付き",
        "1",
        30,
        None,
        Some("2026-04-30"),
    )
    .unwrap();

    let task = ytasky::db::load_task_by_id(&db, id).unwrap().unwrap();
    assert_eq!(task.deadline, Some("2026-04-30".to_string()));
}

// ---- update_actual -----------------------------------------------------------

#[test]
fn test_update_actual() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "実績", "1", 30, None).unwrap();
    ytasky::db::update_actual(&mut db, id, Some(540), Some(570)).unwrap();

    let task = ytasky::db::load_task_by_id(&db, id).unwrap().unwrap();
    assert_eq!(task.actual_start, Some(540));
    assert_eq!(task.actual_end, Some(570));
}

// ---- sort_order / normalize --------------------------------------------------

#[test]
fn test_sort_order_sequential() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_task(&mut db, "2026-04-19", "T1", "1", 30, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "T2", "1", 30, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "T3", "1", 30, None).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    let orders: Vec<i32> = tasks.iter().map(|t| t.sort_order).collect();
    assert_eq!(orders, vec![0, 1, 2]);
}

#[test]
fn test_swap_sort_order() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id1 = ytasky::db::insert_task(&mut db, "2026-04-19", "T1", "1", 30, None).unwrap();
    let id2 = ytasky::db::insert_task(&mut db, "2026-04-19", "T2", "1", 30, None).unwrap();

    ytasky::db::swap_sort_order(&mut db, id1, id2).unwrap();

    let t1 = ytasky::db::load_task_by_id(&db, id1).unwrap().unwrap();
    let t2 = ytasky::db::load_task_by_id(&db, id2).unwrap().unwrap();
    assert_eq!(t1.sort_order, 1);
    assert_eq!(t2.sort_order, 0);
}

#[test]
fn test_swap_sort_order_different_dates_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id1 = ytasky::db::insert_task(&mut db, "2026-04-19", "T1", "1", 30, None).unwrap();
    let id2 = ytasky::db::insert_task(&mut db, "2026-04-20", "T2", "1", 30, None).unwrap();

    // 異日 swap は no-op
    ytasky::db::swap_sort_order(&mut db, id1, id2).unwrap();

    let t1 = ytasky::db::load_task_by_id(&db, id1).unwrap().unwrap();
    let t2 = ytasky::db::load_task_by_id(&db, id2).unwrap().unwrap();
    // sort_order 変わらず (各日で 0)
    assert_eq!(t1.sort_order, 0);
    assert_eq!(t2.sort_order, 0);
}

// ---- backlog -----------------------------------------------------------------

#[test]
fn test_load_backlog_tasks() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_backlog_task(&mut db, "B1", "1", 30, None).unwrap();
    ytasky::db::insert_backlog_task(&mut db, "B2", "1", 30, Some("2026-04-25")).unwrap();

    let tasks = ytasky::db::load_backlog_tasks(&db).unwrap();
    assert_eq!(tasks.len(), 2);
    // deadline あり (B2) が先に来る
    assert_eq!(tasks[0].title, "B2");
    assert_eq!(tasks[1].title, "B1");
}

#[test]
fn test_set_backlog_flag_to_true() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "スケジュール", "1", 30, None).unwrap();
    ytasky::db::set_backlog_flag(&mut db, id, true).unwrap();

    let task = ytasky::db::load_task_by_id(&db, id).unwrap().unwrap();
    assert!(task.is_backlog);

    let bl = ytasky::db::load_backlog_tasks(&db).unwrap();
    assert_eq!(bl.len(), 1);
}

#[test]
fn test_insert_backlog_task_at_head() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let sched_id =
        ytasky::db::insert_task(&mut db, "2026-04-19", "既存", "1", 30, None).unwrap();
    let bl_id = ytasky::db::insert_backlog_task(&mut db, "挿入", "1", 30, None).unwrap();

    // 先頭 (index=0) に挿入
    ytasky::db::insert_backlog_task_at(&mut db, bl_id, "2026-04-19", 0).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, bl_id);
    assert_eq!(tasks[1].id, sched_id);
}

#[test]
fn test_append_backlog_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id1 = ytasky::db::insert_task(&mut db, "2026-04-19", "T1", "1", 30, None).unwrap();
    let bl_id = ytasky::db::insert_backlog_task(&mut db, "末尾挿入", "1", 30, None).unwrap();

    ytasky::db::append_backlog_task(&mut db, bl_id, "2026-04-19").unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, id1);
    assert_eq!(tasks[1].id, bl_id);
}

// ---- restore_task_to_schedule / restore_task_to_backlog ----------------------

#[test]
fn test_restore_task_to_schedule() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "元タスク", "1", 30, None).unwrap();
    // バックログに移動
    ytasky::db::set_backlog_flag(&mut db, id, true).unwrap();
    // スケジュールに戻す
    ytasky::db::restore_task_to_schedule(&mut db, id, "2026-04-19", 0).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(!tasks[0].is_backlog);
}

#[test]
fn test_restore_task_to_backlog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let sched_id =
        ytasky::db::insert_task(&mut db, "2026-04-19", "スケジュール", "1", 30, None).unwrap();
    let bl_id = ytasky::db::insert_backlog_task(&mut db, "バックログ", "1", 30, None).unwrap();

    // スケジュールからバックログへ移動
    ytasky::db::set_backlog_flag(&mut db, sched_id, true).unwrap();

    // バックログに戻す (sort_order=0)
    ytasky::db::restore_task_to_backlog(&mut db, bl_id, 0).unwrap();

    let bl = ytasky::db::load_backlog_tasks(&db).unwrap();
    assert_eq!(bl.len(), 2);
}

// ---- report ------------------------------------------------------------------

#[test]
fn test_report_by_category() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_task(&mut db, "2026-04-19", "T1", "1", 60, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "T2", "1", 30, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "T3", "2", 45, None).unwrap();

    let report = ytasky::db::report_by_category(&db, "2026-04-19").unwrap();
    // category_id=1 の planned_min = 90
    let r1 = report.iter().find(|r| r.category_id == "1").unwrap();
    assert_eq!(r1.planned_min, 90);
    // category_id=2 の planned_min = 45
    let r2 = report.iter().find(|r| r.category_id == "2").unwrap();
    assert_eq!(r2.planned_min, 45);
}

#[test]
fn test_report_by_title() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_task(&mut db, "2026-04-19", "同タイトル", "1", 60, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "同タイトル", "1", 30, None).unwrap();
    ytasky::db::insert_task(&mut db, "2026-04-19", "別タイトル", "2", 45, None).unwrap();

    let report = ytasky::db::report_by_title(&db, "2026-04-19").unwrap();
    // 同タイトルは集約 planned_min=90
    let r = report
        .iter()
        .find(|r| r.title == "同タイトル")
        .unwrap();
    assert_eq!(r.planned_min, 90);
    // 最大値が先頭
    assert_eq!(report[0].title, "同タイトル");
}

// ---- recurrence CRUD ---------------------------------------------------------

#[test]
fn test_insert_load_recurrence() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_recurrence(
        &mut db, "毎日タスク", "1", 30, None,
        "daily", None, "2026-01-01", None,
    ).unwrap();
    assert!(id > 0);

    let recs = ytasky::db::load_recurrences(&db).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].title, "毎日タスク");
    assert_eq!(recs[0].pattern, "daily");
}

#[test]
fn test_update_recurrence() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_recurrence(
        &mut db, "元タイトル", "1", 30, None,
        "daily", None, "2026-01-01", None,
    ).unwrap();

    ytasky::db::update_recurrence(
        &mut db, id, "新タイトル", "2", 60, None,
        "weekly", Some(r#"{"days":[1,3,5]}"#), "2026-01-01", None,
    ).unwrap();

    let recs = ytasky::db::load_recurrences(&db).unwrap();
    assert_eq!(recs[0].title, "新タイトル");
    assert_eq!(recs[0].pattern, "weekly");
}

#[test]
fn test_delete_recurrence_cascades() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    // recurrence 作成
    let rec_id = ytasky::db::insert_recurrence(
        &mut db, "週次", "1", 30, None,
        "weekly", Some(r#"{"days":[1]}"#), "2026-01-01", None,
    ).unwrap();

    // recurrence_exception 追加
    ytasky::db::add_recurrence_exception(&mut db, rec_id, "2026-04-21").unwrap();

    // recurrence に紐づくタスクを作成
    let task_id =
        ytasky::db::insert_task(&mut db, "2026-04-21", "週次インスタンス", "1", 30, None).unwrap();
    // recurrence_id を手動設定 (generate_recurring_tasks を使わずにテスト)
    ybasey::Database::update(
        &mut db,
        "tasks",
        task_id as u64,
        vec![("recurrence_id".into(), rec_id.to_string())],
    ).unwrap();

    // cascade delete
    ytasky::db::delete_recurrence(&mut db, rec_id).unwrap();

    // exception も削除されている
    let recs = ytasky::db::load_recurrences(&db).unwrap();
    assert!(recs.is_empty());

    // task.recurrence_id は null (set-null)
    let task = ytasky::db::load_task_by_id(&db, task_id).unwrap().unwrap();
    assert!(task.recurrence_id.is_none());
}

// ---- generate_recurring_tasks ------------------------------------------------

#[test]
fn test_generate_daily_recurring_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_recurrence(
        &mut db, "毎日", "1", 30, None,
        "daily", None, "2026-04-01", None,
    ).unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "毎日");
}

#[test]
fn test_generate_recurring_task_with_exception() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let rec_id = ytasky::db::insert_recurrence(
        &mut db, "毎日", "1", 30, None,
        "daily", None, "2026-04-01", None,
    ).unwrap();

    // 2026-04-19 を例外に登録
    ytasky::db::add_recurrence_exception(&mut db, rec_id, "2026-04-19").unwrap();

    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert!(tasks.is_empty(), "exception day should not generate task");
}

#[test]
fn test_generate_recurring_task_existing_skips() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    ytasky::db::insert_recurrence(
        &mut db, "毎日", "1", 30, None,
        "daily", None, "2026-04-01", None,
    ).unwrap();

    // 1回目の load_tasks で生成
    ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    // 2回目の load_tasks で重複生成しない
    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1, "should not duplicate");
}

#[test]
fn test_generate_weekly_recurring_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    // 2026-04-19 は日曜 = weekday 7 (chrono: 月曜=1)
    ytasky::db::insert_recurrence(
        &mut db, "日曜タスク", "1", 30, None,
        "weekly", Some(r#"{"days":[7]}"#), "2026-04-01", None,
    ).unwrap();

    // 日曜日
    let tasks = ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1);

    // 月曜日 (2026-04-20) は生成されない
    let tasks_mon = ytasky::db::load_tasks(&mut db, "2026-04-20").unwrap();
    assert!(tasks_mon.is_empty());
}

// ---- load_task_by_id / load_task_position ------------------------------------

#[test]
fn test_load_task_by_id_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let db = setup_db(tmp.path());
    let result = ytasky::db::load_task_by_id(&db, 9999).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_load_task_position() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let id = ytasky::db::insert_task(&mut db, "2026-04-19", "位置テスト", "1", 30, None).unwrap();
    let pos = ytasky::db::load_task_position(&db, id).unwrap().unwrap();
    assert_eq!(pos.0, "2026-04-19");
    assert_eq!(pos.1, 0);
    assert!(!pos.2);
}

// ---- create_recurrence_from_task ---------------------------------------------

#[test]
fn test_create_recurrence_from_task() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let task_id =
        ytasky::db::insert_task(&mut db, "2026-04-19", "元タスク", "1", 30, None).unwrap();

    let rec_id = ytasky::db::create_recurrence_from_task(
        &mut db, task_id, "daily", None, None,
    ).unwrap();
    assert!(rec_id > 0);

    // task に recurrence_id が設定されている
    let task = ytasky::db::load_task_by_id(&db, task_id).unwrap().unwrap();
    assert_eq!(task.recurrence_id, Some(rec_id));
}
