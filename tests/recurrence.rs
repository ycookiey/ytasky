//! Integration tests for recurrence expansion into concrete tasks.

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

/// Points `YTASKY_CONFIG_DIR` at a temp dir so the horizon file stays out of the
/// user's config. Keep the returned dir alive for the whole test.
/// The variable is process-wide, so parallel tests can overwrite each other's value.
fn setup_horizon_env() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: test-only. Racy under parallel tests; run with one thread to avoid it.
    unsafe {
        std::env::set_var("YTASKY_CONFIG_DIR", tmp.path().to_str().unwrap());
    }
    tmp
}

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

// ---- generate_dates_for_recurrence / build_task_records ----------------------

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
        external_id: None,
    };
    use chrono::NaiveDate;
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 7).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 7);
}

#[test]
fn test_generate_dates_weekly_14days() {
    // Mondays (1) and Wednesdays (3) only.
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
        external_id: None,
    };
    use chrono::NaiveDate;
    // 2026-04-01 is a Wednesday. 14 days hold 2 Mondays and 2 Wednesdays.
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 14).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 4);
}

#[test]
fn test_generate_dates_monthly_60days() {
    // The 15th of every month.
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
        external_id: None,
    };
    use chrono::NaiveDate;
    // 60 days from 2026-04-01 hold 4/15 and 5/15.
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 5, 30).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates.len(), 2);
}

// ---- interval / setpos edge cases ---------------------------------------------

fn make_rec(pattern: &str, pattern_data: Option<&str>, start: &str) -> ytasky::model::Recurrence {
    ytasky::model::Recurrence {
        id: 1,
        title: "t".into(),
        category_id: "1".into(),
        duration_min: 30,
        fixed_start: None,
        pattern: pattern.into(),
        pattern_data: pattern_data.map(Into::into),
        start_date: start.into(),
        end_date: None,
        external_id: None,
    }
}

