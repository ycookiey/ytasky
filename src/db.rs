//! ytasky database layer — wraps ybasey::Database
//!
//! Sub-task 1: skeleton (all functions are todo!() stubs).
//! Sub-task 2+: stubs will be replaced one by one.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::PathBuf;
use ybasey::{Database, NewRecord, Op};

/// backlog タスクの sentinel date (date 型非互換の "__backlog__" の代替)
const BACKLOG_DATE: &str = "9999-12-31";

// ---- 型変換 helper (Value → primitive) ----------------------------------------

/// Value::Int(i64) を取り出す。なければ 0。
fn get_int(r: &ybasey::engine::Record, field: &str) -> i64 {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => *v,
        _ => 0,
    }
}

/// Value::Str(s) を取り出す。なければ "".
fn get_str(r: &ybasey::engine::Record, field: &str) -> String {
    match r.get(field) {
        Some(ybasey::schema::Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// nullable int field。Null または未定義 → None。
fn get_opt_int(r: &ybasey::engine::Record, field: &str) -> Option<i64> {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => Some(*v),
        _ => None,
    }
}

/// nullable str field。Null または未定義 → None。
fn get_opt_str(r: &ybasey::engine::Record, field: &str) -> Option<String> {
    match r.get(field) {
        Some(ybasey::schema::Value::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Ref 型 field を u64 id → String に変換。Value::Int で格納されている。
fn get_ref_id_str(r: &ybasey::engine::Record, field: &str) -> String {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => v.to_string(),
        _ => String::new(),
    }
}

/// nullable Ref 型 field。Value::Int → Some(id), Null → None。
fn get_opt_ref_id(r: &ybasey::engine::Record, field: &str) -> Option<i64> {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => Some(*v),
        _ => None,
    }
}

// ---- Record → model struct 変換 ------------------------------------------------

pub(crate) fn record_to_task(r: &ybasey::engine::Record) -> crate::model::Task {
    crate::model::Task {
        id: r.id as i64,
        date: get_str(r, "date"),
        sort_order: get_int(r, "sort_order") as i32,
        title: get_str(r, "title"),
        category_id: get_ref_id_str(r, "category_id"),
        duration_min: get_int(r, "duration_min") as i32,
        fixed_start: get_opt_int(r, "fixed_start").map(|v| v as i32),
        actual_start: get_opt_int(r, "actual_start").map(|v| v as i32),
        actual_end: get_opt_int(r, "actual_end").map(|v| v as i32),
        recurrence_id: get_opt_ref_id(r, "recurrence_id"),
        is_backlog: get_int(r, "is_backlog") != 0,
        deadline: get_opt_str(r, "deadline"),
    }
}

fn record_to_category(r: &ybasey::engine::Record) -> crate::model::Category {
    crate::model::Category {
        id: r.id.to_string(),
        name: get_str(r, "name"),
        icon: get_str(r, "icon"),
        color: get_str(r, "color"),
    }
}

fn record_to_recurrence(r: &ybasey::engine::Record) -> crate::model::Recurrence {
    crate::model::Recurrence {
        id: r.id as i64,
        title: get_str(r, "title"),
        category_id: get_ref_id_str(r, "category_id"),
        duration_min: get_int(r, "duration_min") as i32,
        fixed_start: get_opt_int(r, "fixed_start").map(|v| v as i32),
        pattern: get_str(r, "pattern"),
        pattern_data: get_opt_str(r, "pattern_data"),
        start_date: get_str(r, "start_date"),
        end_date: get_opt_str(r, "end_date"),
    }
}

// ---- next_sort_order helper ----------------------------------------------------

fn next_sort_order_for_date(db: &Database, date: &str) -> Result<i32> {
    let table = db.table("tasks")?;
    let max = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog)
        .map(|t| t.sort_order)
        .max()
        .unwrap_or(-1);
    Ok(max + 1)
}

fn next_sort_order_for_backlog(db: &Database) -> Result<i32> {
    let table = db.table("tasks")?;
    let max = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.is_backlog)
        .map(|t| t.sort_order)
        .max()
        .unwrap_or(-1);
    Ok(max + 1)
}

// ---- normalize helpers ---------------------------------------------------------

fn normalize_sort_order(db: &mut Database, date: &str) -> Result<()> {
    let table = db.table("tasks")?;
    let mut pairs: Vec<(u64, i32)> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog)
        .map(|t| (t.id as u64, t.sort_order))
        .collect();
    pairs.sort_by_key(|&(id, sort)| (sort, id));
    if pairs.is_empty() {
        return Ok(());
    }
    let ops: Vec<Op> = pairs
        .iter()
        .enumerate()
        .filter(|(idx, (_, sort))| *sort != *idx as i32)
        .map(|(idx, &(id, _))| Op::Update {
            id,
            fields: vec![("sort_order".into(), idx.to_string())],
        })
        .collect();
    if !ops.is_empty() {
        db.batch("tasks", ops)?;
    }
    Ok(())
}

