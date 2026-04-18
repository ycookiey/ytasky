//! history.rs — undo/redo Command pattern
//!
//! Sub-task 11: rusqlite::Connection → ybasey::Database に全面置換済み。

use anyhow::{Context, Result};
use ybasey::Database;

use crate::model::Task;

pub trait Command {
    fn execute(&mut self, db: &mut Database) -> Result<()>;
    fn undo(&mut self, db: &mut Database) -> Result<()>;
    fn redo(&mut self, db: &mut Database) -> Result<()>;
}

pub struct UndoManager {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
    max_size: usize,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::with_max_size(100)
    }
}

impl UndoManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_size,
        }
    }

    pub fn execute_command(&mut self, db: &mut Database, mut command: Box<dyn Command>) -> Result<()> {
        command.execute(db)?;
        if self.undo_stack.len() >= self.max_size {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(command);
        self.redo_stack.clear();
        Ok(())
    }

    pub fn undo(&mut self, db: &mut Database) -> Result<bool> {
        let Some(mut command) = self.undo_stack.pop() else {
            return Ok(false);
        };
        command.undo(db)?;
        self.redo_stack.push(command);
        Ok(true)
    }

    pub fn redo(&mut self, db: &mut Database) -> Result<bool> {
        let Some(mut command) = self.redo_stack.pop() else {
            return Ok(false);
        };
        command.redo(db)?;
        self.undo_stack.push(command);
        Ok(true)
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }
}

#[allow(dead_code)]
pub struct AddTaskCommand {
    date: String,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    inserted_task: Option<Task>,
}

#[allow(dead_code)]
impl AddTaskCommand {
    pub fn new(
        date: String,
        title: String,
        category_id: String,
        duration_min: i32,
        fixed_start: Option<i32>,
    ) -> Self {
        Self {
            date,
            title,
            category_id,
            duration_min,
            fixed_start,
            inserted_task: None,
        }
    }
}

impl Command for AddTaskCommand {
    fn execute(&mut self, db: &mut Database) -> Result<()> {
        let id = crate::db::insert_task(
            db,
            &self.date,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
        )?;
        let task =
            crate::db::load_task_by_id(db, id)?.context("追加後のタスク状態を取得できませんでした")?;
        self.inserted_task = Some(task);
        Ok(())
    }

    fn undo(&mut self, db: &mut Database) -> Result<()> {
        if let Some(task) = &self.inserted_task {
            crate::db::delete_task(db, task.id)?;
        }
        Ok(())
    }

    fn redo(&mut self, db: &mut Database) -> Result<()> {
        let task = self
            .inserted_task
            .as_ref()
            .context("Redo対象の追加タスク情報がありません")?;
        restore_task(db, task)?;
        Ok(())
    }
}

#[allow(dead_code)]
pub struct EditTaskCommand {
    task_id: i64,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    before: Option<Task>,
    after: Option<Task>,
}

#[allow(dead_code)]
impl EditTaskCommand {
    pub fn new(
        task_id: i64,
        title: String,
        category_id: String,
        duration_min: i32,
        fixed_start: Option<i32>,
    ) -> Self {
        Self {
            task_id,
            title,
            category_id,
            duration_min,
            fixed_start,
            before: None,
            after: None,
        }
    }
}

impl Command for EditTaskCommand {
    fn execute(&mut self, db: &mut Database) -> Result<()> {
        let before =
            crate::db::load_task_by_id(db, self.task_id)?.context("編集対象タスクが見つかりません")?;
        crate::db::update_task(
            db,
            self.task_id,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
        )?;
        let after = crate::db::load_task_by_id(db, self.task_id)?
            .context("編集後のタスク状態を取得できませんでした")?;
        self.before = Some(before);
        self.after = Some(after);
        Ok(())
    }

