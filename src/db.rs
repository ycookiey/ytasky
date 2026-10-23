//! Database layer over `ybasey::Database`.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::PathBuf;
use ybasey::{Database, NewRecord, Op};

/// Date stored for backlog tasks. A `date` field rejects a non-date marker such as "__backlog__".
const BACKLOG_DATE: &str = "9999-12-31";

// ---- Value -> primitive helpers -------------------------------------------------

fn get_int(r: &ybasey::engine::Record, field: &str) -> i64 {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => *v,
        _ => 0,
    }
}

fn get_str(r: &ybasey::engine::Record, field: &str) -> String {
    match r.get(field) {
        Some(ybasey::schema::Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

fn get_opt_int(r: &ybasey::engine::Record, field: &str) -> Option<i64> {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => Some(*v),
        _ => None,
    }
}

fn get_opt_str(r: &ybasey::engine::Record, field: &str) -> Option<String> {
    match r.get(field) {
        Some(ybasey::schema::Value::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Ref fields hold the id as `Value::Int`; returns it as a String, or "" when absent.
fn get_ref_id_str(r: &ybasey::engine::Record, field: &str) -> String {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => v.to_string(),
        _ => String::new(),
    }
}

fn get_opt_ref_id(r: &ybasey::engine::Record, field: &str) -> Option<i64> {
    match r.get(field) {
        Some(ybasey::schema::Value::Int(v)) => Some(*v),
        _ => None,
    }
}

// ---- Record -> model structs ----------------------------------------------------

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
        external_id: get_opt_str(r, "external_id"),
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
        external_id: get_opt_str(r, "external_id"),
    }
}

// ---- Placing fixed-time tasks ---------------------------------------------------

/// Computes the sort_order at which to insert a task, given its fixed_start.
/// `exclude_id` leaves out the task itself when editing.
///
/// - No fixed_start, or no other fixed tasks on the day: append at the end.
/// - Otherwise, among the day's fixed tasks sorted by time, take the one before and
///   the one after the new fixed_start, and insert next to whichever is closer
///   (the end of the previous one or the start of the next one). Ties go before.
///   Earlier than all: just before the first fixed task. Later than all: just after the last.
fn compute_insert_sort_order(
    db: &Database,
    date: &str,
    fixed_start: Option<i32>,
    exclude_id: Option<i64>,
) -> Result<i32> {
    let table = db.table("tasks")?;
    let mut tasks: Vec<crate::model::Task> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog && Some(t.id) != exclude_id)
        .collect();
    tasks.sort_by_key(|t| t.sort_order);

    let max_sort = tasks.last().map(|t| t.sort_order).unwrap_or(-1);

    let Some(fs) = fixed_start else {
        return Ok(max_sort + 1);
    };

    let mut fixed_tasks: Vec<&crate::model::Task> =
        tasks.iter().filter(|t| t.fixed_start.is_some()).collect();
    if fixed_tasks.is_empty() {
        return Ok(max_sort + 1);
    }
    fixed_tasks.sort_by_key(|t| t.fixed_start.unwrap());

    let pos = fixed_tasks
        .iter()
        .position(|t| t.fixed_start.unwrap() > fs)
        .unwrap_or(fixed_tasks.len());

    let prev_fixed = if pos > 0 {
        Some(fixed_tasks[pos - 1])
    } else {
        None
    };
    let next_fixed = fixed_tasks.get(pos).copied();

    let target = match (prev_fixed, next_fixed) {
        (None, Some(n)) => n.sort_order,
        (Some(p), None) => p.sort_order + 1,
        (Some(p), Some(n)) => {
            let dist_prev = fs - (p.fixed_start.unwrap() + p.duration_min);
            let dist_next = n.fixed_start.unwrap() - fs;
            if dist_prev <= dist_next {
                p.sort_order + 1
            } else {
                n.sort_order
            }
        }
        (None, None) => max_sort + 1,
    };
    Ok(target)
}

/// Shifts tasks with `sort_order >= threshold` by +1, skipping `exclude_id`.
fn shift_sort_orders_at_or_after(
    db: &mut Database,
    date: &str,
    threshold: i32,
    exclude_id: Option<i64>,
) -> Result<()> {
    let to_shift: Vec<(u64, i32)> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| {
                t.date == date
                    && !t.is_backlog
                    && t.sort_order >= threshold
                    && Some(t.id) != exclude_id
            })
            .map(|t| (t.id as u64, t.sort_order))
            .collect()
    };
    if to_shift.is_empty() {
        return Ok(());
    }
    let ops: Vec<Op> = to_shift
        .iter()
        .map(|&(id, sort)| Op::Update {
            id,
            fields: vec![("sort_order".into(), (sort + 1).to_string())],
        })
        .collect();
    db.batch("tasks", ops)?;
    Ok(())
}

