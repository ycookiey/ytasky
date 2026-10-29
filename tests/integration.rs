//! Integration tests: CLI/TUI-equivalent same data directory access
//!
//! Verifies that two Database handles opened on the same directory:
//! 1. Can see inserts from the other handle after refresh
//! 2. Never produce duplicate IDs under concurrent inserts (sequential via 2 handles)

use std::path::Path;
use tempfile::TempDir;
use ybasey::Database;

fn setup_db(data_dir: &Path) -> Database {
    std::fs::write(
        data_dir.join("_meta"),
        "tz: Asia/Tokyo\nview_sync: async\n",
    )
    .unwrap();
    let mut db = Database::open(data_dir, Some("integration-test")).unwrap();
    ytasky::init::apply_schema(&mut db).unwrap();
    db
}

#[test]
fn test_cli_insert_visible_to_another_handle() {
    let tmp = TempDir::new().unwrap();
    let mut db_a = setup_db(tmp.path());

    let mut db_b = Database::open(tmp.path(), Some("handle-b")).unwrap();

    let id = ytasky::db::insert_task(&mut db_a, "2026-04-19", "共有テスト", "1", 30, None).unwrap();
    assert!(id > 0);

    // Handle B sees the insert only after refresh.
    db_b.refresh().unwrap();

    let tasks = ytasky::db::load_tasks(&mut db_b, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 1, "handle B が handle A の insert を refresh 後に参照できる");
    assert_eq!(tasks[0].title, "共有テスト");
}

#[test]
fn test_concurrent_handles_no_duplicate_id() {
    let tmp = TempDir::new().unwrap();
    let mut db_a = setup_db(tmp.path());
    let mut db_b = Database::open(tmp.path(), Some("handle-b")).unwrap();

    let mut ids = Vec::new();

    for i in 0..50 {
        let id = ytasky::db::insert_task(
            &mut db_a,
            "2026-04-19",
            &format!("A-{i}"),
            "1",
            15,
            None,
        )
        .unwrap();
        ids.push(id);
    }

    db_b.refresh().unwrap();
    for i in 0..50 {
        let id = ytasky::db::insert_task(
            &mut db_b,
            "2026-04-19",
            &format!("B-{i}"),
            "1",
            15,
            None,
        )
        .unwrap();
        ids.push(id);
    }

    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        ids.len(),
        sorted.len(),
        "100 inserts (2 handles) で id 重複: {:?}",
        ids
    );

    db_a.refresh().unwrap();
    let tasks = ytasky::db::load_tasks(&mut db_a, "2026-04-19").unwrap();
    assert_eq!(tasks.len(), 100);
}
