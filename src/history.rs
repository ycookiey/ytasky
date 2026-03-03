use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::Task;

pub trait Command {
    fn execute(&mut self, conn: &Connection) -> Result<()>;
    fn undo(&mut self, conn: &Connection) -> Result<()>;
    fn redo(&mut self, conn: &Connection) -> Result<()>;
}

#[derive(Default)]
pub struct UndoManager {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
}

impl UndoManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn execute_command(
        &mut self,
        conn: &Connection,
        mut command: Box<dyn Command>,
    ) -> Result<()> {
        command.execute(conn)?;
        self.undo_stack.push(command);
        self.redo_stack.clear();
        Ok(())
    }

    pub fn undo(&mut self, conn: &Connection) -> Result<bool> {
        let Some(mut command) = self.undo_stack.pop() else {
            return Ok(false);
        };

        command.undo(conn)?;
        self.redo_stack.push(command);
        Ok(true)
    }

    pub fn redo(&mut self, conn: &Connection) -> Result<bool> {
        let Some(mut command) = self.redo_stack.pop() else {
            return Ok(false);
        };

        command.redo(conn)?;
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

pub struct AddTaskCommand {
    date: String,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    inserted_task: Option<Task>,
}

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
    fn execute(&mut self, conn: &Connection) -> Result<()> {
        let id = crate::db::insert_task(
            conn,
            &self.date,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
        )?;

        let task =
            load_task_by_id(conn, id)?.context("追加後のタスク状態を取得できませんでした")?;
        self.inserted_task = Some(task);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> Result<()> {
        if let Some(task) = &self.inserted_task {
            crate::db::delete_task(conn, task.id)?;
        }
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> Result<()> {
        let task = self
            .inserted_task
            .as_ref()
            .context("Redo対象の追加タスク情報がありません")?;
        restore_task(conn, task)?;
        Ok(())
    }
}

pub struct EditTaskCommand {
    task_id: i64,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    before: Option<Task>,
    after: Option<Task>,
}

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
    fn execute(&mut self, conn: &Connection) -> Result<()> {
        let before =
            load_task_by_id(conn, self.task_id)?.context("編集対象タスクが見つかりません")?;

        crate::db::update_task(
            conn,
            self.task_id,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
        )?;

        let after = load_task_by_id(conn, self.task_id)?
            .context("編集後のタスク状態を取得できませんでした")?;
        self.before = Some(before);
        self.after = Some(after);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> Result<()> {
        let before = self
            .before
            .as_ref()
            .context("Undo対象の編集前状態がありません")?;

        crate::db::update_task(
            conn,
            before.id,
            &before.title,
            &before.category_id,
            before.duration_min,
            before.fixed_start,
        )?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> Result<()> {
        let after = self
            .after
            .as_ref()
            .context("Redo対象の編集後状態がありません")?;

        crate::db::update_task(
            conn,
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
    fn execute(&mut self, conn: &Connection) -> Result<()> {
        let task =
            load_task_by_id(conn, self.task_id)?.context("削除対象タスクが見つかりません")?;
        crate::db::delete_task(conn, self.task_id)?;
        self.deleted_task = Some(task);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> Result<()> {
        let task = self
            .deleted_task
            .as_ref()
            .context("Undo対象の削除タスク情報がありません")?;
        restore_task(conn, task)?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> Result<()> {
        crate::db::delete_task(conn, self.task_id)?;
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
    fn execute(&mut self, conn: &Connection) -> Result<()> {
        crate::db::swap_sort_order(conn, self.id1, self.id2)?;
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> Result<()> {
        crate::db::swap_sort_order(conn, self.id1, self.id2)?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> Result<()> {
        crate::db::swap_sort_order(conn, self.id1, self.id2)?;
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
    fn execute(&mut self, conn: &Connection) -> Result<()> {
        crate::db::update_actual(conn, self.task_id, self.after_start, self.after_end)?;
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> Result<()> {
        crate::db::update_actual(conn, self.task_id, self.before_start, self.before_end)?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> Result<()> {
        crate::db::update_actual(conn, self.task_id, self.after_start, self.after_end)?;
        Ok(())
    }
}

fn load_task_by_id(conn: &Connection, id: i64) -> Result<Option<Task>> {
    conn.query_row(
        "SELECT id, date, sort_order, title, category_id, duration_min,
                fixed_start, actual_start, actual_end, recurrence_id, deadline
         FROM tasks WHERE id = ?1",
        params![id],
        |row| {
            Ok(Task {
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

fn restore_task(conn: &Connection, task: &Task) -> Result<()> {
    let existing = load_task_by_id(conn, task.id)?;
    if existing.is_some() {
        return Ok(());
    }

    conn.execute(
        "UPDATE tasks
         SET sort_order = sort_order + 1
         WHERE date = ?1 AND sort_order >= ?2",
        params![task.date.as_str(), task.sort_order],
    )?;

    conn.execute(
        "INSERT INTO tasks (
            id, date, sort_order, title, category_id, duration_min,
            fixed_start, actual_start, actual_end, recurrence_id, deadline
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            task.id,
            task.date.as_str(),
            task.sort_order,
            task.title.as_str(),
            task.category_id.as_str(),
            task.duration_min,
            task.fixed_start,
            task.actual_start,
            task.actual_end,
            task.recurrence_id,
            task.deadline.as_deref(),
        ],
    )?;

    normalize_sort_order(conn, &task.date)?;
    Ok(())
}

fn normalize_sort_order(conn: &Connection, date: &str) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT id FROM tasks WHERE date = ?1 ORDER BY sort_order, id ASC")?;
    let ids = stmt
        .query_map(params![date], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if ids.is_empty() {
        return Ok(());
    }

    conn.execute(
        "UPDATE tasks SET sort_order = sort_order + 1000000 WHERE date = ?1",
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