fn normalize_backlog_sort_order(db: &mut Database) -> Result<()> {
    let table = db.table("tasks")?;
    let mut pairs: Vec<(u64, i32)> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.is_backlog)
        .map(|t| (t.id as u64, t.sort_order))
        .collect();
    pairs.sort_by_key(|&(id, sort)| (sort, id));
    if pairs.is_empty() {
        return Ok(());
    }
    let ops: Vec<Op> = pairs
        .iter()
        .enumerate()
        .filter(|(idx, (_, sort))| *sort != *idx as i32)
        .map(|(idx, &(id, _))| Op::Update {
            id,
            fields: vec![("sort_order".into(), idx.to_string())],
        })
        .collect();
    if !ops.is_empty() {
        db.batch("tasks", ops)?;
    }
    Ok(())
}

// ---- recurrence pattern helpers — recurrence.rs に移動済、ここでは委譲のみ --------

fn matches_recurrence_pattern(
    pattern: &str,
    pattern_data: Option<&str>,
    target_date: NaiveDate,
) -> Result<bool> {
    crate::recurrence::matches_recurrence_pattern(pattern, pattern_data, target_date)
}

// ---- データディレクトリ解決 -------------------------------------------------------

/// ybasey data dir を返す (init.rs と同一ロジック)
pub fn data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YTASKY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_dir().context("OS data dir not found")?;
    Ok(base.join("ytasky-ybasey"))
}

/// ybasey Database を開く
pub fn open() -> Result<Database> {
    let dir = data_dir()?;
    Database::open(&dir, Some("ytasky")).map_err(Into::into)
}

// ---- Read 系 -------------------------------------------------------------------

/// 指定日のタスクを取得 (繰り返し生成も実行)
pub fn load_tasks(db: &mut Database, date: &str) -> Result<Vec<crate::model::Task>> {
    generate_recurring_tasks(db, date)?;
    let table = db.table("tasks")?;
    let mut tasks: Vec<crate::model::Task> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog)
        .collect();
    tasks.sort_by_key(|t| t.sort_order);
    Ok(tasks)
}

