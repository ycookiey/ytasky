use std::sync::Mutex;

use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, schemars, tool, tool_handler, tool_router,
};
use rusqlite::Connection;
use serde_json::json;

use crate::{db, model};

/// HH:MM → 分に変換
fn parse_time(s: &str) -> Result<i32, McpError> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(McpError::invalid_params(
            format!("時刻形式が不正: {s} (HH:MM を使用)"),
            None,
        ));
    }
    let h: i32 = parts[0]
        .parse()
        .map_err(|_| McpError::invalid_params("時が不正", None))?;
    let m: i32 = parts[1]
        .parse()
        .map_err(|_| McpError::invalid_params("分が不正", None))?;
    Ok(h * 60 + m)
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn current_minutes() -> i32 {
    let now = chrono::Local::now();
    now.format("%H").to_string().parse::<i32>().unwrap() * 60
        + now.format("%M").to_string().parse::<i32>().unwrap()
}

fn ok_json(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap(),
    )]))
}

fn db_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

// --- Tool parameter types ---

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListTasksParams {
    /// Target date in YYYY-MM-DD format. Defaults to today.
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddTaskParams {
    /// Task title
    title: String,
    /// Category ID: work, study, sleep, meal, exercise, personal, break, commute, errand
    category: String,
    /// Duration in minutes (multiples of 15)
    duration: i32,
    /// Target date in YYYY-MM-DD format. Defaults to today.
    #[serde(default)]
    date: Option<String>,
    /// Fixed start time in HH:MM format (optional)
    #[serde(default)]
    fixed_start: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EditTaskParams {
    /// Task ID to edit
    id: i64,
    /// New title (optional, keeps current if omitted)
    #[serde(default)]
    title: Option<String>,
    /// New category ID (optional)
    #[serde(default)]
    category: Option<String>,
    /// New duration in minutes (optional)
    #[serde(default)]
    duration: Option<i32>,
    /// New fixed start time in HH:MM format, or "none" to remove (optional)
    #[serde(default)]
    fixed_start: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TaskIdParams {
    /// Task ID
    id: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct MoveTaskParams {
    /// Task ID to move
    id: i64,
    /// Task ID to swap position with
    swap_with: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReportParams {
    /// Target date in YYYY-MM-DD format. Defaults to today.
    #[serde(default)]
    date: Option<String>,
}

// --- MCP Server ---

#[derive(Clone)]
pub struct YtaskyMcp {
    conn: std::sync::Arc<Mutex<Connection>>,
    tool_router: ToolRouter<YtaskyMcp>,
}

#[tool_router]
impl YtaskyMcp {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: std::sync::Arc::new(Mutex::new(conn)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List tasks for a given date. Returns all tasks with their IDs, times, durations, categories, and actual start/end times."
    )]
    fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let conn = self.conn.lock().unwrap();
        let tasks = db::load_tasks(&conn, &date).map_err(db_err)?;
        ok_json(json!({ "date": date, "tasks": serialize_tasks(&tasks) }))
    }

    #[tool(
        description = "Add a new task. Duration should be in minutes (multiples of 15). Category must be one of: work, study, sleep, meal, exercise, personal, break, commute, errand."
    )]
    fn add_task(
        &self,
        Parameters(params): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let fixed = params.fixed_start.map(|s| parse_time(&s)).transpose()?;
        let conn = self.conn.lock().unwrap();
        let id = db::insert_task(
            &conn,
            &date,
            &params.title,
            &params.category,
            params.duration,
            fixed,
        )
        .map_err(db_err)?;
        ok_json(json!({ "id": id, "date": date }))
    }

    #[tool(
        description = "Edit an existing task. Only specified fields are updated; omitted fields keep their current values. Use fixed_start=\"none\" to remove a fixed start time."
    )]
    fn edit_task(
        &self,
        Parameters(params): Parameters<EditTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self.conn.lock().unwrap();
        let task_date: String = conn
            .query_row("SELECT date FROM tasks WHERE id = ?1", [params.id], |r| {
                r.get(0)
            })
            .map_err(|_| McpError::invalid_params("タスクが見つからない", None))?;
        let tasks = db::load_tasks(&conn, &task_date).map_err(db_err)?;
        let task = tasks
            .iter()
            .find(|t| t.id == params.id)
            .ok_or_else(|| McpError::invalid_params("タスクが見つからない", None))?;

        let new_title = params.title.as_deref().unwrap_or(&task.title);
        let new_category = params.category.as_deref().unwrap_or(&task.category_id);
        let new_duration = params.duration.unwrap_or(task.duration_min);
        let new_fixed = match &params.fixed_start {
            Some(s) if s == "none" => None,
            Some(s) => Some(parse_time(s)?),
            None => task.fixed_start,
        };

        db::update_task(
            &conn,
            params.id,
            new_title,
            new_category,
            new_duration,
            new_fixed,
        )
        .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Delete a task by ID.")]
    fn delete_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self.conn.lock().unwrap();
        db::delete_task(&conn, params.id).map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Start a task. Sets actual_start to the current time.")]
    fn start_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mins = current_minutes();
        let conn = self.conn.lock().unwrap();
        db::update_actual(&conn, params.id, Some(mins), None).map_err(db_err)?;
        ok_json(json!({
            "ok": true,
            "id": params.id,
            "actual_start": model::format_time(mins),
        }))
    }

    #[tool(
        description = "Complete a task. Sets actual_end to the current time. If actual_start was not set, it is also set to the current time."
    )]
    fn complete_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mins = current_minutes();
        let conn = self.conn.lock().unwrap();
        let current_start: Option<i32> = conn
            .query_row(
                "SELECT actual_start FROM tasks WHERE id = ?1",
                [params.id],
                |r| r.get(0),
            )
            .map_err(|_| McpError::invalid_params("タスクが見つからない", None))?;
        let start = current_start.unwrap_or(mins);
        db::update_actual(&conn, params.id, Some(start), Some(mins)).map_err(db_err)?;
        ok_json(json!({
            "ok": true,
            "id": params.id,
            "actual_start": model::format_time(start),
            "actual_end": model::format_time(mins),
        }))
    }

    #[tool(
        description = "Swap the position of two tasks (reorder). Both tasks must be on the same date."
    )]
    fn move_task(
        &self,
        Parameters(params): Parameters<MoveTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self.conn.lock().unwrap();
        db::swap_sort_order(&conn, params.id, params.swap_with).map_err(db_err)?;
        ok_json(json!({ "ok": true }))
    }

    #[tool(
        description = "Get a daily report with time aggregation by category and by title. Shows planned vs actual minutes."
    )]
    fn get_report(
        &self,
        Parameters(params): Parameters<ReportParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let conn = self.conn.lock().unwrap();
        let by_cat = db::report_by_category(&conn, &date).map_err(db_err)?;
        let by_title = db::report_by_title(&conn, &date).map_err(db_err)?;
        ok_json(json!({
            "date": date,
            "by_category": by_cat,
            "by_title": by_title,
        }))
    }

    #[tool(description = "List all available task categories with their IDs and names.")]
    fn list_categories(&self) -> Result<CallToolResult, McpError> {
        let conn = self.conn.lock().unwrap();
        let cats = db::load_categories(&conn).map_err(db_err)?;
        ok_json(json!(cats))
    }
}

#[tool_handler]
impl ServerHandler for YtaskyMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "ytasky is a time-blocking scheduler. Tasks have a date, title, category, \
                 duration (minutes), optional fixed start time, and actual start/end times. \
                 Categories: work, study, sleep, meal, exercise, personal, break, commute, errand."
                    .into(),
            ),
        }
    }
}

fn serialize_tasks(tasks: &[model::Task]) -> Vec<serde_json::Value> {
    tasks
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "sort_order": t.sort_order,
                "title": t.title,
                "category_id": t.category_id,
                "duration_min": t.duration_min,
                "fixed_start": t.fixed_start.map(model::format_time),
                "actual_start": t.actual_start.map(model::format_time),
                "actual_end": t.actual_end.map(model::format_time),
                "recurrence_id": t.recurrence_id,
            })
        })
        .collect()
}
