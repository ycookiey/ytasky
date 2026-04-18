use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// ybasey data dir を解決する。
/// $YTASKY_DATA_DIR > OS default (~/.local/share/ytasky-ybasey 等)。
/// 既存 rusqlite DB は ~/.local/share/ytasky/ytasky.db のため、
/// ytasky-ybasey を別ディレクトリに配置して衝突を回避する。
fn resolve_data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YTASKY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_dir().context("OS data dir not found")?;
    Ok(base.join("ytasky-ybasey"))
}

/// `ytasky init` エントリポイント。
/// force=true のとき既存 data_dir を削除して再作成。
/// yes=true のとき確認プロンプトを skip。
pub fn run_init(force: bool, yes: bool) -> Result<()> {
    let data_dir = resolve_data_dir()?;

    // 既存 schema 検出
    let ybasey_dir = data_dir.join(".ybasey");
    if ybasey_dir.exists() && !force {
        anyhow::bail!(
            "ytasky: already initialized at {}. Use --force to reinitialize.",
            data_dir.display()
        );
    }

    // 確認プロンプト
    if !yes {
        eprint!(
            "Initialize ybasey schema at {}? [y/N] ",
            data_dir.display()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            std::process::exit(1);
        }
    }

    // --force: 既存 dir を削除して再作成
    if force && data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)?;
    }
    std::fs::create_dir_all(&data_dir)?;

    // 1. _meta を先に書く (Database::open が読む)
    write_meta(&data_dir)?;

    // 2. Database::open
    let mut db = ybasey::Database::open(&data_dir, Some("ytasky-init"))?;

    // 3. schema 投入
    apply_schema(&mut db)?;

    eprintln!("ytasky: initialized at {}", data_dir.display());
    Ok(())
}

/// `_meta` ファイルを data_dir 直下に書き出す。
/// ybasey には _meta write API が無いため Database::open 前に直接ファイル書込する。
fn write_meta(data_dir: &Path) -> Result<()> {
    let meta_path = data_dir.join("_meta");
    std::fs::write(&meta_path, "tz: Asia/Tokyo\nview_sync: async\n")?;
    Ok(())
}

/// ybasey Database に ytasky schema を投入する。
/// テストから直接呼べるように pub。
/// 投入順序: categories → recurrences → recurrence_exceptions → tasks (ref 先が先に存在する必要)
pub fn apply_schema(db: &mut ybasey::Database) -> Result<()> {
    // ---- categories ----
    db.create_table("categories")?;
    db.add_field("categories", "name", "str", false)?;
    db.add_field("categories", "color", "str", false)?;
    db.add_field("categories", "icon", "str", false)?;

    // seed 9 カテゴリ投入
    seed_categories(db)?;

    // ---- recurrences ----
    db.create_table("recurrences")?;
    db.add_field("recurrences", "title", "str", false)?;
    db.add_field(
        "recurrences",
        "category_id",
        "ref(categories) on-delete=set-null",
        true,
    )?;
    db.add_field("recurrences", "duration_min", "int", false)?;
    db.add_field("recurrences", "fixed_start", "int", true)?;
    db.add_field("recurrences", "pattern", "str", false)?;
    db.add_field("recurrences", "pattern_data", "str", true)?;
    db.add_field("recurrences", "start_date", "date", false)?;
    db.add_field("recurrences", "end_date", "date", true)?;

    // ---- recurrence_exceptions ----
    db.create_table("recurrence_exceptions")?;
    db.add_field(
        "recurrence_exceptions",
        "recurrence_id",
        "ref(recurrences) on-delete=cascade",
        false,
    )?;
    db.add_field("recurrence_exceptions", "exception_date", "date", false)?;
    db.add_field("recurrence_exceptions", "reason", "str", true)?;

    // ---- tasks ----
    db.create_table("tasks")?;
    db.add_field("tasks", "date", "date", false)?;
    db.add_field("tasks", "title", "str", false)?;
    db.add_field(
        "tasks",
        "category_id",
        "ref(categories) on-delete=set-null",
        true,
    )?;
    db.add_field("tasks", "duration_min", "int", false)?;
    db.add_field("tasks", "fixed_start", "int", true)?;
    db.add_field("tasks", "actual_start", "int", true)?;
    db.add_field("tasks", "actual_end", "int", true)?;
    db.add_field("tasks", "status", "enum(todo,doing,done)", false)?;
    db.add_field("tasks", "sort_order", "int", false)?;
    db.add_field(
        "tasks",
        "recurrence_id",
        "ref(recurrences) on-delete=set-null",
        true,
    )?;
    db.add_field("tasks", "note", "str", true)?;

    // ---- views ----
    apply_views(db)?;

    Ok(())
}

/// 9 つの seed カテゴリを挿入する。
/// insert 順序で id=1..=9 が割り当てられる。この順序は固定。
fn seed_categories(db: &mut ybasey::Database) -> Result<()> {
    // (name, icon, color) の順 — icon は db.rs の既存 seed と同一
    let categories: &[(&str, &str, &str)] = &[
        ("睡眠", "󰒲", "blue-grey"),
        ("食事", "󰩃", "yellow"),
        ("開発・仕事", "󰈙", "pink"),
        ("勉強・講義", "󰑴", "purple"),
        ("運動", "󰖏", "green"),
        ("身支度・自由時間", "\u{f0830}", "orange"),
        ("休憩", "󰾴", "cyan"),
        ("移動", "󰄋", "red"),
        ("用事・雑務", "󰃀", "teal"),
    ];

    for (name, icon, color) in categories {
        let record = ybasey::NewRecord::from(vec![
            ("name".to_string(), name.to_string()),
            ("color".to_string(), color.to_string()),
            ("icon".to_string(), icon.to_string()),
        ]);
        db.insert("categories", record)?;
    }
    Ok(())
}

/// ytasky 用 view 定義を投入する。
fn apply_views(db: &mut ybasey::Database) -> Result<()> {
    // report_by_category: カテゴリ別の計画時間合計とタスク数
    // count の alias 変更は DSL 非サポートのため count のまま
    db.add_view(
        "tasks",
        "report_by_category",
        "group by category_id | sum(duration_min) as planned, count",
    )?;

    // report_by_date: 日付別の計画時間合計とタスク数 (日付昇順)
    db.add_view(
        "tasks",
        "report_by_date",
        "group by date | sum(duration_min) as total_min, count sort by date asc",
    )?;

    // upcoming_tasks: 未着手タスクを日付・sort_order 昇順で一覧
    db.add_view(
        "tasks",
        "upcoming_tasks",
        r#"status == "todo" sort by date asc, sort_order asc"#,
    )?;

    Ok(())
}

/// ybasey schema が存在するか check する。
/// 存在しなければ stderr に案内を出して exit(1)。
/// 本 phase では未使用 (phase 3-4 で各起動パスに挿入)。
#[allow(dead_code)]
pub fn ensure_schema_exists() -> Result<()> {
    let data_dir = resolve_data_dir()?;
    let ybasey_dir = data_dir.join(".ybasey");
    if !ybasey_dir.exists() {
        eprintln!("ytasky: database not initialized. Run 'ytasky init' first.");
        std::process::exit(1);
    }
    // table 存在確認
    let db = ybasey::Database::open(&data_dir, Some("ytasky-check"))?;
    let required = ["categories", "tasks", "recurrences", "recurrence_exceptions"];
    for table in &required {
        if !db.has_table(table) {
            eprintln!(
                "ytasky: schema incomplete (missing table '{table}'). Run 'ytasky init --force' to reinitialize."
            );
            std::process::exit(1);
        }
    }
    Ok(())
}