/// バックログタスクを期限順で取得（期限なしは末尾）
pub fn load_backlog_tasks(db: &Database) -> Result<Vec<crate::model::Task>> {
    let table = db.table("tasks")?;
    let mut tasks: Vec<crate::model::Task> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.is_backlog)
        .collect();
    tasks.sort_by(|a, b| {
        let a_has = a.deadline.is_some();
        let b_has = b.deadline.is_some();
        // deadline あり → 先 (降順比較で Some=true > None=false)
        b_has
            .cmp(&a_has)
            .then_with(|| a.deadline.cmp(&b.deadline))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(tasks)
}

/// 全カテゴリ取得
pub fn load_categories(db: &Database) -> Result<Vec<crate::model::Category>> {
    let table = db.table("categories")?;
    Ok(table.list().iter().map(|r| record_to_category(r)).collect())
}

/// 全繰り返しルール取得
pub fn load_recurrences(db: &Database) -> Result<Vec<crate::model::Recurrence>> {
    let table = db.table("recurrences")?;
    let mut recs: Vec<_> = table.list().iter().map(|r| record_to_recurrence(r)).collect();
    recs.sort_by_key(|r| r.id);
    Ok(recs)
}

/// ID 指定でタスク1件取得
pub fn load_task_by_id(db: &Database, id: i64) -> Result<Option<crate::model::Task>> {
    let table = db.table("tasks")?;
    Ok(table.list().into_iter().find(|r| r.id == id as u64).map(record_to_task))
}

/// タスクの位置情報 (date, sort_order, is_backlog) を取得
pub fn load_task_position(db: &Database, id: i64) -> Result<Option<(String, i32, bool)>> {
    let table = db.table("tasks")?;
    Ok(table
        .list()
        .into_iter()
        .find(|r| r.id == id as u64)
        .map(|r| {
            let t = record_to_task(r);
            (t.date, t.sort_order, t.is_backlog)
        }))
}

// ---- Write 系 (task) -----------------------------------------------------------

/// タスク挿入
pub fn insert_task(
    db: &mut Database,
    date: &str,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
) -> Result<i64> {
    insert_task_with_deadline(db, date, title, category_id, duration_min, fixed_start, None)
}

/// 期限付きタスク挿入
pub fn insert_task_with_deadline(
    db: &mut Database,
    date: &str,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<i64> {
    let sort_order = next_sort_order_for_date(db, date)?;
    let mut fields = vec![
        ("date".into(), date.into()),
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
        ("status".into(), "todo".into()),
        ("sort_order".into(), sort_order.to_string()),
        ("is_backlog".into(), "0".into()),
    ];
    if let Some(fs) = fixed_start {
        fields.push(("fixed_start".into(), fs.to_string()));
    }
    if let Some(dl) = deadline {
        fields.push(("deadline".into(), dl.into()));
    }
    let id = db.insert("tasks", NewRecord::from(fields))?;
    Ok(id as i64)
}

/// タスク更新 (deadline は現在値を維持)
pub fn update_task(
    db: &mut Database,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
) -> Result<()> {
    let current_deadline = match db.table("tasks")?.get(id as u64) {
        Ok(r) => get_opt_str(r, "deadline"),
        Err(_) => None,
    };
    update_task_with_deadline(
        db,
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
    db: &mut Database,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<()> {
    let mut fields = vec![
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
    ];
    match fixed_start {
        Some(fs) => fields.push(("fixed_start".into(), fs.to_string())),
        None => fields.push(("fixed_start".into(), "_".into())),
    }
    match deadline {
        Some(dl) => fields.push(("deadline".into(), dl.into())),
        None => fields.push(("deadline".into(), "_".into())),
    }
    db.update("tasks", id as u64, fields)?;
    Ok(())
}

/// タスク削除
pub fn delete_task(db: &mut Database, id: i64) -> Result<()> {
    let pos = load_task_position(db, id)?;
    db.delete("tasks", id as u64)?;
    if let Some((date, _, is_backlog)) = pos {
        if is_backlog {
            normalize_backlog_sort_order(db)?;
        } else {
            normalize_sort_order(db, &date)?;
        }
    }
    Ok(())
}

/// 実績時刻更新
pub fn update_actual(
    db: &mut Database,
    id: i64,
    actual_start: Option<i32>,
    actual_end: Option<i32>,
) -> Result<()> {
    let fields = vec![
        (
            "actual_start".into(),
            actual_start
                .map(|v| v.to_string())
                .unwrap_or_else(|| "_".into()),
        ),
        (
            "actual_end".into(),
            actual_end
                .map(|v| v.to_string())
                .unwrap_or_else(|| "_".into()),
        ),
    ];
    db.update("tasks", id as u64, fields)?;
    Ok(())
}

// ---- Write 系 (backlog 操作) ---------------------------------------------------

/// バックログに新規追加
pub fn insert_backlog_task(
    db: &mut Database,
    title: &str,
    category_id: &str,
    duration_min: i32,
    deadline: Option<&str>,
) -> Result<i64> {
    let sort_order = next_sort_order_for_backlog(db)?;
    let mut fields = vec![
        ("date".into(), BACKLOG_DATE.into()),
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
        ("status".into(), "todo".into()),
        ("sort_order".into(), sort_order.to_string()),
        ("is_backlog".into(), "1".into()),
    ];
    if let Some(dl) = deadline {
        fields.push(("deadline".into(), dl.into()));
    }
    let id = db.insert("tasks", NewRecord::from(fields))?;
    Ok(id as i64)
}

/// タスクをバックログ化/解除
pub fn set_backlog_flag(db: &mut Database, task_id: i64, is_backlog: bool) -> Result<()> {
    if is_backlog {
        let current_date = match load_task_position(db, task_id)? {
            Some((date, _, _)) => date,
            None => return Ok(()),
        };
        let sort_order = next_sort_order_for_backlog(db)?;
        db.update(
            "tasks",
            task_id as u64,
            vec![
                ("is_backlog".into(), "1".into()),
                ("date".into(), BACKLOG_DATE.into()),
                ("sort_order".into(), sort_order.to_string()),
                ("actual_start".into(), "_".into()),
                ("actual_end".into(), "_".into()),
            ],
        )?;
        if current_date != BACKLOG_DATE {
            normalize_sort_order(db, &current_date)?;
        }
        normalize_backlog_sort_order(db)?;
        return Ok(());
    }

    db.update(
        "tasks",
        task_id as u64,
        vec![("is_backlog".into(), "0".into())],
    )?;
    Ok(())
}

/// バックログタスクを指定日の指定位置に挿入
pub fn insert_backlog_task_at(
    db: &mut Database,
    task_id: i64,
    date: &str,
    index: usize,
) -> Result<()> {
    // 指定日の非 backlog タスク数を取得
    let count = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog)
            .count()
    };
    let target = (index as i32).clamp(0, count as i32);

    // target 以降の sort_order を +1 shift
    let to_shift: Vec<u64> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog && t.sort_order >= target)
            .map(|t| t.id as u64)
            .collect()
    };
    if !to_shift.is_empty() {
        // 各タスクの現在の sort_order を取得して +1
        let ops: Vec<Op> = to_shift
            .iter()
            .filter_map(|&id| {
                let table = db.table("tasks").ok()?;
                let r = table.get(id).ok()?;
                let sort = get_int(r, "sort_order") as i32;
                Some(Op::Update {
                    id,
                    fields: vec![("sort_order".into(), (sort + 1).to_string())],
                })
            })
            .collect();
        if !ops.is_empty() {
            db.batch("tasks", ops)?;
        }
    }

    // task を schedule に移動
    db.update(
        "tasks",
        task_id as u64,
        vec![
            ("is_backlog".into(), "0".into()),
            ("date".into(), date.into()),
            ("sort_order".into(), target.to_string()),
            ("actual_start".into(), "_".into()),
            ("actual_end".into(), "_".into()),
        ],
    )?;

    normalize_backlog_sort_order(db)?;
    normalize_sort_order(db, date)?;
    Ok(())
}

