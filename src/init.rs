use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolves the data dir: `$YTASKY_DATA_DIR`, else a folder in the OS data dir.
/// The folder name differs from the legacy SQLite data folder (`ytasky/`) so both can coexist.
fn resolve_data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YTASKY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_dir().context("OS data dir not found")?;
    Ok(base.join("ytasky-ybasey"))
}

/// Entry point of `ytasky init`.
/// `force` deletes and recreates an existing data dir; `yes` skips the confirmation prompt.
pub fn run_init(force: bool, yes: bool) -> Result<()> {
    let data_dir = resolve_data_dir()?;

    // Detect an existing schema
    let ybasey_dir = data_dir.join(".ybasey");
    if ybasey_dir.exists() && !force {
        anyhow::bail!(
            "ytasky: already initialized at {}. Use --force to reinitialize.",
            data_dir.display()
        );
    }

    // Confirmation prompt
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

    // --force: delete the existing dir and recreate it
    if force && data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)?;
    }
    std::fs::create_dir_all(&data_dir)?;

    // 1. Write _meta first: Database::open reads it
    write_meta(&data_dir)?;

    // 2. Database::open
    let mut db = ybasey::Database::open(&data_dir, Some("ytasky-init"))?;

    // 3. Apply the schema
    apply_schema(&mut db)?;

    eprintln!("ytasky: initialized at {}", data_dir.display());
    Ok(())
}

/// Writes the `_meta` file into the data dir.
/// ybasey has no API to write _meta, so the file is written directly before Database::open.
fn write_meta(data_dir: &Path) -> Result<()> {
    let meta_path = data_dir.join("_meta");
    std::fs::write(&meta_path, "tz: Asia/Tokyo\nview_sync: async\n")?;
    Ok(())
}

/// Applies the ytasky schema to the database.
/// Public so that tests can call it directly.
/// Order: categories -> recurrences -> recurrence_exceptions -> tasks (a ref target must exist first)
pub fn apply_schema(db: &mut ybasey::Database) -> Result<()> {
    // ---- categories ----
    db.create_table("categories")?;
    db.add_field("categories", "name", "str", false)?;
    db.add_field("categories", "color", "str", false)?;
    db.add_field("categories", "icon", "str", false)?;

    // Seed the 9 categories
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
    db.add_field("recurrences", "external_id", "str", true)?;

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
    db.add_field("tasks", "is_backlog", "int", false)?;
    db.add_field("tasks", "deadline", "str", true)?;
    db.add_field("tasks", "external_id", "str", true)?;

    // ---- views ----
    apply_views(db)?;

    Ok(())
}

/// Inserts the 9 seed categories.
/// The insert order assigns ids 1..=9; keep the order fixed.
fn seed_categories(db: &mut ybasey::Database) -> Result<()> {
    // (name, icon, color)
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

/// Adds the ytasky view definitions.
fn apply_views(db: &mut ybasey::Database) -> Result<()> {
    // report_by_category: planned minutes and task count per category
    // The DSL cannot alias count, so the column stays "count"
    db.add_view(
        "tasks",
        "report_by_category",
        "group by category_id | sum(duration_min) as planned, count",
    )?;

    // report_by_date: planned minutes and task count per date (date ascending)
    db.add_view(
        "tasks",
        "report_by_date",
        "group by date | sum(duration_min) as total_min, count sort by date asc",
    )?;

    // upcoming_tasks: todo tasks by date and sort_order ascending
    db.add_view(
        "tasks",
        "upcoming_tasks",
        r#"status == "todo" sort by date asc, sort_order asc"#,
    )?;

    Ok(())
}

/// Applies schema additions to an existing database.
/// A fresh init gets everything from `apply_schema`; this adds, idempotently, what older databases lack.
pub fn migrate_schema(db: &mut ybasey::Database) -> Result<()> {
    add_field_if_absent(db, "tasks", "external_id", "str", true)?;
    add_field_if_absent(db, "recurrences", "external_id", "str", true)?;
    Ok(())
}

fn add_field_if_absent(
    db: &mut ybasey::Database,
    table: &str,
    name: &str,
    type_spec: &str,
    nullable: bool,
) -> Result<()> {
    if db.table(table)?.schema.has_field(name) {
        return Ok(());
    }
    db.add_field(table, name, type_spec, nullable)?;
    Ok(())
}

/// Checks that the schema exists.
/// If it does not, prints a hint to stderr and exits with 1.
/// Currently unused.
#[allow(dead_code)]
pub fn ensure_schema_exists() -> Result<()> {
    let data_dir = resolve_data_dir()?;
    let ybasey_dir = data_dir.join(".ybasey");
    if !ybasey_dir.exists() {
        eprintln!("ytasky: database not initialized. Run 'ytasky init' first.");
        std::process::exit(1);
    }
    // Check that the tables exist
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