/// When a day's fixed tasks are not in time order, moves only the fixed tasks into
/// time order. Tasks without a fixed time keep their sort_order slots.
fn normalize_fixed_tasks_by_time(db: &mut Database, date: &str) -> Result<()> {
    let table = db.table("tasks")?;
    let mut tasks: Vec<crate::model::Task> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog)
        .collect();
    if tasks.len() < 2 {
        return Ok(());
    }
    tasks.sort_by_key(|t| t.sort_order);

    // Fixed tasks in their current sort_order
    let fixed_by_sort: Vec<&crate::model::Task> =
        tasks.iter().filter(|t| t.fixed_start.is_some()).collect();
    if fixed_by_sort.len() < 2 {
        return Ok(());
    }

    // The same tasks in time order
    let mut fixed_by_time = fixed_by_sort.clone();
    fixed_by_time.sort_by_key(|t| t.fixed_start.unwrap());

    let already_ok = fixed_by_sort
        .iter()
        .zip(fixed_by_time.iter())
        .all(|(a, b)| a.id == b.id);
    if already_ok {
        return Ok(());
    }

    // Slots (positions in the current order) held by fixed tasks
    let fixed_slots: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.fixed_start.is_some())
        .map(|(i, _)| i)
        .collect();

    // Assign the time-ordered fixed tasks to those slots and build each task's new sort_order
    let mut slot_owner: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    for (slot_idx, fx) in fixed_slots.iter().zip(fixed_by_time.iter()) {
        slot_owner[*slot_idx] = fx.id;
    }

    let mut id_to_new_sort: std::collections::HashMap<i64, i32> = Default::default();
    for (i, id) in slot_owner.iter().enumerate() {
        id_to_new_sort.insert(*id, i as i32);
    }

    let ops: Vec<Op> = tasks
        .iter()
        .filter_map(|t| {
            let new_sort = *id_to_new_sort.get(&t.id)?;
            if new_sort != t.sort_order {
                Some(Op::Update {
                    id: t.id as u64,
                    fields: vec![("sort_order".into(), new_sort.to_string())],
                })
            } else {
                None
            }
        })
        .collect();
    if !ops.is_empty() {
        db.batch("tasks", ops)?;
    }
    Ok(())
}

/// Relocates the edited task: closes the gap it leaves, computes the new insert
/// position, and shifts the others. Returns the new sort_order.
fn relocate_task_for_fixed_change(
    db: &mut Database,
    task_id: i64,
    date: &str,
    fixed_start: Option<i32>,
) -> Result<i32> {
    let current_sort = load_task_by_id(db, task_id)?
        .context("relocate 対象タスクが見つかりません")?
        .sort_order;

    // Close the gap: move the other tasks after the current position up by one
    let to_decrement: Vec<(u64, i32)> = {
        let table = db.table("tasks")?;
        table
            .list()
            .iter()
            .map(|r| record_to_task(r))
            .filter(|t| {
                t.date == date && !t.is_backlog && t.id != task_id && t.sort_order > current_sort
            })
            .map(|t| (t.id as u64, t.sort_order))
            .collect()
    };
    if !to_decrement.is_empty() {
        let ops: Vec<Op> = to_decrement
            .iter()
            .map(|&(id, s)| Op::Update {
                id,
                fields: vec![("sort_order".into(), (s - 1).to_string())],
            })
            .collect();
        db.batch("tasks", ops)?;
    }

    // Compute the new position without the task itself
    let new_target = compute_insert_sort_order(db, date, fixed_start, Some(task_id))?;
    // Shift the other tasks at or after the new position by +1
    shift_sort_orders_at_or_after(db, date, new_target, Some(task_id))?;
    Ok(new_target)
}

/// Snapshot of the day's (id, sort_order) pairs, for undo.
pub fn snapshot_sort_orders(db: &Database, date: &str) -> Result<Vec<(i64, i32)>> {
    let table = db.table("tasks")?;
    let pairs: Vec<(i64, i32)> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| t.date == date && !t.is_backlog)
        .map(|t| (t.id, t.sort_order))
        .collect();
    Ok(pairs)
}

