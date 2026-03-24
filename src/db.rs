use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

const BACKLOG_DATE: &str = "__backlog__";

/// データディレクトリのパスを返す
fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .context("データディレクトリが見つからない")?
        .join("ytasky");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// DB初期化: テーブル作成 + カテゴリ初期データ
pub fn init() -> Result<Connection> {
    let db_path = data_dir()?.join("ytasky.db");
    let conn = Connection::open(&db_path)?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    create_tables(&conn)?;
    seed_categories(&conn)?;

    Ok(conn)
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS categories (
            id    TEXT PRIMARY KEY,
            name  TEXT NOT NULL,
            icon  TEXT NOT NULL,
            color TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS recurrences (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            title        TEXT NOT NULL,
            category_id  TEXT NOT NULL REFERENCES categories(id),
            duration_min INTEGER NOT NULL,
            fixed_start  INTEGER,
            pattern      TEXT NOT NULL,
            pattern_data TEXT,
            start_date   TEXT NOT NULL,
            end_date     TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            date          TEXT NOT NULL,
            sort_order    INTEGER NOT NULL,
            title         TEXT NOT NULL,
            category_id   TEXT NOT NULL REFERENCES categories(id),
            duration_min  INTEGER NOT NULL,
            fixed_start   INTEGER,
            actual_start  INTEGER,
            actual_end    INTEGER,
            recurrence_id INTEGER REFERENCES recurrences(id),
            is_backlog    INTEGER NOT NULL DEFAULT 0,
            deadline      TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(date, sort_order)
        );

        CREATE TABLE IF NOT EXISTS recurrence_exceptions (
            recurrence_id INTEGER NOT NULL REFERENCES recurrences(id),
            date          TEXT NOT NULL,
            PRIMARY KEY (recurrence_id, date)
        );

        CREATE INDEX IF NOT EXISTS idx_tasks_date ON tasks(date);

        CREATE TABLE IF NOT EXISTS gcal_config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS gcal_calendars (
            calendar_id TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS external_events (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            gcal_event_id  TEXT NOT NULL,
            calendar_id    TEXT NOT NULL,
            date           TEXT NOT NULL,
            title          TEXT NOT NULL,
            start_min      INTEGER,
            duration_min   INTEGER NOT NULL,
            is_all_day     INTEGER NOT NULL DEFAULT 0,
            last_synced    TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(gcal_event_id, date)
        );

        CREATE INDEX IF NOT EXISTS idx_external_events_date ON external_events(date);
        ",
    )?;

    migrate_tasks_table(conn)?;
    Ok(())
}

fn migrate_tasks_table(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if !columns.iter().any(|name| name == "is_backlog") {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN is_backlog INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|name| name == "deadline") {
        conn.execute("ALTER TABLE tasks ADD COLUMN deadline TEXT", [])?;
    }

    Ok(())
}

fn seed_categories(conn: &Connection) -> Result<()> {
    let categories = [
        ("sleep", "睡眠", "󰒲", "blue-grey"),
        ("meal", "食事", "󰩃", "yellow"),
        ("work", "開発・仕事", "󰈙", "pink"),
        ("study", "勉強・講義", "󰑴", "purple"),
        ("exercise", "運動", "󰖏", "green"),
        ("personal", "身支度・自由時間", "\u{f0830}", "orange"),
        ("break", "休憩", "󰾴", "cyan"),
        ("commute", "移動", "󰄋", "red"),
        ("errand", "用事・雑務", "󰃀", "teal"),
    ];

    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO categories (id, name, icon, color) VALUES (?1, ?2, ?3, ?4)",
    )?;

    for (id, name, icon, color) in &categories {
        stmt.execute(rusqlite::params![id, name, icon, color])?;
    }

    Ok(())
}