#[test]
fn test_daily_interval_2() {
    use chrono::NaiveDate;
    let rec = make_rec("daily", Some(r#"{"interval":2}"#), "2026-04-01");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    // 4/1, 4/3, 4/5, 4/7, 4/9
    assert_eq!(dates.len(), 5);
    assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());
    assert_eq!(dates[4], NaiveDate::from_ymd_opt(2026, 4, 9).unwrap());
}

#[test]
fn test_weekly_interval_2_tuesday() {
    use chrono::NaiveDate;
    // 2026-04-01 is a Wednesday; the rule is Tuesdays with interval=2.
    // 4/7 falls in the week after the start week (3/30-4/5), so delta_weeks=1: no match.
    // 4/14 has delta_weeks=2 and 4/28 has delta_weeks=4: both match.
    let rec = make_rec("weekly", Some(r#"{"days":[2],"interval":2}"#), "2026-04-01");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert_eq!(dates, vec![
        NaiveDate::from_ymd_opt(2026, 4, 14).unwrap(),
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap(),
    ]);
}

#[test]
fn test_monthly_interval_2_bymonthday() {
    use chrono::NaiveDate;
    // The 15th of every other month, starting 2026-04-15.
    let rec = make_rec("monthly", Some(r#"{"days":[15],"interval":2}"#), "2026-04-15");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    // 4/15, 6/15, 8/15, 10/15, 12/15
    assert_eq!(dates.len(), 5);
}

#[test]
fn test_monthly_setpos_2nd_tuesday() {
    use chrono::NaiveDate;
    // Second Tuesday (days=[2], setpos=2).
    let rec = make_rec("monthly", Some(r#"{"days":[2],"setpos":2}"#), "2026-04-01");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    // 4/14, 5/12, 6/9, 7/14
    assert_eq!(dates, vec![
        NaiveDate::from_ymd_opt(2026, 4, 14).unwrap(),
        NaiveDate::from_ymd_opt(2026, 5, 12).unwrap(),
        NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
    ]);
}

#[test]
fn test_monthly_setpos_last_friday() {
    use chrono::NaiveDate;
    // Last Friday (days=[5], setpos=-1).
    let rec = make_rec("monthly", Some(r#"{"days":[5],"setpos":-1}"#), "2026-04-01");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    // 4/24, 5/29, 6/26, 7/31
    assert_eq!(dates, vec![
        NaiveDate::from_ymd_opt(2026, 4, 24).unwrap(),
        NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(),
        NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
    ]);
}

#[test]
fn test_monthly_setpos_last_day_boundary() {
    use chrono::NaiveDate;
    // The last Wednesday must be right across months of different lengths: Jan 2026 (31 days), Feb 2026 (28 days).
    let rec = make_rec("monthly", Some(r#"{"days":[3],"setpos":-1}"#), "2026-01-01");
    let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    // 1/28, 2/25, 3/25
    assert_eq!(dates, vec![
        NaiveDate::from_ymd_opt(2026, 1, 28).unwrap(),
        NaiveDate::from_ymd_opt(2026, 2, 25).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 25).unwrap(),
    ]);
}

#[test]
fn test_target_before_start_date_returns_false() {
    use chrono::NaiveDate;
    let rec = make_rec("daily", None, "2026-04-10");
    let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
    let dates = ytasky::recurrence::generate_dates_for_recurrence(&rec, from, to).unwrap();
    assert!(dates.is_empty(), "expected no dates before start_date, got {dates:?}");
}

// ---- expand_recurrences_to_horizon ---------------------------------------------

#[test]
fn test_expand_to_horizon_daily() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // The horizon is about 2 years ahead (~730 days), today included.
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
    // Mondays (1) and Wednesdays (3).
    insert_recurrence(&mut db, "weekly task", "weekly", Some(r#"{"days":[1,3]}"#), &start_str, None);

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // About 104 weeks with 2 days each: ~208 tasks.
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

    // The horizon has moved forward, so the second run inserts nothing.
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
    // end_date is 3 months from today.
    let end = today + chrono::Months::new(3);
    let end_str = end.format("%Y-%m-%d").to_string();
    insert_recurrence(&mut db, "daily task", "daily", None, &start_str, Some(&end_str));

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // About 90 days (±1).
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

    // Make today and tomorrow exceptions.
    let tomorrow = today + chrono::Duration::days(1);
    ytasky::db::add_recurrence_exception(&mut db, rec_id, &today.format("%Y-%m-%d").to_string()).unwrap();
    ytasky::db::add_recurrence_exception(&mut db, rec_id, &tomorrow.format("%Y-%m-%d").to_string()).unwrap();

    let result = ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();
    // 2 years minus 2 days: ~728 tasks.
    assert!(result.total_inserted >= 698, "inserted: {}", result.total_inserted);
    assert!(result.total_inserted <= 738);
}

// ---- replace_future_tasks ------------------------------------------------------

#[test]
fn test_replace_future_tasks() {
    let _cfg = setup_horizon_env();
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());

    let today = chrono::Local::now().date_naive();
    let start_str = today.format("%Y-%m-%d").to_string();
    let rec_id = insert_recurrence(&mut db, "daily task", "daily", None, &start_str, None);
    ytasky::recurrence::expand_recurrences_to_horizon(&mut db).unwrap();

    // Change the pattern to weekly, Mondays only.
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
    // Removed: the future daily tasks.
    assert!(result.deleted > 100, "deleted: {}", result.deleted);
    // Regenerated: once a week for 2 years, ~104 tasks.
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

    // Give yesterday's task an actual_start so it counts as done.
    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();
    let tasks = ytasky::db::load_tasks(&mut db, &yesterday_str).unwrap();
    let completed = tasks
        .iter()
        .find(|t| t.recurrence_id == Some(rec_id));
    if let Some(t) = completed {
        ytasky::db::update_actual(&mut db, t.id, Some(480), Some(510)).unwrap();
    }

    let result = ytasky::recurrence::replace_future_tasks(&mut db, rec_id).unwrap();
    // Past tasks with an actual_start are kept.
    let tasks_yesterday = ytasky::db::load_tasks(&mut db, &yesterday_str).unwrap();
    let still_exists = tasks_yesterday
        .iter()
        .any(|t| t.recurrence_id == Some(rec_id) && t.actual_start.is_some());
    assert!(still_exists, "completed task should be preserved");
    assert!(result.inserted > 0 || result.deleted > 0);
}

// ---- UndoManager max_size ------------------------------------------------------

#[test]
fn test_undo_manager_max_size() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = setup_db(tmp.path());
    let mut manager = ytasky::history::UndoManager::with_max_size(100);

    // After 101 executes the stack holds 100 entries; the oldest is dropped.
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