/// Writes back a snapshot of (id, sort_order) pairs, for undo.
pub fn apply_sort_orders(db: &mut Database, pairs: &[(i64, i32)]) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let ops: Vec<Op> = pairs
        .iter()
        .map(|&(id, sort)| Op::Update {
            id: id as u64,
            fields: vec![("sort_order".into(), sort.to_string())],
        })
        .collect();
    db.batch("tasks", ops)?;
    Ok(())
}

/// Whether the task keeps the same rank among the day's fixed tasks after the edit.
/// Can be true only when both values are Some.
fn fixed_position_unchanged(
    db: &Database,
    task_id: i64,
    date: &str,
    old_fs: Option<i32>,
    new_fs: Option<i32>,
) -> Result<bool> {
    let (Some(old), Some(new)) = (old_fs, new_fs) else {
        return Ok(false);
    };
    let table = db.table("tasks")?;
    let other_fs: Vec<i32> = table
        .list()
        .iter()
        .map(|r| record_to_task(r))
        .filter(|t| {
            t.date == date && !t.is_backlog && t.id != task_id && t.fixed_start.is_some()
        })
        .map(|t| t.fixed_start.unwrap())
        .collect();
    let rank_old = other_fs.iter().filter(|&&f| f < old).count();
    let rank_new = other_fs.iter().filter(|&&f| f < new).count();
    Ok(rank_old == rank_new)
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

// ---- Normalize helpers ----------------------------------------------------------

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

// ---- Data directory -------------------------------------------------------------

/// Must stay in sync with `init::resolve_data_dir`.
pub fn data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("YTASKY_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_dir().context("OS data dir not found")?;
    Ok(base.join("ytasky-ybasey"))
}

pub fn open() -> Result<Database> {
    let dir = data_dir()?;
    warn_legacy_sqlite_files();
    let mut db = Database::open(&dir, Some("ytasky"))?;
    crate::init::migrate_schema(&mut db)?;
    Ok(db)
}

fn warn_legacy_sqlite_files() {
    if let Some(base) = dirs::data_dir() {
        let legacy_dir = base.join("ytasky");
        for name in ["ytasky.db", "ytasky_history.db"] {
            let path = legacy_dir.join(name);
            if path.exists() {
                eprintln!(
                    "ytasky: 旧 SQLite DB が存在します: {}. 不要であれば手動削除してください。",
                    path.display()
                );
            }
        }
    }
}

// ---- Reads ----------------------------------------------------------------------

/// Tasks of a date. Generates the date's recurring tasks first.
pub fn load_tasks(db: &mut Database, date: &str) -> Result<Vec<crate::model::Task>> {
    generate_recurring_tasks(db, date)?;
    // Repair existing data: put fixed tasks back in time order when needed
    normalize_fixed_tasks_by_time(db, date)?;
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

/// Backlog tasks ordered by deadline, tasks without a deadline last.
/// Tasks with `status="done"` (for example, issues closed upstream) are hidden.
pub fn load_backlog_tasks(db: &Database) -> Result<Vec<crate::model::Task>> {
    let table = db.table("tasks")?;
    let mut tasks: Vec<crate::model::Task> = table
        .list()
        .iter()
        .filter(|r| get_str(r, "status") != "done")
        .map(|r| record_to_task(r))
        .filter(|t| t.is_backlog)
        .collect();
    tasks.sort_by(|a, b| {
        let a_has = a.deadline.is_some();
        let b_has = b.deadline.is_some();
        // Tasks with a deadline first (reversed comparison: Some=true > None=false)
        b_has
            .cmp(&a_has)
            .then_with(|| a.deadline.cmp(&b.deadline))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(tasks)
}

pub fn load_categories(db: &Database) -> Result<Vec<crate::model::Category>> {
    let table = db.table("categories")?;
    Ok(table.list().iter().map(|r| record_to_category(r)).collect())
}

pub fn load_recurrences(db: &Database) -> Result<Vec<crate::model::Recurrence>> {
    let table = db.table("recurrences")?;
    let mut recs: Vec<_> = table.list().iter().map(|r| record_to_recurrence(r)).collect();
    recs.sort_by_key(|r| r.id);
    Ok(recs)
}

pub fn load_task_by_id(db: &Database, id: i64) -> Result<Option<crate::model::Task>> {
    let table = db.table("tasks")?;
    Ok(table.list().into_iter().find(|r| r.id == id as u64).map(record_to_task))
}

/// Position of a task: (date, sort_order, is_backlog).
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

// ---- Writes (tasks) -------------------------------------------------------------

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

pub fn insert_task_with_deadline(
    db: &mut Database,
    date: &str,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<i64> {
    // Put existing fixed tasks back in time order before computing the position
    normalize_fixed_tasks_by_time(db, date)?;

    let target_sort = compute_insert_sort_order(db, date, fixed_start, None)?;
    shift_sort_orders_at_or_after(db, date, target_sort, None)?;

    let mut fields = vec![
        ("date".into(), date.into()),
        ("title".into(), title.into()),
        ("category_id".into(), category_id.into()),
        ("duration_min".into(), duration_min.to_string()),
        ("status".into(), "todo".into()),
        ("sort_order".into(), target_sort.to_string()),
        ("is_backlog".into(), "0".into()),
    ];
    if let Some(fs) = fixed_start {
        fields.push(("fixed_start".into(), fs.to_string()));
    }
    if let Some(dl) = deadline {
        fields.push(("deadline".into(), dl.into()));
    }
    let id = db.insert("tasks", NewRecord::from(fields))?;
    normalize_sort_order(db, date)?;
    Ok(id as i64)
}

/// Updates a task, keeping its current deadline.
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

pub fn update_task_with_deadline(
    db: &mut Database,
    id: i64,
    title: &str,
    category_id: &str,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<&str>,
) -> Result<()> {
    let before = load_task_by_id(db, id)?
        .context("編集対象タスクが見つかりません")?;
    let date = before.date.clone();
    let fixed_changed = before.fixed_start != fixed_start;

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

    let mut should_relocate = false;
    if fixed_changed && !before.is_backlog {
        // If the task keeps its rank among the fixed tasks, keep its sort_order
        let unchanged =
            fixed_position_unchanged(db, id, &date, before.fixed_start, fixed_start)?;
        if !unchanged {
            should_relocate = true;
        }
    }

    if should_relocate {
        let new_sort = relocate_task_for_fixed_change(db, id, &date, fixed_start)?;
        fields.push(("sort_order".into(), new_sort.to_string()));
    }

    db.update("tasks", id as u64, fields)?;
    if should_relocate {
        normalize_sort_order(db, &date)?;
    }
    Ok(())
}

/// For undo/redo: rewrites the fields only, without relocating any task.
pub fn update_task_fields_only(
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

// ---- Writes (backlog) -----------------------------------------------------------

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

/// Id of the task with the given external_id, used by imports to find duplicates.
#[cfg(feature = "github")]
pub fn find_task_id_by_external_id(db: &Database, external_id: &str) -> Result<Option<i64>> {
    let table = db.table("tasks").context("tasks テーブルが無い")?;
    Ok(table
        .find_by_field("external_id", external_id)
        .first()
        .map(|r| r.id as i64))
}

/// Adds a backlog task with an external_id, for imports.
#[cfg(feature = "github")]
pub fn insert_backlog_task_imported(
    db: &mut Database,
    title: &str,
    category_id: &str,
    duration_min: i32,
    deadline: Option<&str>,
    external_id: &str,
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
        ("external_id".into(), external_id.into()),
    ];
    if let Some(dl) = deadline {
        fields.push(("deadline".into(), dl.into()));
    }
    let id = db.insert("tasks", NewRecord::from(fields))?;
    Ok(id as i64)
}

/// Updates the columns an import owns (title, deadline, status). Columns the user
/// owns, such as category_id, duration_min and placement, stay untouched.
/// deadline=None clears the field with the Null sentinel "_".
/// status goes back to "todo" so that an issue reopened upstream returns to the
/// backlog after an earlier closed-issue check marked it "done".
#[cfg(feature = "github")]
pub fn update_task_imported_fields(
    db: &mut Database,
    id: i64,
    title: &str,
    deadline: Option<&str>,
) -> Result<()> {
    let fields = vec![
        ("title".into(), title.into()),
        (
            "deadline".into(),
            deadline.map(|d| d.to_string()).unwrap_or_else(|| "_".into()),
        ),
        ("status".into(), "todo".into()),
    ];
    db.update("tasks", id as u64, fields)?;
    Ok(())
}

/// Sets only `status` to "done". Unlike `done_task`, it records no actual times,
/// so the task does not count toward report totals. Used for backlog tasks whose
/// GitHub issue was closed upstream.
#[cfg(feature = "github")]
pub fn set_task_status_done(db: &mut Database, id: i64) -> Result<()> {
    db.update("tasks", id as u64, vec![("status".into(), "done".into())])?;
    Ok(())
}

/// (id, external_id) of GitHub-imported tasks that are in the backlog and not done.
/// Skipping tasks already marked done keeps a rerun of the closed-issue check from
/// calling the REST API for them again or counting them again in `closed_done`.
#[cfg(feature = "github")]
pub fn list_backlog_github_external_ids(db: &Database) -> Result<Vec<(i64, String)>> {
    let table = db.table("tasks").context("tasks テーブルが無い")?;
    let mut out = Vec::new();
    for r in table.list().iter() {
        if get_str(r, "status") == "done" {
            continue;
        }
        let t = record_to_task(r);
        if !t.is_backlog {
            continue;
        }
        let Some(ext) = t.external_id else {
            continue;
        };
        if ext.starts_with("github:") {
            out.push((t.id, ext));
        }
    }
    Ok(out)
}

/// Moves a task to or from the backlog.
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

/// Inserts a backlog task into a date at the given position.
pub fn insert_backlog_task_at(
    db: &mut Database,
    task_id: i64,
    date: &str,
    index: usize,
) -> Result<()> {
    // Number of scheduled (non-backlog) tasks on the date
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

    // Shift sort_order at or after the target by +1
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

/// Appends a backlog task to the end of a date.
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
    // A swap keeps sort_order contiguous, so no normalize is needed
    Ok(())
}

/// Moves a task back into the schedule (for undo).
pub fn restore_task_to_schedule(
    db: &mut Database,
    task_id: i64,
    date: &str,
    sort_order: i32,
) -> Result<()> {
    // Shift sort_order at and after its position
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

/// Moves a task back into the backlog (for undo).
pub fn restore_task_to_backlog(db: &mut Database, task_id: i64, sort_order: i32) -> Result<()> {
    // Shift the backlog's sort_order at and after its position
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

// ---- Writes (recurrences) -------------------------------------------------------

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

/// Deletes a recurrence. The schema cascades: its exceptions are deleted and
/// tasks.recurrence_id is set to null.
pub fn delete_recurrence(db: &mut Database, id: i64) -> Result<()> {
    db.delete("recurrences", id as u64)?;
    Ok(())
}

/// Turns an existing task into a recurrence.
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

pub fn add_recurrence_exception(
    db: &mut Database,
    recurrence_id: i64,
    date: &str,
) -> Result<()> {
    // Skip when an exception with the same recurrence_id and date exists
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

// ---- Generating recurring tasks -------------------------------------------------

/// Generates the recurring tasks of a date when the date is first loaded.
pub fn generate_recurring_tasks(db: &mut Database, date: &str) -> Result<()> {
    let target_date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("日付形式が不正です: {date}"))?;

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

    for rec in recurrences {
        if !crate::recurrence::matches_recurrence_pattern(&rec, target_date)? {
            continue;
        }

        // Skip exception dates
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

        // Skip when already generated (same date, same recurrence_id, not in the backlog)
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

        // Go through compute -> shift -> insert each time to keep fixed tasks in time order
        let target_sort = compute_insert_sort_order(db, date, rec.fixed_start, None)?;
        shift_sort_orders_at_or_after(db, date, target_sort, None)?;

        let mut fields = vec![
            ("date".into(), date.into()),
            ("title".into(), rec.title.clone()),
            ("category_id".into(), rec.category_id.clone()),
            ("duration_min".into(), rec.duration_min.to_string()),
            ("status".into(), "todo".into()),
            ("sort_order".into(), target_sort.to_string()),
            ("is_backlog".into(), "0".into()),
            ("recurrence_id".into(), rec.id.to_string()),
        ];
        if let Some(fs) = rec.fixed_start {
            fields.push(("fixed_start".into(), fs.to_string()));
        }
        db.insert("tasks", NewRecord::from(fields))?;
        normalize_sort_order(db, date)?;
    }

    Ok(())
}

// ---- History --------------------------------------------------------------------

/// Operation history, read from the database log.
pub fn query_history(
    db: &Database,
    table: Option<&str>,
    limit: usize,
) -> Result<Vec<String>> {
    db.query_log_tail(table, limit).map_err(Into::into)
}

// ---- Reports --------------------------------------------------------------------

/// Public wrapper of normalize_sort_order for history.rs.
pub fn normalize_sort_order_pub(db: &mut Database, date: &str) -> Result<()> {
    normalize_sort_order(db, date)
}

/// Public wrapper of normalize_backlog_sort_order for history.rs.
pub fn normalize_backlog_sort_order_pub(db: &mut Database) -> Result<()> {
    normalize_backlog_sort_order(db)
}

/// Totals per category for a date.
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

/// Totals per title for a date.
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