    fn undo(&mut self, db: &mut Database) -> Result<()> {
        let before = self
            .before
            .as_ref()
            .context("Undo対象の編集前状態がありません")?;
        crate::db::update_task(
            db,
            before.id,
            &before.title,
            &before.category_id,
            before.duration_min,
            before.fixed_start,
        )?;
        Ok(())
    }

    fn redo(&mut self, db: &mut Database) -> Result<()> {
        let after = self
            .after
            .as_ref()
            .context("Redo対象の編集後状態がありません")?;
        crate::db::update_task(
            db,
            after.id,
            &after.title,
            &after.category_id,
            after.duration_min,
            after.fixed_start,
        )?;
        Ok(())
    }
}

pub struct DeleteTaskCommand {
    task_id: i64,
    deleted_task: Option<Task>,
}

impl DeleteTaskCommand {
    pub fn new(task_id: i64) -> Self {
        Self {
            task_id,
            deleted_task: None,
        }
    }
}

impl Command for DeleteTaskCommand {
    fn execute(&mut self, db: &mut Database) -> Result<()> {
        let task =
            crate::db::load_task_by_id(db, self.task_id)?.context("削除対象タスクが見つかりません")?;
        crate::db::delete_task(db, self.task_id)?;
        self.deleted_task = Some(task);
        Ok(())
    }

    fn undo(&mut self, db: &mut Database) -> Result<()> {
        let task = self
            .deleted_task
            .as_ref()
            .context("Undo対象の削除タスク情報がありません")?;
        restore_task(db, task)?;
        Ok(())
    }

    fn redo(&mut self, db: &mut Database) -> Result<()> {
        crate::db::delete_task(db, self.task_id)?;
        Ok(())
    }
}

pub struct ReorderTaskCommand {
    id1: i64,
    id2: i64,
}

impl ReorderTaskCommand {
    pub fn new(id1: i64, id2: i64) -> Self {
        Self { id1, id2 }
    }
}

impl Command for ReorderTaskCommand {
    fn execute(&mut self, db: &mut Database) -> Result<()> {
        crate::db::swap_sort_order(db, self.id1, self.id2)?;
        Ok(())
    }

    fn undo(&mut self, db: &mut Database) -> Result<()> {
        crate::db::swap_sort_order(db, self.id1, self.id2)?;
        Ok(())
    }

    fn redo(&mut self, db: &mut Database) -> Result<()> {
        crate::db::swap_sort_order(db, self.id1, self.id2)?;
        Ok(())
    }
}

pub struct ToggleActualCommand {
    task_id: i64,
    before_start: Option<i32>,
    before_end: Option<i32>,
    after_start: Option<i32>,
    after_end: Option<i32>,
}

impl ToggleActualCommand {
    pub fn new(task: &Task, now_min: i32) -> Self {
        let (after_start, after_end) = match (task.actual_start, task.actual_end) {
            (None, None) => (Some(now_min), None),
            (Some(start), None) => (Some(start), Some(now_min.max(start))),
            (Some(_), Some(_)) | (None, Some(_)) => (None, None),
        };
        Self {
            task_id: task.id,
            before_start: task.actual_start,
            before_end: task.actual_end,
            after_start,
            after_end,
        }
    }
}

impl Command for ToggleActualCommand {
    fn execute(&mut self, db: &mut Database) -> Result<()> {
        crate::db::update_actual(db, self.task_id, self.after_start, self.after_end)?;
        Ok(())
    }

    fn undo(&mut self, db: &mut Database) -> Result<()> {
        crate::db::update_actual(db, self.task_id, self.before_start, self.before_end)?;
        Ok(())
    }

    fn redo(&mut self, db: &mut Database) -> Result<()> {
        crate::db::update_actual(db, self.task_id, self.after_start, self.after_end)?;
        Ok(())
    }
}

// ---- history.rs 内部 helper (restore_task) ------------------------------------

