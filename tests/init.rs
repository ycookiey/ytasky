use std::path::Path;

fn setup_db(data_dir: &Path) -> ybasey::Database {
    std::fs::write(
        data_dir.join("_meta"),
        "tz: Asia/Tokyo\nview_sync: async\n",
    )
    .unwrap();
    ybasey::Database::open(data_dir, Some("test-init")).unwrap()
}

#[test]
fn test_init_creates_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    assert!(db.has_table("categories"), "categories table missing");
    assert!(db.has_table("tasks"), "tasks table missing");
    assert!(db.has_table("recurrences"), "recurrences table missing");
    assert!(
        db.has_table("recurrence_exceptions"),
        "recurrence_exceptions table missing"
    );

    let result = db.query("categories", "| count").unwrap();
    assert!(result.contains("count=9"), "expected count=9, got: {result}");

    let result = db.query("tasks", "| count").unwrap();
    assert!(result.contains("count=0"), "expected count=0, got: {result}");
}

/// A data dir that already has `.ybasey` counts as initialized.
#[test]
fn test_init_already_exists_error() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    std::fs::create_dir_all(data_dir.join(".ybasey")).unwrap();

    // `run_init` resolves the data dir only through `YTASKY_DATA_DIR`.
    // SAFETY: test-only, single-threaded context within this test binary
    unsafe { std::env::set_var("YTASKY_DATA_DIR", data_dir.to_str().unwrap()) };
    let result = ytasky::init::run_init(false, true);
    unsafe { std::env::remove_var("YTASKY_DATA_DIR") };

    assert!(result.is_err(), "expected error for double init");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("already initialized"),
        "unexpected error message: {msg}"
    );
}

/// Exercises reinitialization through `apply_schema` instead of `run_init`, so the
/// test does not depend on the process-wide `YTASKY_DATA_DIR`.
#[test]
fn test_init_force_reinitializes() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();
    assert!(db.has_table("tasks"), "tasks should exist after first init");
    drop(db);

    // Same steps as `init --force`.
    std::fs::remove_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let mut db2 = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db2).unwrap();

    assert!(db2.has_table("categories"), "categories missing after force reinit");
    assert!(db2.has_table("tasks"), "tasks missing after force reinit");
    assert!(db2.has_table("recurrences"), "recurrences missing after force reinit");
    assert!(
        db2.has_table("recurrence_exceptions"),
        "recurrence_exceptions missing after force reinit"
    );

    let result = db2.query("categories", "| count").unwrap();
    assert!(result.contains("count=9"), "expected count=9 after reinit, got: {result}");
}

#[test]
fn test_migrate_schema_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    // `apply_schema` already adds `external_id`, so both migrations must be no-ops.
    ytasky::init::migrate_schema(&mut db).expect("first migrate should succeed");
    ytasky::init::migrate_schema(&mut db).expect("second migrate should also succeed");

    assert!(
        db.table("tasks").unwrap().schema.has_field("external_id"),
        "tasks.external_id missing"
    );
    assert!(
        db.table("recurrences")
            .unwrap()
            .schema
            .has_field("external_id"),
        "recurrences.external_id missing"
    );
}

#[test]
fn test_external_id_upsert() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    let ext = "gcal:primary:abc123";
    let fields_v1 = vec![
        ("date".to_string(), "2026-05-16".to_string()),
        ("title".to_string(), "Meeting".to_string()),
        ("category_id".to_string(), "3".to_string()),
        ("duration_min".to_string(), "30".to_string()),
        ("status".to_string(), "todo".to_string()),
        ("sort_order".to_string(), "0".to_string()),
        ("is_backlog".to_string(), "0".to_string()),
        ("external_id".to_string(), ext.to_string()),
    ];
    let (id1, inserted1) = db.upsert("tasks", "external_id", ext, fields_v1).unwrap();
    assert!(inserted1, "first upsert should insert");

    // Upserting the same external_id again must update in place and keep the id.
    let fields_v2 = vec![
        ("date".to_string(), "2026-05-16".to_string()),
        ("title".to_string(), "Meeting (updated)".to_string()),
        ("category_id".to_string(), "3".to_string()),
        ("duration_min".to_string(), "45".to_string()),
        ("status".to_string(), "todo".to_string()),
        ("sort_order".to_string(), "0".to_string()),
        ("is_backlog".to_string(), "0".to_string()),
        ("external_id".to_string(), ext.to_string()),
    ];
    let (id2, inserted2) = db.upsert("tasks", "external_id", ext, fields_v2).unwrap();
    assert!(!inserted2, "second upsert should update, not insert");
    assert_eq!(id1, id2, "upsert by external_id must keep id stable");

    let result = db.query("tasks", "| count").unwrap();
    assert!(result.contains("count=1"), "expected count=1, got: {result}");
}

#[test]
fn test_init_insert_and_view() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    // category_id=1 is the first seeded category.
    let record = ybasey::NewRecord::from(vec![
        ("date".to_string(), "2026-04-19".to_string()),
        ("title".to_string(), "Test task".to_string()),
        ("category_id".to_string(), "1".to_string()),
        ("duration_min".to_string(), "30".to_string()),
        ("status".to_string(), "todo".to_string()),
        ("sort_order".to_string(), "0".to_string()),
        ("is_backlog".to_string(), "0".to_string()),
    ]);
    let id = db.insert("tasks", record).unwrap();
    assert_eq!(id, 1, "first task id should be 1");

    let result = db.query("tasks", "| count").unwrap();
    assert!(result.contains("count=1"), "expected count=1, got: {result}");

    // View files are written asynchronously, so poll for up to 500 ms.
    let view_file = data_dir.join("tasks").join("_v.upcoming_tasks");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while !view_file.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        view_file.exists(),
        "view file not generated: {}",
        view_file.display()
    );
}