/// 指定日のタスクを取得
pub fn load_tasks(conn: &Connection, date: &str) -> Result<Vec<crate::model::Task>> {
    generate_recurring_tasks(conn, date)?;

    let mut stmt = conn.prepare(
        "SELECT id, date, sort_order, title, category_id, duration_min,
                fixed_start, actual_start, actual_end, recurrence_id, deadline
         FROM tasks
         WHERE date = ?1 AND is_backlog = 0
         ORDER BY sort_order",
    )?;

    let tasks = stmt
        .query_map(rusqlite::params![date], |row| {
            Ok(crate::model::Task {
                id: row.get(0)?,
                date: row.get(1)?,
                sort_order: row.get(2)?,
                title: row.get(3)?,
                category_id: row.get(4)?,
                duration_min: row.get(5)?,
                fixed_start: row.get(6)?,
                actual_start: row.get(7)?,
                actual_end: row.get(8)?,
                recurrence_id: row.get(9)?,
                deadline: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// バックログタスクを期限順で取得（期限なしは末尾）
pub fn load_backlog_tasks(conn: &Connection) -> Result<Vec<crate::model::Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, date, sort_order, title, category_id, duration_min,
                fixed_start, actual_start, actual_end, recurrence_id, deadline
         FROM tasks
         WHERE is_backlog = 1
         ORDER BY
            CASE WHEN deadline IS NULL OR deadline = '' THEN 1 ELSE 0 END ASC,
            deadline ASC,
            sort_order ASC,
            id ASC",
    )?;

    let tasks = stmt
        .query_map([], |row| {
            Ok(crate::model::Task {
                id: row.get(0)?,
                date: row.get(1)?,
                sort_order: row.get(2)?,
                title: row.get(3)?,
                category_id: row.get(4)?,
                duration_min: row.get(5)?,
                fixed_start: row.get(6)?,
                actual_start: row.get(7)?,
                actual_end: row.get(8)?,
                recurrence_id: row.get(9)?,
                deadline: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// 指定日に該当する繰り返しタスクを遅延生成
pub fn generate_recurring_tasks(conn: &Connection, date: &str) -> Result<()> {
    let target_date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("日付形式が不正です: {date}"))?;

    let mut stmt = conn.prepare(
        "SELECT id, title, category_id, duration_min, fixed_start, pattern, pattern_data
         FROM recurrences
         WHERE start_date <= ?1
           AND (end_date IS NULL OR ?1 <= end_date)
         ORDER BY id",
    )?;

    let recurrences = stmt
        .query_map(params![date], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, Option<i32>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if recurrences.is_empty() {
        return Ok(());
    }

    let mut next_sort_order: i32 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0)
         FROM tasks
         WHERE date = ?1 AND is_backlog = 0",
        params![date],
        |row| row.get(0),
    )?;

    for (recurrence_id, title, category_id, duration_min, fixed_start, pattern, pattern_data) in
        recurrences
    {
        if !matches_recurrence_pattern(&pattern, pattern_data.as_deref(), target_date)? {
            continue;
        }

        let is_exception = conn
            .query_row(
                "SELECT 1 FROM recurrence_exceptions
                 WHERE recurrence_id = ?1 AND date = ?2
                 LIMIT 1",
                params![recurrence_id, date],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if is_exception {
            continue;
        }

        let exists = conn
            .query_row(
                "SELECT 1 FROM tasks
                 WHERE date = ?1 AND recurrence_id = ?2 AND is_backlog = 0
                 LIMIT 1",
                params![date, recurrence_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            continue;
        }

        conn.execute(
            "INSERT INTO tasks
                (date, sort_order, title, category_id, duration_min, fixed_start, recurrence_id)
             VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                date,
                next_sort_order,
                title,
                category_id,
                duration_min,
                fixed_start,
                recurrence_id
            ],
        )?;
        next_sort_order += 1;
    }

    Ok(())
}

/// 繰り返しルールを登録
#[allow(clippy::too_many_arguments)]
pub fn insert_recurrence(
    conn: &Connection,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    pattern: &str,
    pattern_data: Option<&str>,
    start_date: &str,
    end_date: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO recurrences
            (title, category_id, duration_min, fixed_start, pattern, pattern_data, start_date, end_date)
         VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            title,
            category_id,
            duration_min,
            fixed_start,
            pattern,
            pattern_data,
            start_date,
            end_date
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn add_recurrence(
    conn: &Connection,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    pattern: &str,
    pattern_data: Option<&str>,
    start_date: &str,
    end_date: Option<&str>,
) -> Result<i64> {
    insert_recurrence(
        conn,
        title,
        category_id,
        duration_min,
        fixed_start,
        pattern,
        pattern_data,
        start_date,
        end_date,
    )
}

/// 全ての繰り返しルールを取得
pub fn load_recurrences(conn: &Connection) -> Result<Vec<crate::model::Recurrence>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, category_id, duration_min, fixed_start,
                pattern, pattern_data, start_date, end_date
         FROM recurrences
         ORDER BY id",
    )?;

    let recurrences = stmt
        .query_map([], |row| {
            Ok(crate::model::Recurrence {
                id: row.get(0)?,
                title: row.get(1)?,
                category_id: row.get(2)?,
                duration_min: row.get(3)?,
                fixed_start: row.get(4)?,
                pattern: row.get(5)?,
                pattern_data: row.get(6)?,
                start_date: row.get(7)?,
                end_date: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(recurrences)
}

/// 繰り返しルールを更新
#[allow(clippy::too_many_arguments)]
pub fn update_recurrence(
    conn: &Connection,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    pattern: &str,
    pattern_data: Option<&str>,
    start_date: &str,
    end_date: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE recurrences
         SET title = ?2, category_id = ?3, duration_min = ?4,
             fixed_start = ?5, pattern = ?6, pattern_data = ?7, start_date = ?8, end_date = ?9
         WHERE id = ?1",
        params![
            id,
            title,
            category_id,
            duration_min,
            fixed_start,
            pattern,
            pattern_data,
            start_date,
            end_date
        ],
    )?;
    Ok(())
}

/// 繰り返しルールを削除（例外も合わせて削除）
pub fn delete_recurrence(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM recurrence_exceptions WHERE recurrence_id = ?1",
        params![id],
    )?;
    conn.execute(
        "UPDATE tasks SET recurrence_id = NULL WHERE recurrence_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM recurrences WHERE id = ?1", params![id])?;
    Ok(())
}

/// 特定日を繰り返し例外として登録（この日は生成しない）
pub fn add_recurrence_exception(conn: &Connection, recurrence_id: i64, date: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO recurrence_exceptions (recurrence_id, date) VALUES (?1, ?2)",
        params![recurrence_id, date],
    )?;
    Ok(())
}

/// 既存タスクを繰り返しルールに変換し、元タスクへ recurrence_id を紐づける
pub fn create_recurrence_from_task(
    conn: &Connection,
    task_id: i64,
    pattern: &str,
    pattern_data: Option<&str>,
    end_date: Option<&str>,
) -> Result<i64> {
    let task = load_task_by_id(conn, task_id)?.context("タスクが見つからない")?;

    let recurrence_id = insert_recurrence(
        conn,
        &task.title,
        &task.category_id,
        task.duration_min,
        task.fixed_start,
        pattern,
        pattern_data,
        &task.date,
        end_date,
    )?;

    conn.execute(
        "UPDATE tasks SET recurrence_id = ?1 WHERE id = ?2",
        params![recurrence_id, task_id],
    )?;

    Ok(recurrence_id)
}

/// 全カテゴリ取得
pub fn load_categories(conn: &Connection) -> Result<Vec<crate::model::Category>> {
    let mut stmt = conn.prepare("SELECT id, name, icon, color FROM categories ORDER BY rowid")?;
    let cats = stmt
        .query_map([], |row| {
            Ok(crate::model::Category {
                id: row.get(0)?,
                name: row.get(1)?,
                icon: row.get(2)?,
                color: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cats)
}

/// 指定日のカテゴリ別集計を取得
pub fn report_by_category(
    conn: &Connection,
    date: &str,
) -> Result<Vec<crate::model::CategoryReport>> {
    let mut stmt = conn.prepare(
        "SELECT
            c.id,
            c.name,
            COALESCE(SUM(t.duration_min), 0) AS planned_min,
            COALESCE(SUM(
                CASE
                    WHEN t.actual_start IS NOT NULL AND t.actual_end IS NOT NULL
                    THEN t.actual_end - t.actual_start
                    ELSE 0
                END
            ), 0) AS actual_min
         FROM categories c
         LEFT JOIN tasks t
           ON t.category_id = c.id
          AND t.date = ?1
          AND t.is_backlog = 0
         GROUP BY c.id, c.name
         HAVING COALESCE(SUM(t.duration_min), 0) > 0
             OR COALESCE(SUM(
                CASE
                    WHEN t.actual_start IS NOT NULL AND t.actual_end IS NOT NULL
                    THEN t.actual_end - t.actual_start
                    ELSE 0
                END
             ), 0) > 0
         ORDER BY c.rowid",
    )?;

    let rows = stmt
        .query_map(params![date], |row| {
            Ok(crate::model::CategoryReport {
                category_id: row.get(0)?,
                category_name: row.get(1)?,
                planned_min: row.get(2)?,
                actual_min: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// 指定日のタイトル別集計を取得
pub fn report_by_title(conn: &Connection, date: &str) -> Result<Vec<crate::model::TitleReport>> {
    let mut stmt = conn.prepare(
        "SELECT
            t.title,
            t.category_id,
            COALESCE(SUM(t.duration_min), 0) AS planned_min,
            COALESCE(SUM(
                CASE
                    WHEN t.actual_start IS NOT NULL AND t.actual_end IS NOT NULL
                    THEN t.actual_end - t.actual_start
                    ELSE 0
                END
            ), 0) AS actual_min
         FROM tasks t
         WHERE t.date = ?1
           AND t.is_backlog = 0
         GROUP BY t.title, t.category_id
         ORDER BY planned_min DESC, t.title ASC",
    )?;

    let rows = stmt
        .query_map(params![date], |row| {
            Ok(crate::model::TitleReport {
                title: row.get(0)?,
                category_id: row.get(1)?,
                planned_min: row.get(2)?,
                actual_min: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// タスク挿入
pub fn insert_task(
    conn: &Connection,
    date: &str,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
) -> Result<i64> {
    insert_task_with_deadline(
        conn,
        date,
        title,
        category_id,
        duration_min,
        fixed_start,
        None,
    )
}

/// 期限付きタスク挿入（通常タスク）
pub fn insert_task_with_deadline(
    conn: &Connection,
    date: &str,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<i64> {
    let sort_order: i32 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0)
         FROM tasks
         WHERE date = ?1 AND is_backlog = 0",
        params![date],
        |row| row.get(0),
    )?;

    conn.execute(
        "INSERT INTO tasks
            (date, sort_order, title, category_id, duration_min, fixed_start, is_backlog, deadline)
         VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![
            date,
            sort_order,
            title,
            category_id,
            duration_min,
            fixed_start,
            deadline
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// タスク更新
pub fn update_task(
    conn: &Connection,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
) -> Result<()> {
    let current_deadline = conn
        .query_row(
            "SELECT deadline FROM tasks WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();

    update_task_with_deadline(
        conn,
        id,
        title,
        category_id,
        duration_min,
        fixed_start,
        current_deadline.as_deref(),
    )
}

/// 期限付きタスク更新
pub fn update_task_with_deadline(
    conn: &Connection,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE tasks
         SET title = ?2, category_id = ?3, duration_min = ?4, fixed_start = ?5, deadline = ?6
         WHERE id = ?1",
        params![id, title, category_id, duration_min, fixed_start, deadline],
    )?;
    Ok(())
}

/// バックログに新規追加
pub fn insert_backlog_task(
    conn: &Connection,
    title: &str,
    category_id: &str,
    duration_min: i32,
    deadline: Option<&str>,
) -> Result<i64> {
    let sort_order: i32 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM tasks WHERE is_backlog = 1",
        [],
        |row| row.get(0),
    )?;

    conn.execute(
        "INSERT INTO tasks
            (date, sort_order, title, category_id, duration_min, fixed_start, is_backlog, deadline)
         VALUES
            (?1, ?2, ?3, ?4, ?5, NULL, 1, ?6)",
        params![
            BACKLOG_DATE,
            sort_order,
            title,
            category_id,
            duration_min,
            deadline
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// タスクをバックログ化/解除
pub fn set_backlog_flag(conn: &Connection, task_id: i64, is_backlog: bool) -> Result<()> {
    if is_backlog {
        let current_date = conn
            .query_row(
                "SELECT date FROM tasks WHERE id = ?1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(current_date) = current_date else {
            return Ok(());
        };

        let sort_order: i32 = conn.query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM tasks WHERE is_backlog = 1",
            [],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE tasks
             SET is_backlog = 1, date = ?2, sort_order = ?3, actual_start = NULL, actual_end = NULL
             WHERE id = ?1",
            params![task_id, BACKLOG_DATE, sort_order],
        )?;

        if current_date != BACKLOG_DATE {
            normalize_sort_order(conn, &current_date)?;
        }
        normalize_backlog_sort_order(conn)?;
        return Ok(());
    }

    conn.execute(
        "UPDATE tasks SET is_backlog = 0 WHERE id = ?1",
        params![task_id],
    )?;
    Ok(())
}

/// バックログタスクを指定日の指定位置に挿入
pub fn insert_backlog_task_at(
    conn: &Connection,
    task_id: i64,
    date: &str,
    index: usize,
) -> Result<()> {
    let count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE date = ?1 AND is_backlog = 0",
        params![date],
        |row| row.get(0),
    )?;
    let target = (index as i32).clamp(0, count);

    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1
         WHERE date = ?1 AND is_backlog = 0 AND sort_order >= ?2",
        params![date, target],
    )?;

    conn.execute(
        "UPDATE tasks
         SET is_backlog = 0, date = ?2, sort_order = ?3, actual_start = NULL, actual_end = NULL
         WHERE id = ?1",
        params![task_id, date, target],
    )?;

    normalize_backlog_sort_order(conn)?;
    normalize_sort_order(conn, date)?;
    Ok(())
}

/// バックログタスクを末尾に挿入
pub fn append_backlog_task(conn: &Connection, task_id: i64, date: &str) -> Result<()> {
    let count: i32 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE date = ?1 AND is_backlog = 0",
        params![date],
        |row| row.get(0),
    )?;
    insert_backlog_task_at(conn, task_id, date, count as usize)
}

/// タスク削除
pub fn delete_task(conn: &Connection, id: i64) -> Result<()> {
    let meta = conn
        .query_row(
            "SELECT date, is_backlog FROM tasks WHERE id = ?1",
            params![id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0)),
        )
        .optional()?;

    conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;

    if let Some((date, is_backlog)) = meta {
        if is_backlog {
            normalize_backlog_sort_order(conn)?;
        } else {
            normalize_sort_order(conn, &date)?;
        }
    }

    Ok(())
}

/// sort_order入れ替え
pub fn swap_sort_order(conn: &Connection, id1: i64, id2: i64) -> Result<()> {
    if id1 == id2 {
        return Ok(());
    }

    let first = conn
        .query_row(
            "SELECT date, sort_order, is_backlog FROM tasks WHERE id = ?1",
            params![id1],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, i32>(2)? != 0,
                ))
            },
        )
        .optional()?;
    let second = conn
        .query_row(
            "SELECT date, sort_order, is_backlog FROM tasks WHERE id = ?1",
            params![id2],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, i32>(2)? != 0,
                ))
            },
        )
        .optional()?;

    let (date1, sort1, backlog1) = match first {
        Some(v) => v,
        None => return Ok(()),
    };
    let (date2, sort2, backlog2) = match second {
        Some(v) => v,
        None => return Ok(()),
    };

    if backlog1 || backlog2 || date1 != date2 {
        return Ok(());
    }

    let tmp_sort: i32 = conn.query_row(
        "SELECT COALESCE(MIN(sort_order), 0) - 1
         FROM tasks
         WHERE date = ?1 AND is_backlog = 0",
        params![date1.as_str()],
        |row| row.get(0),
    )?;

    conn.execute(
        "UPDATE tasks SET sort_order = ?1 WHERE id = ?2",
        params![tmp_sort, id1],
    )?;
    conn.execute(
        "UPDATE tasks SET sort_order = ?1 WHERE id = ?2",
        params![sort1, id2],
    )?;
    conn.execute(
        "UPDATE tasks SET sort_order = ?1 WHERE id = ?2",
        params![sort2, id1],
    )?;

    normalize_sort_order(conn, &date1)?;
    Ok(())
}

/// 実績時刻更新
pub fn update_actual(
    conn: &Connection,
    id: i64,
    actual_start: Option<i32>,
    actual_end: Option<i32>,
) -> Result<()> {
    conn.execute(
        "UPDATE tasks SET actual_start = ?2, actual_end = ?3 WHERE id = ?1",
        params![id, actual_start, actual_end],
    )?;
    Ok(())
}

/// タスクIDで1件取得
pub fn load_task_by_id(conn: &Connection, id: i64) -> Result<Option<crate::model::Task>> {
    conn.query_row(
        "SELECT id, date, sort_order, title, category_id, duration_min,
                fixed_start, actual_start, actual_end, recurrence_id, deadline
         FROM tasks
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(crate::model::Task {
                id: row.get(0)?,
                date: row.get(1)?,
                sort_order: row.get(2)?,
                title: row.get(3)?,
                category_id: row.get(4)?,
                duration_min: row.get(5)?,
                fixed_start: row.get(6)?,
                actual_start: row.get(7)?,
                actual_end: row.get(8)?,
                recurrence_id: row.get(9)?,
                deadline: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// タスクの位置情報（date, sort_order, is_backlog）を取得
pub fn load_task_position(conn: &Connection, id: i64) -> Result<Option<(String, i32, bool)>> {
    conn.query_row(
        "SELECT date, sort_order, is_backlog FROM tasks WHERE id = ?1",
        params![id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)? != 0,
            ))
        },
    )
    .optional()
    .map_err(Into::into)
}

/// タスクをスケジュール側へ戻す（Undo用）
pub fn restore_task_to_schedule(
    conn: &Connection,
    task_id: i64,
    date: &str,
    sort_order: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1
         WHERE date = ?1 AND is_backlog = 0 AND sort_order >= ?2",
        params![date, sort_order],
    )?;
    conn.execute(
        "UPDATE tasks
         SET is_backlog = 0, date = ?2, sort_order = ?3
         WHERE id = ?1",
        params![task_id, date, sort_order],
    )?;
    normalize_sort_order(conn, date)?;
    normalize_backlog_sort_order(conn)?;
    Ok(())
}

/// タスクをバックログ側へ戻す（Undo用）
pub fn restore_task_to_backlog(conn: &Connection, task_id: i64, sort_order: i32) -> Result<()> {
    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1
         WHERE is_backlog = 1 AND sort_order >= ?1",
        params![sort_order],
    )?;
    conn.execute(
        "UPDATE tasks
         SET is_backlog = 1, date = ?2, sort_order = ?3, actual_start = NULL, actual_end = NULL
         WHERE id = ?1",
        params![task_id, BACKLOG_DATE, sort_order],
    )?;
    normalize_backlog_sort_order(conn)?;
    Ok(())
}

fn normalize_sort_order(conn: &Connection, date: &str) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id
         FROM tasks
         WHERE date = ?1 AND is_backlog = 0
         ORDER BY sort_order, id ASC",
    )?;
    let ids = stmt
        .query_map(params![date], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if ids.is_empty() {
        return Ok(());
    }

    // UNIQUE(date, sort_order)を壊さないよう一旦オフセット
    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1000000
         WHERE date = ?1 AND is_backlog = 0",
        params![date],
    )?;

    for (idx, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE tasks SET sort_order = ?1 WHERE id = ?2",
            params![idx as i32, id],
        )?;
    }
    Ok(())
}

fn normalize_backlog_sort_order(conn: &Connection) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT id FROM tasks WHERE is_backlog = 1 ORDER BY sort_order, id ASC")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if ids.is_empty() {
        return Ok(());
    }

    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1000000
         WHERE is_backlog = 1",
        [],
    )?;

    for (idx, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE tasks SET sort_order = ?1 WHERE id = ?2",
            params![idx as i32, id],
        )?;
    }
    Ok(())
}

fn matches_recurrence_pattern(
    pattern: &str,
    pattern_data: Option<&str>,
    target_date: NaiveDate,
) -> Result<bool> {
    match pattern {
        "daily" => Ok(true),
        "weekly" => {
            let weekday = target_date.weekday().number_from_monday() as u8;
            let days = parse_pattern_days(pattern_data)?;
            Ok(days.contains(&weekday))
        }
        "monthly" => {
            let day = target_date.day() as u8;
            let days = parse_pattern_days(pattern_data)?;
            Ok(days.contains(&day))
        }
        _ => Ok(false),
    }
}

fn parse_pattern_days(pattern_data: Option<&str>) -> Result<Vec<u8>> {
    let Some(raw) = pattern_data else {
        return Ok(Vec::new());
    };

    let parsed = serde_json::from_str::<crate::model::PatternData>(raw)
        .with_context(|| format!("pattern_dataのJSONが不正です: {raw}"))?;
    Ok(parsed.days.unwrap_or_default())
}

// --- Google Calendar ---

pub fn gcal_get_config(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM gcal_config WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn gcal_set_config(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO gcal_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn gcal_is_configured(conn: &Connection) -> Result<bool> {
    Ok(gcal_get_config(conn, "refresh_token")?.is_some())
}

pub fn gcal_upsert_calendar(
    conn: &Connection,
    calendar_id: &str,
    name: &str,
    enabled: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO gcal_calendars (calendar_id, name, enabled) VALUES (?1, ?2, ?3)
         ON CONFLICT(calendar_id) DO UPDATE SET name = excluded.name",
        params![calendar_id, name, enabled as i32],
    )?;
    Ok(())
}

pub fn gcal_load_calendars(conn: &Connection) -> Result<Vec<crate::model::GCalCalendar>> {
    let mut stmt =
        conn.prepare("SELECT calendar_id, name, enabled FROM gcal_calendars ORDER BY name")?;
    let calendars = stmt
        .query_map([], |row| {
            Ok(crate::model::GCalCalendar {
                calendar_id: row.get(0)?,
                name: row.get(1)?,
                enabled: row.get::<_, i32>(2)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(calendars)
}

pub fn gcal_set_calendar_enabled(
    conn: &Connection,
    calendar_id: &str,
    enabled: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE gcal_calendars SET enabled = ?2 WHERE calendar_id = ?1",
        params![calendar_id, enabled as i32],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn gcal_upsert_event(
    conn: &Connection,
    gcal_event_id: &str,
    calendar_id: &str,
    date: &str,
    title: &str,
    start_min: Option<i32>,
    duration_min: i32,
    is_all_day: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO external_events
            (gcal_event_id, calendar_id, date, title, start_min, duration_min, is_all_day, last_synced)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
         ON CONFLICT(gcal_event_id, date) DO UPDATE SET
            calendar_id = excluded.calendar_id,
            title       = excluded.title,
            start_min   = excluded.start_min,
            duration_min = excluded.duration_min,
            is_all_day  = excluded.is_all_day,
            last_synced = excluded.last_synced",
        params![
            gcal_event_id,
            calendar_id,
            date,
            title,
            start_min,
            duration_min,
            is_all_day as i32
        ],
    )?;
    Ok(())
}

pub fn gcal_load_events(
    conn: &Connection,
    date: &str,
) -> Result<Vec<crate::model::ExternalEvent>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.gcal_event_id, e.calendar_id, e.date, e.title,
                e.start_min, e.duration_min, e.is_all_day
         FROM external_events e
         JOIN gcal_calendars c ON e.calendar_id = c.calendar_id AND c.enabled = 1
         WHERE e.date = ?1
         ORDER BY e.is_all_day DESC, e.start_min ASC, e.title ASC",
    )?;
    let events = stmt
        .query_map(params![date], |row| {
            Ok(crate::model::ExternalEvent {
                id: row.get(0)?,
                gcal_event_id: row.get(1)?,
                calendar_id: row.get(2)?,
                date: row.get(3)?,
                title: row.get(4)?,
                start_min: row.get(5)?,
                duration_min: row.get(6)?,
                is_all_day: row.get::<_, i32>(7)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(events)
}

pub fn gcal_delete_stale_events(
    conn: &Connection,
    calendar_id: &str,
    date: &str,
    current_event_ids: &[String],
) -> Result<usize> {
    if current_event_ids.is_empty() {
        let deleted = conn.execute(
            "DELETE FROM external_events WHERE calendar_id = ?1 AND date = ?2",
            params![calendar_id, date],
        )?;
        return Ok(deleted);
    }
    let placeholders: Vec<String> = (0..current_event_ids.len())
        .map(|i| format!("?{}", i + 3))
        .collect();
    let sql = format!(
        "DELETE FROM external_events
         WHERE calendar_id = ?1 AND date = ?2
           AND gcal_event_id NOT IN ({})",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(calendar_id.to_string()));
    param_values.push(Box::new(date.to_string()));
    for id in current_event_ids {
        param_values.push(Box::new(id.clone()));
    }
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let deleted = stmt.execute(params_ref.as_slice())?;
    Ok(deleted)
}

pub fn gcal_clear_all(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM external_events;
         DELETE FROM gcal_calendars;
         DELETE FROM gcal_config;",
    )?;
    Ok(())
}