/// バックログタスクを末尾に挿入
pub fn append_backlog_task(db: &mut Database, task_id: i64, date: &str) -> Result<()> {
    let count = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog)
            .count()
    };
    insert_backlog_task_at(db, task_id, date, count)
}

/// sort_order 入れ替え
pub fn swap_sort_order(db: &mut Database, id1: i64, id2: i64) -> Result<()> {
    if id1 == id2 {
        return Ok(());
    }
    let (date1, sort1, backlog1) = match load_task_position(db, id1)? {
        Some(v) => v,
        None => return Ok(()),
    };
    let (date2, sort2, backlog2) = match load_task_position(db, id2)? {
        Some(v) => v,
        None => return Ok(()),
    };
    if backlog1 || backlog2 || date1 != date2 {
        return Ok(());
    }
    db.batch(
        "tasks",
        vec![
            Op::Update {
                id: id1 as u64,
                fields: vec![("sort_order".into(), sort2.to_string())],
            },
            Op::Update {
                id: id2 as u64,
                fields: vec![("sort_order".into(), sort1.to_string())],
            },
        ],
    )?;
    // swap 後も連続性は維持されるため normalize は不要
    Ok(())
}

/// タスクをスケジュール側へ戻す (Undo 用)
pub fn restore_task_to_schedule(
    db: &mut Database,
    task_id: i64,
    date: &str,
    sort_order: i32,
) -> Result<()> {
    // sort_order 以降を shift
    let to_shift: Vec<(u64, i32)> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog && t.sort_order >= sort_order)
            .map(|t| (t.id as u64, t.sort_order))
            .collect()
    };
    if !to_shift.is_empty() {
        let ops: Vec<Op> = to_shift
            .iter()
            .map(|&(id, sort)| Op::Update {
                id,
                fields: vec![("sort_order".into(), (sort + 1).to_string())],
            })
            .collect();
        db.batch("tasks", ops)?;
    }
    db.update(
        "tasks",
        task_id as u64,
        vec![
            ("is_backlog".into(), "0".into()),
            ("date".into(), date.into()),
            ("sort_order".into(), sort_order.to_string()),
        ],
    )?;
    normalize_sort_order(db, date)?;
    normalize_backlog_sort_order(db)?;
    Ok(())
}