/// 削除済みタスクを復元する (Undo 用)
fn restore_task(db: &mut Database, task: &Task) -> Result<()> {
    // 既存なら何もしない
    if crate::db::load_task_by_id(db, task.id)?.is_some() {
        return Ok(());
    }

    // sort_order 以降を shift
    if task.is_backlog {
        let to_shift: Vec<(u64, i32)> = {
            let table = db.table("tasks")?;
            table
                .list()
                .iter()
                .map(|r| {
                    let id = r.id;
                    let sort = match r.get("sort_order") {
                        Some(ybasey::schema::Value::Int(v)) => *v as i32,
                        _ => 0,
                    };
                    let is_bl = match r.get("is_backlog") {
                        Some(ybasey::schema::Value::Int(v)) => *v != 0,
                        _ => false,
                    };
                    (id, sort, is_bl)
                })
                .filter(|(_, sort, is_bl)| *is_bl && *sort >= task.sort_order)
                .map(|(id, sort, _)| (id, sort))
                .collect()
        };
        if !to_shift.is_empty() {
            let ops: Vec<ybasey::Op> = to_shift
                .iter()
                .map(|&(id, sort)| ybasey::Op::Update {
                    id,
                    fields: vec![("sort_order".into(), (sort + 1).to_string())],
                })
                .collect();
            db.batch("tasks", ops)?;
        }
    } else {
        let to_shift: Vec<(u64, i32)> = {
            let table = db.table("tasks")?;
            table
                .list()
                .iter()
                .map(|r| {
                    let id = r.id;
                    let sort = match r.get("sort_order") {
                        Some(ybasey::schema::Value::Int(v)) => *v as i32,
                        _ => 0,
                    };
                    let date = match r.get("date") {
                        Some(ybasey::schema::Value::Str(s)) => s.clone(),
                        _ => String::new(),
                    };
                    let is_bl = match r.get("is_backlog") {
                        Some(ybasey::schema::Value::Int(v)) => *v != 0,
                        _ => false,
                    };
                    (id, sort, date, is_bl)
                })
                .filter(|(_, sort, date, is_bl)| {
                    !is_bl && date == &task.date && *sort >= task.sort_order
                })
                .map(|(id, sort, _, _)| (id, sort))
                .collect()
        };
        if !to_shift.is_empty() {
            let ops: Vec<ybasey::Op> = to_shift
                .iter()
                .map(|&(id, sort)| ybasey::Op::Update {
                    id,
                    fields: vec![("sort_order".into(), (sort + 1).to_string())],
                })
                .collect();
            db.batch("tasks", ops)?;
        }
    }

    // InsertWithId で元の id で復元
    let mut fields = vec![
        ("date".into(), task.date.clone()),
        ("title".into(), task.title.clone()),
        ("category_id".into(), task.category_id.clone()),
        ("duration_min".into(), task.duration_min.to_string()),
        ("status".into(), "todo".into()),
        ("sort_order".into(), task.sort_order.to_string()),
        ("is_backlog".into(), if task.is_backlog { "1" } else { "0" }.into()),
    ];
    if let Some(fs) = task.fixed_start {
        fields.push(("fixed_start".into(), fs.to_string()));
    }
    if let Some(s) = task.actual_start {
        fields.push(("actual_start".into(), s.to_string()));
    }
    if let Some(e) = task.actual_end {
        fields.push(("actual_end".into(), e.to_string()));
    }
    if let Some(rid) = task.recurrence_id {
        fields.push(("recurrence_id".into(), rid.to_string()));
    }
    if let Some(dl) = &task.deadline {
        fields.push(("deadline".into(), dl.clone()));
    }

    db.batch(
        "tasks",
        vec![ybasey::Op::InsertWithId {
            id: task.id as u64,
            record: ybasey::NewRecord::from(fields),
        }],
    )?;

    // normalize
    if task.is_backlog {
        crate::db::normalize_backlog_sort_order_pub(db)?;
    } else {
        crate::db::normalize_sort_order_pub(db, &task.date)?;
    }
    Ok(())
}
