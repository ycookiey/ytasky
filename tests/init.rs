/// ytasky init.rs の integration test
/// Sub-task 5: 4 ケース
use std::path::Path;

/// テスト用 data_dir に _meta を書いて Database を開く helper
fn setup_db(data_dir: &Path) -> ybasey::Database {
    std::fs::write(
        data_dir.join("_meta"),
        "tz: Asia/Tokyo\nview_sync: async\n",
    )
    .unwrap();
    ybasey::Database::open(data_dir, Some("test-init")).unwrap()
}

/// ケース1: init 成功 — schema と seed データが正しく作成される
#[test]
fn test_init_creates_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    // 全 table 存在確認
    assert!(db.has_table("categories"), "categories table missing");
    assert!(db.has_table("tasks"), "tasks table missing");
    assert!(db.has_table("recurrences"), "recurrences table missing");
    assert!(
        db.has_table("recurrence_exceptions"),
        "recurrence_exceptions table missing"
    );

    // categories seed データ 9 件
    let result = db.query("categories", "| count").unwrap();
    assert!(result.contains("count=9"), "expected count=9, got: {result}");

    // tasks は空
    let result = db.query("tasks", "| count").unwrap();
    assert!(result.contains("count=0"), "expected count=0, got: {result}");
}

/// ケース2: init --force なしの二重 init はエラー
/// (.ybasey dir が存在する = 初期化済みとして扱う)
/// run_init は env var を使うため set/remove_var は unsafe ブロックで囲む
#[test]
fn test_init_already_exists_error() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // .ybasey dir を作成して init 済み状態を模擬
    std::fs::create_dir_all(data_dir.join(".ybasey")).unwrap();

    // YTASKY_DATA_DIR を override して run_init を呼ぶ
    // force=false → already initialized エラーになること
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

/// ケース3: --force 相当 — 既存 schema を削除して再作成できる
/// run_init の env var 競合を避けるため apply_schema レベルで force reinit を検証する
#[test]
fn test_init_force_reinitializes() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // 最初の schema 投入
    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();
    assert!(db.has_table("tasks"), "tasks should exist after first init");
    drop(db);

    // data_dir を削除して再作成 (--force 相当)
    std::fs::remove_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    // 再 init
    let mut db2 = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db2).unwrap();

    // schema が再作成されている
    assert!(db2.has_table("categories"), "categories missing after force reinit");
    assert!(db2.has_table("tasks"), "tasks missing after force reinit");
    assert!(db2.has_table("recurrences"), "recurrences missing after force reinit");
    assert!(
        db2.has_table("recurrence_exceptions"),
        "recurrence_exceptions missing after force reinit"
    );

    // seed data 再投入確認
    let result = db2.query("categories", "| count").unwrap();
    assert!(result.contains("count=9"), "expected count=9 after reinit, got: {result}");
}

/// ケース4: sample record CRUD + view file 生成確認
#[test]
fn test_init_insert_and_view() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let mut db = setup_db(&data_dir);
    ytasky::init::apply_schema(&mut db).unwrap();

    // sample task insert (category_id=1: 睡眠)
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

    // query で 1 件
    let result = db.query("tasks", "| count").unwrap();
    assert!(result.contains("count=1"), "expected count=1, got: {result}");

    // view file が async で生成される (最大 500ms 待機)
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