/// タスクをバックログ側へ戻す (Undo 用)
pub fn restore_task_to_backlog(db: &mut Database, task_id: i64, sort_order: i32) -> Result<()> {
    // backlog の sort_order 以降を shift
    let to_shift: Vec<(u64, i32)> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.is_backlog && t.sort_order >= sort_order)
            .map(|t| (t.id as u64, t.sort_order))
            .collect()
    };
    if !to_shift.is_empty() {
        let ops: Vec<Op> = to_shift
            .iter()
            .map(|&(id, sort)| Op::Update {
                id,
                fields: vec![("sort_order".into(), (sort + 1).to_string())],
            })
            .collect();
        db.batch("tasks", ops)?;
    }
    db.update(
        "tasks",
        task_id as u64,
        vec![
            ("is_backlog".into(), "1".into()),
            ("date".into(), BACKLOG_DATE.into()),
            ("sort_order".into(), sort_order.to_string()),
            ("actual_start".into(), "_".into()),
            ("actual_end".into(), "_".into()),
        ],
    )?;
    normalize_backlog_sort_order(db)?;
    Ok(())
}

// ---- Write 系 (recurrence) -----------------------------------------------------

/// 繰り返しルール登録
#[allow(clippy::too_many_arguments)]
pub fn insert_recurrence(
    db: &mut Database,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    pattern: &str,
    pattern_data: Option<&str>,
    start_date: &str,
    end_date: Option<&str>,
) -> Result<i64> {
    let mut fields = vec![
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
        ("pattern".into(), pattern.into()),
        ("start_date".into(), start_date.into()),
    ];
    if let Some(fs) = fixed_start {
        fields.push(("fixed_start".into(), fs.to_string()));
    }
    if let Some(pd) = pattern_data {
        fields.push(("pattern_data".into(), pd.into()));
    }
    if let Some(ed) = end_date {
        fields.push(("end_date".into(), ed.into()));
    }
    let id = db.insert("recurrences", NewRecord::from(fields))?;
    Ok(id as i64)
}

