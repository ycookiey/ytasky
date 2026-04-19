use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, schemars, tool, tool_handler, tool_router,
};
use serde_json::json;
use tokio::sync::RwLock;
use ybasey::Database;

use crate::{db, model};

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
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() as i32 * 60 + now.minute() as i32
}

fn ok_json(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap(),
    )]))
}

fn db_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
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
                "deadline": t.deadline,
            })
        })
        .collect()
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddBacklogParams {
    /// Task title
    title: String,
    /// Category ID: work, study, sleep, meal, exercise, personal, break, commute, errand
    category: String,
    /// Duration in minutes (multiples of 15)
    duration: i32,
    /// Deadline in YYYY-MM-DD or "YYYY-MM-DD HH:MM" format (optional)
    #[serde(default)]
    deadline: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EditBacklogParams {
    /// Task ID to edit
    id: i64,
    /// New title (optional)
    #[serde(default)]
    title: Option<String>,
    /// New category ID (optional)
    #[serde(default)]
    category: Option<String>,
    /// New duration in minutes (optional)
    #[serde(default)]
    duration: Option<i32>,
    /// New deadline in YYYY-MM-DD or "YYYY-MM-DD HH:MM" format, or "none" to remove (optional)
    #[serde(default)]
    deadline: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ScheduleBacklogParams {
    /// Backlog task ID to insert into schedule
    id: i64,
    /// Target date in YYYY-MM-DD format. Defaults to today.
    #[serde(default)]
    date: Option<String>,
    /// Position to insert at (0-based index). If omitted, appends to end.
    #[serde(default)]
    position: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ToBacklogParams {
    /// Scheduled task ID to move to backlog
    id: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddRecurrenceParams {
    /// Task title
    title: String,
    /// Category ID
    category: String,
    /// Duration in minutes
    duration: i32,
    /// Fixed start time in HH:MM format (optional)
    #[serde(default)]
    fixed_start: Option<String>,
    /// Pattern: daily, weekly, monthly
    pattern: String,
    /// Pattern data JSON (optional), e.g. {"days":[1,3,5]}
    #[serde(default)]
    pattern_data: Option<String>,
    /// Start date in YYYY-MM-DD
    start_date: String,
    /// End date in YYYY-MM-DD (optional)
    #[serde(default)]
    end_date: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EditRecurrenceParams {
    /// Recurrence rule ID
    id: i64,
    /// New title (optional)
    #[serde(default)]
    title: Option<String>,
    /// New category ID (optional)
    #[serde(default)]
    category: Option<String>,
    /// New duration in minutes (optional)
    #[serde(default)]
    duration: Option<i32>,
    /// New fixed start in HH:MM, or "none" (optional)
    #[serde(default)]
    fixed_start: Option<String>,
    /// New pattern (optional)
    #[serde(default)]
    pattern: Option<String>,
    /// New pattern data JSON, or "none" (optional)
    #[serde(default)]
    pattern_data: Option<String>,
    /// New start date YYYY-MM-DD (optional)
    #[serde(default)]
    start_date: Option<String>,
    /// New end date YYYY-MM-DD, or "none" (optional)
    #[serde(default)]
    end_date: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RecurrenceIdParams {
    /// Recurrence rule ID
    id: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct HistoryParams {
    /// Number of entries to return (default: 20)
    #[serde(default)]
    limit: Option<usize>,
    /// Table filter (optional)
    #[serde(default)]
    table: Option<String>,
}

// --- MCP Server ---

#[derive(Clone)]
pub struct YtaskyMcpServer {
    db: Arc<RwLock<Database>>,
    tool_router: ToolRouter<YtaskyMcpServer>,
}

#[tool_router]
impl YtaskyMcpServer {
    pub fn new(db: Database) -> Self {
        Self {
            db: Arc::new(RwLock::new(db)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List tasks for a given date. Returns all tasks with their IDs, times, durations, categories, and actual start/end times."
    )]
    async fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let mut db = self.db.write().await;
        let tasks = db::load_tasks(&mut db, &date).map_err(db_err)?;
        ok_json(json!({ "date": date, "tasks": serialize_tasks(&tasks) }))
    }

    #[tool(
        description = "Add a new task. Duration should be in minutes (multiples of 15). Category must be one of: work, study, sleep, meal, exercise, personal, break, commute, errand."
    )]
    async fn add_task(
        &self,
        Parameters(params): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let fixed = params.fixed_start.map(|s| parse_time(&s)).transpose()?;
        let mut db = self.db.write().await;
        let id = db::insert_task(&mut db, &date, &params.title, &params.category, params.duration, fixed)
            .map_err(db_err)?;
        ok_json(json!({ "id": id, "date": date }))
    }

    #[tool(
        description = "Edit an existing task. Only specified fields are updated; omitted fields keep their current values. Use fixed_start=\"none\" to remove a fixed start time."
    )]
    async fn edit_task(
        &self,
        Parameters(params): Parameters<EditTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        let task = db::load_task_by_id(&db, params.id)
            .map_err(db_err)?
            .ok_or_else(|| McpError::invalid_params("タスクが見つからない", None))?;

        let new_title = params.title.as_deref().unwrap_or(&task.title);
        let new_category = params.category.as_deref().unwrap_or(&task.category_id);
        let new_duration = params.duration.unwrap_or(task.duration_min);
        let new_fixed = match &params.fixed_start {
            Some(s) if s == "none" => None,
            Some(s) => Some(parse_time(s)?),
            None => task.fixed_start,
        };

        db::update_task(&mut db, params.id, new_title, new_category, new_duration, new_fixed)
            .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Delete a task by ID.")]
    async fn delete_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        db::delete_task(&mut db, params.id).map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Start a task. Sets actual_start to the current time.")]
    async fn start_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mins = current_minutes();
        let mut db = self.db.write().await;
        db::update_actual(&mut db, params.id, Some(mins), None).map_err(db_err)?;
        ok_json(json!({
            "ok": true,
            "id": params.id,
            "actual_start": model::format_time(mins),
        }))
    }

    #[tool(
        description = "Complete a task. Sets actual_end to the current time. If actual_start was not set, it is also set to the current time."
    )]
    async fn done_task(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mins = current_minutes();
        let mut db = self.db.write().await;
        let current_start = db::load_task_by_id(&db, params.id)
            .map_err(db_err)?
            .ok_or_else(|| McpError::invalid_params("タスクが見つからない", None))?
            .actual_start;
        let start = current_start.unwrap_or(mins);
        db::update_actual(&mut db, params.id, Some(start), Some(mins)).map_err(db_err)?;
        ok_json(json!({
            "ok": true,
            "id": params.id,
            "actual_start": model::format_time(start),
            "actual_end": model::format_time(mins),
        }))
    }

    #[tool(
        description = "Swap the position (sort_order) of two tasks. Both tasks must be on the same date."
    )]
    async fn move_task(
        &self,
        Parameters(params): Parameters<MoveTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        db::swap_sort_order(&mut db, params.id, params.swap_with).map_err(db_err)?;
        ok_json(json!({ "ok": true }))
    }

    #[tool(
        description = "Get a summary report for a date, showing planned and actual minutes by category and by task title."
    )]
    async fn report(
        &self,
        Parameters(params): Parameters<ReportParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let db = self.db.read().await;
        let by_cat = db::report_by_category(&db, &date).map_err(db_err)?;
        let by_title = db::report_by_title(&db, &date).map_err(db_err)?;
        ok_json(json!({
            "date": date,
            "by_category": by_cat,
            "by_title": by_title,
        }))
    }

    #[tool(description = "List all categories.")]
    async fn list_categories(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.read().await;
        let cats = db::load_categories(&db).map_err(db_err)?;
        ok_json(json!({ "categories": cats }))
    }

    #[tool(description = "List all backlog tasks ordered by deadline.")]
    async fn list_backlog(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.read().await;
        let tasks = db::load_backlog_tasks(&db).map_err(db_err)?;
        ok_json(json!({ "tasks": serialize_tasks(&tasks) }))
    }

    #[tool(description = "Add a new task to the backlog.")]
    async fn add_backlog(
        &self,
        Parameters(params): Parameters<AddBacklogParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        let id = db::insert_backlog_task(&mut db, &params.title, &params.category, params.duration, params.deadline.as_deref())
            .map_err(db_err)?;
        ok_json(json!({ "id": id }))
    }

    #[tool(
        description = "Edit a backlog task. Only specified fields are updated. Use deadline=\"none\" to remove the deadline."
    )]
    async fn edit_backlog(
        &self,
        Parameters(params): Parameters<EditBacklogParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        let task = db::load_task_by_id(&db, params.id)
            .map_err(db_err)?
            .ok_or_else(|| McpError::invalid_params("タスクが見つからない", None))?;

        let new_title = params.title.as_deref().unwrap_or(&task.title);
        let new_category = params.category.as_deref().unwrap_or(&task.category_id);
        let new_duration = params.duration.unwrap_or(task.duration_min);
        let new_deadline = match &params.deadline {
            Some(s) if s == "none" => None,
            Some(s) => Some(s.as_str()),
            None => task.deadline.as_deref(),
        };

        db::update_task_with_deadline(&mut db, params.id, new_title, new_category, new_duration, task.fixed_start, new_deadline)
            .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Delete a backlog task by ID.")]
    async fn delete_backlog(
        &self,
        Parameters(params): Parameters<TaskIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        db::delete_task(&mut db, params.id).map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(
        description = "Move a backlog task into a day's schedule. Optionally specify position (0-based). Defaults to appending at end."
    )]
    async fn schedule_backlog(
        &self,
        Parameters(params): Parameters<ScheduleBacklogParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = params.date.unwrap_or_else(today);
        let mut db = self.db.write().await;
        match params.position {
            Some(pos) => db::insert_backlog_task_at(&mut db, params.id, &date, pos),
            None => db::append_backlog_task(&mut db, params.id, &date),
        }
        .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id, "date": date }))
    }

    #[tool(
        description = "Move a scheduled task to backlog. The task keeps its title, category, duration, and deadline but is removed from the day's schedule."
    )]
    async fn to_backlog(
        &self,
        Parameters(params): Parameters<ToBacklogParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        db::set_backlog_flag(&mut db, params.id, true).map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(
        description = "Add a recurrence rule. Pattern must be one of: daily, weekly, monthly. pattern_data may contain JSON like {\"days\":[1,3,5]}."
    )]
    async fn add_recurrence(
        &self,
        Parameters(params): Parameters<AddRecurrenceParams>,
    ) -> Result<CallToolResult, McpError> {
        let fixed = params.fixed_start.map(|s| parse_time(&s)).transpose()?;
        let mut db = self.db.write().await;
        let id = db::insert_recurrence(
            &mut db,
            &params.title,
            &params.category,
            params.duration,
            fixed,
            &params.pattern,
            params.pattern_data.as_deref(),
            &params.start_date,
            params.end_date.as_deref(),
        )
        .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": id }))
    }

    #[tool(
        description = "Edit an existing recurrence rule. Omitted fields keep current values. Use fixed_start=\"none\" or end_date=\"none\" to clear values."
    )]
    async fn edit_recurrence(
        &self,
        Parameters(params): Parameters<EditRecurrenceParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        let current = db::load_recurrences(&db)
            .map_err(db_err)?
            .into_iter()
            .find(|item| item.id == params.id)
            .ok_or_else(|| McpError::invalid_params("繰り返しルールが見つからない", None))?;

        let new_title = params.title.unwrap_or(current.title);
        let new_category = params.category.unwrap_or(current.category_id);
        let new_duration = params.duration.unwrap_or(current.duration_min);
        let new_fixed_start = match params.fixed_start.as_deref() {
            Some("none") => None,
            Some(s) => Some(parse_time(s)?),
            None => current.fixed_start,
        };
        let new_pattern = params.pattern.unwrap_or(current.pattern);
        let new_pattern_data = match params.pattern_data.as_deref() {
            Some("none") => None,
            Some(data) => Some(data),
            None => current.pattern_data.as_deref(),
        };
        let new_start_date = params.start_date.unwrap_or(current.start_date);
        let new_end_date = match params.end_date.as_deref() {
            Some("none") => None,
            Some(date) => Some(date),
            None => current.end_date.as_deref(),
        };

        db::update_recurrence(
            &mut db,
            params.id,
            &new_title,
            &new_category,
            new_duration,
            new_fixed_start,
            &new_pattern,
            new_pattern_data,
            &new_start_date,
            new_end_date,
        )
        .map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "Delete a recurrence rule by ID.")]
    async fn delete_recurrence(
        &self,
        Parameters(params): Parameters<RecurrenceIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut db = self.db.write().await;
        db::delete_recurrence(&mut db, params.id).map_err(db_err)?;
        ok_json(json!({ "ok": true, "id": params.id }))
    }

    #[tool(description = "List all recurrence rules.")]
    async fn list_recurrences(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.read().await;
        let recurrences = db::load_recurrences(&db).map_err(db_err)?;
        ok_json(json!({ "recurrences": recurrences }))
    }

    #[tool(description = "Get recent operation history. Returns raw log entries (newest first).")]
    async fn history(
        &self,
        Parameters(params): Parameters<HistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20);
        let db = self.db.read().await;
        let entries = db::query_history(&db, params.table.as_deref(), limit).map_err(db_err)?;
        ok_json(json!({ "entries": entries }))
    }
}

#[tool_handler]
impl ServerHandler for YtaskyMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "ytasky is a time-blocking scheduler. Tasks have a date, title, category, \
                 duration (minutes), optional fixed start time, and actual start/end times. \
                 Categories: work, study, sleep, meal, exercise, personal, break, commute, errand. \
                 Backlog tasks are unscheduled tasks with optional deadlines that can be inserted \
                 into any day's schedule. Recurrence rules can generate tasks automatically with \
                 daily/weekly/monthly patterns."
                    .into(),
            ),
        }
    }
}