/// 繰り返しルール更新
#[allow(clippy::too_many_arguments)]
pub fn update_recurrence(
    db: &mut Database,
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
    let mut fields = vec![
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
        ("pattern".into(), pattern.into()),
        ("start_date".into(), start_date.into()),
    ];
    match fixed_start {
        Some(fs) => fields.push(("fixed_start".into(), fs.to_string())),
        None => fields.push(("fixed_start".into(), "_".into())),
    }
    match pattern_data {
        Some(pd) => fields.push(("pattern_data".into(), pd.into())),
        None => fields.push(("pattern_data".into(), "_".into())),
    }
    match end_date {
        Some(ed) => fields.push(("end_date".into(), ed.into())),
        None => fields.push(("end_date".into(), "_".into())),
    }
    db.update("recurrences", id as u64, fields)?;
    Ok(())
}

/// 繰り返しルール削除 (cascade で exceptions + tasks.recurrence_id=null 自動)
pub fn delete_recurrence(db: &mut Database, id: i64) -> Result<()> {
    db.delete("recurrences", id as u64)?;
    Ok(())
}

/// 既存タスクを繰り返しルールに変換
pub fn create_recurrence_from_task(
    db: &mut Database,
    task_id: i64,
    pattern: &str,
    pattern_data: Option<&str>,
    end_date: Option<&str>,
) -> Result<i64> {
    let task = load_task_by_id(db, task_id)?.context("タスクが見つからない")?;
    let recurrence_id = insert_recurrence(
        db,
        &task.title,
        &task.category_id,
        task.duration_min,
        task.fixed_start,
        pattern,
        pattern_data,
        &task.date,
        end_date,
    )?;
    db.update(
        "tasks",
        task_id as u64,
        vec![("recurrence_id".into(), recurrence_id.to_string())],
    )?;
    Ok(recurrence_id)
}

/// 繰り返し例外日を登録
pub fn add_recurrence_exception(
    db: &mut Database,
    recurrence_id: i64,
    date: &str,
) -> Result<()> {
    // 重複チェック: find_by_field で recurrence_id + date が一致するものを探す
    let already_exists = {
        let table = db.table("recurrence_exceptions")?;
        let rec_id_str = recurrence_id.to_string();
        table
            .find_by_field("recurrence_id", &rec_id_str)
            .iter()
            .any(|r| get_str(r, "exception_date") == date)
    };
    if already_exists {
        return Ok(());
    }
    let fields = vec![
        ("recurrence_id".into(), recurrence_id.to_string()),
        ("exception_date".into(), date.into()),
    ];
    db.insert("recurrence_exceptions", NewRecord::from(fields))?;
    Ok(())
}

// ---- 繰り返しタスク生成 ----------------------------------------------------------

/// 指定日に該当する繰り返しタスクを遅延生成
pub fn generate_recurring_tasks(db: &mut Database, date: &str) -> Result<()> {
    let target_date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("日付形式が不正です: {date}"))?;

    // 対象の繰り返しルールを取得
    let recurrences: Vec<crate::model::Recurrence> = {
        let table = db.table("recurrences")?;
        table
            .list()
            .iter()
            .map(|r| record_to_recurrence(r))
            .filter(|rec| {
                rec.start_date.as_str() <= date
                    && rec.end_date.as_ref().map(|ed| date <= ed.as_str()).unwrap_or(true)
            })
            .collect()
    };

    if recurrences.is_empty() {
        return Ok(());
    }

    let next_sort = next_sort_order_for_date(db, date)?;
    let mut current_sort = next_sort;

    for rec in recurrences {
        if !matches_recurrence_pattern(&rec.pattern, rec.pattern_data.as_deref(), target_date)? {
            continue;
        }

        // 例外日チェック
        let is_exception = {
            let table = db.table("recurrence_exceptions")?;
            let rec_id_str = rec.id.to_string();
            table
                .find_by_field("recurrence_id", &rec_id_str)
                .iter()
                .any(|r| get_str(r, "exception_date") == date)
        };
        if is_exception {
            continue;
        }

        // 既存チェック (同日 + 同 recurrence_id + 非 backlog)
        let exists = {
            let table = db.table("tasks")?;
            let rec_id_str = rec.id.to_string();
            table
                .find_by_field("recurrence_id", &rec_id_str)
                .iter()
                .any(|r| {
                    let t = record_to_task(r);
                    t.date == date && !t.is_backlog
                })
        };
        if exists {
            continue;
        }

        // insert
        let mut fields = vec![
            ("date".into(), date.into()),
            ("title".into(), rec.title.clone()),
            ("category_id".into(), rec.category_id.clone()),
            ("duration_min".into(), rec.duration_min.to_string()),
            ("status".into(), "todo".into()),
            ("sort_order".into(), current_sort.to_string()),
            ("is_backlog".into(), "0".into()),
            ("recurrence_id".into(), rec.id.to_string()),
        ];
        if let Some(fs) = rec.fixed_start {
            fields.push(("fixed_start".into(), fs.to_string()));
        }
        db.insert("tasks", NewRecord::from(fields))?;
        current_sort += 1;
    }

    Ok(())
}

// ---- History 系 ----------------------------------------------------------------

/// 操作履歴を取得 (ybasey query_log_tail 経由)
pub fn query_history(
    db: &Database,
    table: Option<&str>,
    limit: usize,
) -> Result<Vec<String>> {
    db.query_log_tail(table, limit).map_err(Into::into)
}

// ---- Report 系 -----------------------------------------------------------------

/// normalize_sort_order の公開 wrapper (history.rs から利用)
pub fn normalize_sort_order_pub(db: &mut Database, date: &str) -> Result<()> {
    normalize_sort_order(db, date)
}

/// normalize_backlog_sort_order の公開 wrapper (history.rs から利用)
pub fn normalize_backlog_sort_order_pub(db: &mut Database) -> Result<()> {
    normalize_backlog_sort_order(db)
}

/// 指定日のカテゴリ別集計
pub fn report_by_category(
    db: &Database,
    date: &str,
) -> Result<Vec<crate::model::CategoryReport>> {
    use std::collections::HashMap;

    let tasks: Vec<crate::model::Task> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog)
            .collect()
    };
    let categories = load_categories(db)?;

    let mut agg: HashMap<String, (i32, i32)> = HashMap::new();
    for task in &tasks {
        let entry = agg.entry(task.category_id.clone()).or_default();
        entry.0 += task.duration_min;
        if let (Some(start), Some(end)) = (task.actual_start, task.actual_end) {
            entry.1 += (end - start).max(0);
        }
    }

    let result: Vec<crate::model::CategoryReport> = categories
        .iter()
        .filter_map(|cat| {
            let (planned, actual) = agg.get(&cat.id).copied().unwrap_or_default();
            if planned > 0 || actual > 0 {
                Some(crate::model::CategoryReport {
                    category_id: cat.id.clone(),
                    category_name: cat.name.clone(),
                    planned_min: planned,
                    actual_min: actual,
                })
            } else {
                None
            }
        })
        .collect();
    Ok(result)
}

/// 指定日のタイトル別集計
pub fn report_by_title(db: &Database, date: &str) -> Result<Vec<crate::model::TitleReport>> {
    use std::collections::HashMap;

    let tasks: Vec<crate::model::Task> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| t.date == date && !t.is_backlog)
            .collect()
    };

    let mut agg: HashMap<(String, String), (i32, i32)> = HashMap::new();
    for task in &tasks {
        let key = (task.title.clone(), task.category_id.clone());
        let entry = agg.entry(key).or_default();
        entry.0 += task.duration_min;
        if let (Some(start), Some(end)) = (task.actual_start, task.actual_end) {
            entry.1 += (end - start).max(0);
        }
    }

    let mut result: Vec<crate::model::TitleReport> = agg
        .into_iter()
        .map(|((title, category_id), (planned, actual))| crate::model::TitleReport {
            title,
            category_id,
            planned_min: planned,
            actual_min: actual,
        })
        .collect();
    result.sort_by(|a, b| {
        b.planned_min
            .cmp(&a.planned_min)
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(result)
}
