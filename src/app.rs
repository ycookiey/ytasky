use anyhow::{Context, bail};
use chrono::{Datelike, Duration, Local, Months, NaiveDate, Timelike};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusqlite::Connection;

use crate::history::{
    Command, DeleteTaskCommand, ReorderTaskCommand, ToggleActualCommand, UndoManager,
};
use crate::model::{Category, CategoryReport, OverflowTask, Task, TitleReport};

const DAY_MINUTES: i32 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormMode {
    Add,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Table,
    Backlog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormTarget {
    Schedule,
    Backlog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    TableView,
    TimelineView,
    ReportView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Category,
    Duration,
    FixedStart,
    Deadline,
    Title,
}

#[derive(Debug, Clone)]
pub struct TaskFormState {
    pub target: FormTarget,
    pub mode: FormMode,
    pub task_id: Option<i64>,
    pub title: String,
    pub is_title_custom: bool,
    pub category_idx: usize,
    pub duration_input: String,
    pub fixed_start: Option<i32>,
    pub deadline: Option<String>,
    pub deadline_input: String,
    pub field: FormField,
}

#[derive(Debug, Clone)]
pub enum InputMode {
    Normal,
    TaskForm(TaskFormState),
    ConfirmDelete { task_id: i64, title: String },
    BacklogSelect { cursor: usize },
}

enum FormAction {
    KeepOpen,
    Submit,
    Cancel,
}

pub struct App {
    conn: Connection,
    pub date: String,
    pub tasks: Vec<Task>,
    pub backlog_tasks: Vec<Task>,
    pub overflow_tasks: Vec<OverflowTask>,
    pub categories: Vec<Category>,
    pub category_reports: Vec<CategoryReport>,
    pub title_reports: Vec<TitleReport>,
    pub cursor: usize,
    pub backlog_cursor: usize,
    pub focus: PanelFocus,
    pub view_mode: ViewMode,
    pub should_quit: bool,
    pub input_mode: InputMode,
    pub status_message: Option<String>,
    undo_manager: UndoManager,
}

impl App {
    pub fn new(conn: Connection) -> anyhow::Result<Self> {
        let date = Local::now().format("%Y-%m-%d").to_string();
        let tasks = crate::db::load_tasks(&conn, &date)?;
        let backlog_tasks = crate::db::load_backlog_tasks(&conn)?;
        let overflow_tasks = Self::compute_overflow(&conn, &date)?;
        let categories = crate::db::load_categories(&conn)?;
        let category_reports = crate::db::report_by_category(&conn, &date)?;
        let title_reports = crate::db::report_by_title(&conn, &date)?;

        Ok(Self {
            conn,
            date,
            tasks,
            backlog_tasks,
            overflow_tasks,
            categories,
            category_reports,
            title_reports,
            cursor: 0,
            backlog_cursor: 0,
            focus: PanelFocus::Table,
            view_mode: ViewMode::TableView,
            should_quit: false,
            input_mode: InputMode::Normal,
            status_message: None,
            undo_manager: UndoManager::new(),
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if matches!(self.input_mode, InputMode::Normal) {
            self.handle_normal_key(key);
        } else {
            self.handle_modal_key(key);
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if self.view_mode == ViewMode::ReportView {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.view_mode = ViewMode::TableView;
                    return;
                }
                _ => return,
            }
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('u') => {
                if let Err(err) = self.undo() {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.view_mode = ViewMode::ReportView;
                return;
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(err) = self.redo() {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('h') => {
                if let Err(err) = self.shift_date(-1) {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('l') => {
                if let Err(err) = self.shift_date(1) {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('H') => {
                if let Err(err) = self.shift_date(-7) {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('L') => {
                if let Err(err) = self.shift_date(7) {
                    self.status_message = Some(err.to_string());
                }
                return;
            }
            KeyCode::Char('t') => {
                self.view_mode = match self.view_mode {
                    ViewMode::TableView => ViewMode::TimelineView,
                    ViewMode::TimelineView => ViewMode::TableView,
                    ViewMode::ReportView => ViewMode::TableView,
                };
                if self.view_mode != ViewMode::TableView {
                    self.focus = PanelFocus::Table;
                }
                return;
            }
            _ => {}
        }

        if self.view_mode == ViewMode::TableView {
            match self.focus {
                PanelFocus::Table => self.handle_table_panel_key(key),
                PanelFocus::Backlog => self.handle_backlog_panel_key(key),
            }
        } else {
            self.handle_timeline_key(key);
        }
    }

    fn handle_table_panel_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.tasks.is_empty() && self.cursor < self.tasks.len() - 1 {
                    self.cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            KeyCode::Char('a') => self.open_add_task_form(FormTarget::Schedule),
            KeyCode::Char('e') => self.open_edit_task_form(FormTarget::Schedule),
            KeyCode::Char('d') => self.open_delete_confirm(),
            KeyCode::Char('J') => {
                if let Err(err) = self.move_task_down() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char('K') => {
                if let Err(err) = self.move_task_up() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char(' ') => {
                if let Err(err) = self.toggle_actual_for_selected() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Tab => {
                self.focus = PanelFocus::Backlog;
            }
            KeyCode::Char('B') => {
                if let Err(err) = self.move_selected_task_to_backlog() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char('p') => {
                let cursor = self
                    .backlog_cursor
                    .min(self.backlog_tasks.len().saturating_sub(1));
                self.input_mode = InputMode::BacklogSelect { cursor };
            }
            _ => {}
        }
    }

    fn handle_backlog_panel_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.backlog_tasks.is_empty()
                    && self.backlog_cursor < self.backlog_tasks.len() - 1
                {
                    self.backlog_cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.backlog_cursor > 0 {
                    self.backlog_cursor -= 1;
                }
            }
            KeyCode::Enter => {
                if let Err(err) = self.insert_backlog_to_schedule_end() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char('d') => {
                if let Err(err) = self.delete_selected_backlog() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char('e') => self.open_edit_task_form(FormTarget::Backlog),
            KeyCode::Char('a') => self.open_add_task_form(FormTarget::Backlog),
            KeyCode::Tab => {
                self.focus = PanelFocus::Table;
            }
            _ => {}
        }
    }

    fn handle_timeline_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.tasks.is_empty() && self.cursor < self.tasks.len() - 1 {
                    self.cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            KeyCode::Char('a') => self.open_add_task_form(FormTarget::Schedule),
            KeyCode::Char('e') => self.open_edit_task_form(FormTarget::Schedule),
            KeyCode::Char('d') => self.open_delete_confirm(),
            KeyCode::Char('J') => {
                if let Err(err) = self.move_task_down() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char('K') => {
                if let Err(err) = self.move_task_up() {
                    self.status_message = Some(err.to_string());
                }
            }
            KeyCode::Char(' ') => {
                if let Err(err) = self.toggle_actual_for_selected() {
                    self.status_message = Some(err.to_string());
                }
            }
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let mode = std::mem::replace(&mut self.input_mode, InputMode::Normal);

        match mode {
            InputMode::TaskForm(mut form) => match self.handle_task_form_key(key, &mut form) {
                FormAction::KeepOpen => self.input_mode = InputMode::TaskForm(form),
                FormAction::Cancel => self.status_message = None,
                FormAction::Submit => {
                    if let Err(err) = self.submit_task_form(&form) {
                        self.status_message = Some(err.to_string());
                        self.input_mode = InputMode::TaskForm(form);
                    } else {
                        self.status_message = None;
                    }
                }
            },
            InputMode::ConfirmDelete { task_id, title } => match key.code {
                KeyCode::Esc => self.status_message = None,
                KeyCode::Enter => {
                    if let Err(err) = self.run_command(Box::new(DeleteTaskCommand::new(task_id))) {
                        self.status_message = Some(err.to_string());
                        self.input_mode = InputMode::ConfirmDelete { task_id, title };
                    } else {
                        self.status_message = None;
                    }
                }
                _ => self.input_mode = InputMode::ConfirmDelete { task_id, title },
            },
            InputMode::BacklogSelect { mut cursor } => match key.code {
                KeyCode::Esc => {
                    self.status_message = None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.backlog_tasks.is_empty() && cursor < self.backlog_tasks.len() - 1 {
                        cursor += 1;
                    }
                    self.backlog_cursor = cursor;
                    self.input_mode = InputMode::BacklogSelect { cursor };
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if cursor > 0 {
                        cursor -= 1;
                    }
                    self.backlog_cursor = cursor;
                    self.input_mode = InputMode::BacklogSelect { cursor };
                }
                KeyCode::Enter => {
                    if let Err(err) = self.insert_backlog_to_schedule_at_cursor(cursor) {
                        self.status_message = Some(err.to_string());
                        self.input_mode = InputMode::BacklogSelect { cursor };
                    } else {
                        self.status_message = None;
                    }
                }
                _ => {
                    self.input_mode = InputMode::BacklogSelect { cursor };
                }
            },
            InputMode::Normal => {}
        }
    }

    fn handle_task_form_key(&mut self, key: KeyEvent, form: &mut TaskFormState) -> FormAction {
        match key.code {
            KeyCode::Esc => FormAction::Cancel,
            KeyCode::Enter => match form.field {
                FormField::Category => {
                    form.field = FormField::Duration;
                    FormAction::KeepOpen
                }
                FormField::Duration => {
                    form.field = if form.target == FormTarget::Schedule {
                        FormField::FixedStart
                    } else {
                        FormField::Deadline
                    };
                    FormAction::KeepOpen
                }
                FormField::FixedStart => {
                    form.field = FormField::Deadline;
                    FormAction::KeepOpen
                }
                FormField::Deadline => {
                    form.field = FormField::Title;
                    FormAction::KeepOpen
                }
                FormField::Title => FormAction::Submit,
            },
            KeyCode::Tab => {
                form.field = match form.field {
                    FormField::Category => FormField::Duration,
                    FormField::Duration => {
                        if form.target == FormTarget::Schedule {
                            FormField::FixedStart
                        } else {
                            FormField::Deadline
                        }
                    }
                    FormField::FixedStart => FormField::Deadline,
                    FormField::Deadline => FormField::Title,
                    FormField::Title => FormField::Category,
                };
                FormAction::KeepOpen
            }
            KeyCode::Backspace => {
                match form.field {
                    FormField::Title => {
                        form.title.pop();
                        form.is_title_custom = true;
                    }
                    FormField::Duration => {
                        form.duration_input.pop();
                        if form.duration_input.is_empty() {
                            form.duration_input.push('0');
                        }
                    }
                    FormField::Deadline => {
                        if !form.deadline_input.is_empty() {
                            form.deadline_input.pop();
                            self.try_resolve_absolute_deadline_input(form);
                        } else {
                            form.deadline = None;
                        }
                    }
                    FormField::Category | FormField::FixedStart => {}
                }
                FormAction::KeepOpen
            }
            KeyCode::Up | KeyCode::Char('k') if form.field == FormField::Category => {
                self.select_prev_category(form);
                if !form.is_title_custom {
                    self.sync_title_with_selected_category(form);
                }
                FormAction::KeepOpen
            }
            KeyCode::Down | KeyCode::Char('j') if form.field == FormField::Category => {
                self.select_next_category(form);
                if !form.is_title_custom {
                    self.sync_title_with_selected_category(form);
                }
                FormAction::KeepOpen
            }
            KeyCode::Up | KeyCode::Char('k') if form.field == FormField::Duration => {
                Self::adjust_duration(form, 15);
                FormAction::KeepOpen
            }
            KeyCode::Down | KeyCode::Char('j') if form.field == FormField::Duration => {
                Self::adjust_duration(form, -15);
                FormAction::KeepOpen
            }
            KeyCode::Right | KeyCode::Char('l') if form.field == FormField::Duration => {
                Self::adjust_duration(form, 60);
                FormAction::KeepOpen
            }
            KeyCode::Left | KeyCode::Char('h') if form.field == FormField::Duration => {
                Self::adjust_duration(form, -60);
                FormAction::KeepOpen
            }
            KeyCode::Up | KeyCode::Char('k') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, -15);
                FormAction::KeepOpen
            }
            KeyCode::Down | KeyCode::Char('j') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, 15);
                FormAction::KeepOpen
            }
            KeyCode::Right | KeyCode::Char('l') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, 60);
                FormAction::KeepOpen
            }
            KeyCode::Left | KeyCode::Char('h') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, -60);
                FormAction::KeepOpen
            }
            KeyCode::Char('L') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, 360);
                FormAction::KeepOpen
            }
            KeyCode::Char('H') if form.field == FormField::FixedStart => {
                self.adjust_fixed_start(form, -360);
                FormAction::KeepOpen
            }
            KeyCode::Char('n') if form.field == FormField::FixedStart => {
                form.fixed_start = None;
                FormAction::KeepOpen
            }
            KeyCode::Up | KeyCode::Char('k') if form.field == FormField::Deadline => {
                self.shift_deadline(form, -1);
                FormAction::KeepOpen
            }
            KeyCode::Down | KeyCode::Char('j') if form.field == FormField::Deadline => {
                self.shift_deadline(form, 1);
                FormAction::KeepOpen
            }
            KeyCode::Right | KeyCode::Char('l') if form.field == FormField::Deadline => {
                self.shift_deadline(form, 7);
                FormAction::KeepOpen
            }
            KeyCode::Left | KeyCode::Char('h') if form.field == FormField::Deadline => {
                self.shift_deadline(form, -7);
                FormAction::KeepOpen
            }
            KeyCode::Char('L') if form.field == FormField::Deadline => {
                self.shift_deadline(form, 30);
                FormAction::KeepOpen
            }
            KeyCode::Char('H') if form.field == FormField::Deadline => {
                self.shift_deadline(form, -30);
                FormAction::KeepOpen
            }
            KeyCode::Char('n') if form.field == FormField::Deadline => {
                form.deadline = None;
                form.deadline_input.clear();
                FormAction::KeepOpen
            }
            KeyCode::Char(c) => {
                match form.field {
                    FormField::Title => {
                        if !c.is_control() {
                            form.title.push(c);
                            form.is_title_custom = true;
                        }
                    }
                    FormField::Duration => {
                        if c.is_ascii_digit() {
                            if form.duration_input == "0" {
                                form.duration_input.clear();
                            }
                            if form.duration_input.len() < 4 {
                                form.duration_input.push(c);
                            }
                        }
                    }
                    FormField::FixedStart => {
                        if c.is_ascii_digit() {
                            let digit = c as i32 - '0' as i32;
                            let current_hour = form.fixed_start.unwrap_or(0) / 60;
                            let new_hour = current_hour % 10 * 10 + digit;
                            if new_hour < 24 {
                                form.fixed_start = Some(new_hour * 60);
                            } else {
                                form.fixed_start = Some(digit * 60);
                            }
                        }
                    }
                    FormField::Deadline => {
                        if c.is_ascii_digit() {
                            if form.deadline_input.len() < 8 {
                                form.deadline_input.push(c);
                            }
                            self.try_resolve_absolute_deadline_input(form);
                        } else if matches!(c, 'd' | 'w' | 'm') {
                            if let Err(err) = self.apply_relative_deadline_input(form, c) {
                                self.status_message = Some(err.to_string());
                            } else {
                                self.status_message = None;
                            }
                        }
                    }
                    FormField::Category => {}
                }
                FormAction::KeepOpen
            }
            _ => FormAction::KeepOpen,
        }
    }

    fn submit_task_form(&mut self, form: &TaskFormState) -> anyhow::Result<()> {
        let title = form.title.trim();
        if title.is_empty() {
            bail!("タイトルを入力してください");
        }

        let duration_min: i32 = form
            .duration_input
            .parse()
            .context("見積もり時間は数値で入力してください")?;

        if duration_min <= 0 || duration_min % 15 != 0 {
            bail!("見積もり時間は15分単位で入力してください");
        }

        let category = self
            .categories
            .get(form.category_idx)
            .context("カテゴリが見つかりません")?;
        let deadline = self.resolve_deadline_for_submit(form)?;

        match (form.target, form.mode) {
            (FormTarget::Schedule, FormMode::Add) => {
                let command = AddScheduledTaskCommand::new(
                    self.date.clone(),
                    title.to_string(),
                    category.id.clone(),
                    duration_min,
                    form.fixed_start,
                    deadline.clone(),
                );
                self.run_command(Box::new(command))?;
                if !self.tasks.is_empty() {
                    self.cursor = self.tasks.len() - 1;
                }
            }
            (FormTarget::Schedule, FormMode::Edit) => {
                let task_id = form.task_id.context("編集対象タスクが不正です")?;
                let command = EditScheduledTaskCommand::new(
                    task_id,
                    title.to_string(),
                    category.id.clone(),
                    duration_min,
                    form.fixed_start,
                    deadline.clone(),
                );
                self.run_command(Box::new(command))?;
                if let Some(idx) = self.tasks.iter().position(|task| task.id == task_id) {
                    self.cursor = idx;
                }
            }
            (FormTarget::Backlog, FormMode::Add) => {
                let command = AddBacklogCommand::new(
                    title.to_string(),
                    category.id.clone(),
                    duration_min,
                    deadline.clone(),
                );
                self.run_command(Box::new(command))?;
                if !self.backlog_tasks.is_empty() {
                    self.backlog_cursor = self.backlog_tasks.len() - 1;
                }
            }
            (FormTarget::Backlog, FormMode::Edit) => {
                let task_id = form.task_id.context("編集対象タスクが不正です")?;
                let command = UpdateBacklogCommand::new(
                    task_id,
                    title.to_string(),
                    category.id.clone(),
                    duration_min,
                    deadline,
                );
                self.run_command(Box::new(command))?;
                if let Some(idx) = self
                    .backlog_tasks
                    .iter()
                    .position(|task| task.id == task_id)
                {
                    self.backlog_cursor = idx;
                }
            }
        }

        Ok(())
    }

    fn adjust_duration(form: &mut TaskFormState, delta: i32) {
        let current: i32 = form.duration_input.parse().unwrap_or(0);
        let new_val = (current + delta).clamp(15, 1440);
        form.duration_input = new_val.to_string();
    }

    fn adjust_fixed_start(&self, form: &mut TaskFormState, delta: i32) {
        let base = form
            .fixed_start
            .unwrap_or_else(|| self.default_fixed_start());
        let next = normalize_fixed_start_time(base + delta);
        form.fixed_start = Some(next);
    }

    fn default_fixed_start(&self) -> i32 {
        if self.tasks.is_empty() {
            return round_up_to_next_15_minutes(current_minutes());
        }

        let mut current_min = 0;
        for task in &self.tasks {
            let start = match task.fixed_start {
                Some(fixed_start) => normalize_schedule_fixed_start(fixed_start, current_min),
                None => current_min,
            };
            current_min = start + task.duration_min;
        }

        normalize_fixed_start_time(current_min)
    }

    fn open_add_task_form(&mut self, target: FormTarget) {
        if self.categories.is_empty() {
            self.status_message = Some("カテゴリが未定義です".to_string());
            return;
        }

        let default_title = self.categories[0].name.clone();

        self.input_mode = InputMode::TaskForm(TaskFormState {
            target,
            mode: FormMode::Add,
            task_id: None,
            title: default_title,
            is_title_custom: false,
            category_idx: 0,
            duration_input: "60".to_string(),
            fixed_start: None,
            deadline: None,
            deadline_input: String::new(),
            field: FormField::Category,
        });
    }

    fn open_edit_task_form(&mut self, target: FormTarget) {
        if self.categories.is_empty() {
            self.status_message = Some("カテゴリが未定義です".to_string());
            return;
        }

        let source = match target {
            FormTarget::Schedule => self.selected_task().cloned(),
            FormTarget::Backlog => self.selected_backlog_task().cloned(),
        };
        let Some(task) = source else {
            return;
        };

        let mut category_idx = 0;
        let mut is_title_custom = true;
        if let Some((idx, category)) = self
            .categories
            .iter()
            .enumerate()
            .find(|(_, cat)| cat.id == task.category_id)
        {
            category_idx = idx;
            is_title_custom = task.title != category.name;
        }

        self.input_mode = InputMode::TaskForm(TaskFormState {
            target,
            mode: FormMode::Edit,
            task_id: Some(task.id),
            title: task.title,
            is_title_custom,
            category_idx,
            duration_input: task.duration_min.to_string(),
            fixed_start: if target == FormTarget::Schedule {
                task.fixed_start
            } else {
                None
            },
            deadline: task.deadline,
            deadline_input: String::new(),
            field: FormField::Category,
        });
    }

    fn open_delete_confirm(&mut self) {
        let Some(task) = self.selected_task() else {
            return;
        };

        self.input_mode = InputMode::ConfirmDelete {
            task_id: task.id,
            title: task.title.clone(),
        };
    }

    fn select_next_category(&self, form: &mut TaskFormState) {
        if self.categories.is_empty() {
            return;
        }
        form.category_idx = (form.category_idx + 1) % self.categories.len();
    }

    fn select_prev_category(&self, form: &mut TaskFormState) {
        if self.categories.is_empty() {
            return;
        }
        if form.category_idx == 0 {
            form.category_idx = self.categories.len() - 1;
        } else {
            form.category_idx -= 1;
        }
    }

    fn sync_title_with_selected_category(&self, form: &mut TaskFormState) {
        if let Some(category) = self.categories.get(form.category_idx) {
            form.title = category.name.clone();
        }
    }

    fn resolve_deadline_for_submit(&self, form: &TaskFormState) -> anyhow::Result<Option<String>> {
        if form.deadline_input.is_empty() {
            return Ok(form.deadline.clone());
        }

        if form.deadline_input.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Some(parse_absolute_deadline(&form.deadline_input)?));
        }

        bail!("期限入力が不正です（MMDD / MMDDhh / Nd/Nw/Nm）");
    }

    fn try_resolve_absolute_deadline_input(&mut self, form: &mut TaskFormState) {
        if form.deadline_input.len() != 4 && form.deadline_input.len() != 6 {
            if !form.deadline_input.is_empty() {
                self.status_message = None;
            }
            return;
        }

        if !form.deadline_input.chars().all(|c| c.is_ascii_digit()) {
            self.status_message = Some("期限は数字で入力してください".to_string());
            return;
        }

        match parse_absolute_deadline(&form.deadline_input) {
            Ok(value) => {
                form.deadline = Some(value);
                if form.deadline_input.len() == 6 {
                    form.deadline_input.clear();
                }
                self.status_message = None;
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
    }

    fn apply_relative_deadline_input(
        &mut self,
        form: &mut TaskFormState,
        unit: char,
    ) -> anyhow::Result<()> {
        let raw = form.deadline_input.trim();
        if raw.is_empty() {
            bail!("期限の相対指定は数字の後に d/w/m を入力してください");
        }
        if !raw.chars().all(|c| c.is_ascii_digit()) {
            bail!("期限の相対指定は数字のみ使用できます");
        }
        let amount: i64 = raw.parse().context("期限の相対指定が不正です")?;
        if amount <= 0 {
            bail!("期限の相対指定は1以上を入力してください");
        }

        let base = Local::now().date_naive();
        let next = match unit {
            'd' => base + Duration::days(amount),
            'w' => base + Duration::days(amount * 7),
            'm' => base
                .checked_add_months(Months::new(amount as u32))
                .context("期限の月指定が不正です")?,
            _ => bail!("未対応の期限単位です"),
        };

        form.deadline = Some(next.format("%Y-%m-%d").to_string());
        form.deadline_input.clear();
        Ok(())
    }

    fn shift_deadline(&mut self, form: &mut TaskFormState, delta_days: i64) {
        if !form.deadline_input.is_empty() {
            self.try_resolve_absolute_deadline_input(form);
            form.deadline_input.clear();
        }

        let current = form
            .deadline
            .as_deref()
            .and_then(parse_deadline_date_time)
            .unwrap_or((Local::now().date_naive(), None));

        let shifted = current.0 + Duration::days(delta_days);
        let new_deadline = match current.1 {
            Some(time) => format!("{} {}", shifted.format("%Y-%m-%d"), time),
            None => shifted.format("%Y-%m-%d").to_string(),
        };
        form.deadline = Some(new_deadline);
        self.status_message = None;
    }

    fn selected_backlog_task(&self) -> Option<&Task> {
        self.backlog_tasks.get(self.backlog_cursor)
    }

    fn move_selected_task_to_backlog(&mut self) -> anyhow::Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };
        let command = MoveToBacklogCommand::new(task.id);
        self.run_command(Box::new(command))
    }

    fn insert_backlog_to_schedule_end(&mut self) -> anyhow::Result<()> {
        let Some(task) = self.selected_backlog_task().cloned() else {
            return Ok(());
        };
        let command = InsertFromBacklogCommand::new(task.id, self.date.clone(), None);
        self.run_command(Box::new(command))
    }

    fn insert_backlog_to_schedule_at_cursor(
        &mut self,
        backlog_cursor: usize,
    ) -> anyhow::Result<()> {
        let Some(task) = self.backlog_tasks.get(backlog_cursor).cloned() else {
            return Ok(());
        };
        let insert_at = self.cursor.min(self.tasks.len());
        let command = InsertFromBacklogCommand::new(task.id, self.date.clone(), Some(insert_at));
        self.run_command(Box::new(command))
    }

    fn delete_selected_backlog(&mut self) -> anyhow::Result<()> {
        let Some(task) = self.selected_backlog_task() else {
            return Ok(());
        };
        self.run_command(Box::new(DeleteBacklogCommand::new(task.id)))
    }

    fn move_task_down(&mut self) -> anyhow::Result<()> {
        if self.tasks.is_empty() || self.cursor >= self.tasks.len() - 1 {
            return Ok(());
        }

        let id1 = self.tasks[self.cursor].id;
        let id2 = self.tasks[self.cursor + 1].id;
        self.run_command(Box::new(ReorderTaskCommand::new(id1, id2)))?;
        self.cursor = (self.cursor + 1).min(self.tasks.len().saturating_sub(1));

        Ok(())
    }

    fn move_task_up(&mut self) -> anyhow::Result<()> {
        if self.tasks.is_empty() || self.cursor == 0 {
            return Ok(());
        }

        let id1 = self.tasks[self.cursor].id;
        let id2 = self.tasks[self.cursor - 1].id;
        self.run_command(Box::new(ReorderTaskCommand::new(id1, id2)))?;
        self.cursor = self.cursor.saturating_sub(1);

        Ok(())
    }

    fn toggle_actual_for_selected(&mut self) -> anyhow::Result<()> {
        let Some(task) = self.selected_task().cloned() else {
            return Ok(());
        };

        let now_min = current_minutes();
        self.run_command(Box::new(ToggleActualCommand::new(&task, now_min)))?;

        Ok(())
    }

    fn run_command(&mut self, command: Box<dyn Command>) -> anyhow::Result<()> {
        self.undo_manager.execute_command(&self.conn, command)?;
        self.refresh_tasks()?;
        Ok(())
    }

    fn undo(&mut self) -> anyhow::Result<()> {
        if self.undo_manager.undo(&self.conn)? {
            self.refresh_tasks()?;
        }
        Ok(())
    }

    fn redo(&mut self) -> anyhow::Result<()> {
        if self.undo_manager.redo(&self.conn)? {
            self.refresh_tasks()?;
        }
        Ok(())
    }

    fn refresh_tasks(&mut self) -> anyhow::Result<()> {
        self.tasks = crate::db::load_tasks(&self.conn, &self.date)?;
        self.backlog_tasks = crate::db::load_backlog_tasks(&self.conn)?;
        self.overflow_tasks = Self::compute_overflow(&self.conn, &self.date)?;
        self.refresh_reports()?;

        if self.tasks.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.tasks.len() {
            self.cursor = self.tasks.len() - 1;
        }

        if self.backlog_tasks.is_empty() {
            self.backlog_cursor = 0;
        } else if self.backlog_cursor >= self.backlog_tasks.len() {
            self.backlog_cursor = self.backlog_tasks.len() - 1;
        }

        Ok(())
    }

    fn refresh_reports(&mut self) -> anyhow::Result<()> {
        self.category_reports = crate::db::report_by_category(&self.conn, &self.date)?;
        self.title_reports = crate::db::report_by_title(&self.conn, &self.date)?;
        Ok(())
    }

    fn shift_date(&mut self, days: i64) -> anyhow::Result<()> {
        let current = NaiveDate::parse_from_str(&self.date, "%Y-%m-%d")
            .with_context(|| format!("日付形式が不正です: {}", self.date))?;
        let next = current + Duration::days(days);
        self.date = next.format("%Y-%m-%d").to_string();
        self.refresh_tasks()?;
        self.cursor = 0;
        Ok(())
    }

    fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.cursor)
    }

    /// 前日のタスクが日を跨いでいる場合のはみ出し分を計算
    fn compute_overflow(conn: &Connection, date: &str) -> anyhow::Result<Vec<OverflowTask>> {
        let current = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .with_context(|| format!("日付形式が不正です: {date}"))?;
        let prev = current - Duration::days(1);
        let prev_date = prev.format("%Y-%m-%d").to_string();

        let prev_tasks = crate::db::load_tasks(conn, &prev_date)?;
        if prev_tasks.is_empty() {
            return Ok(Vec::new());
        }

        let mut current_min = 0i32;
        let mut overflow = Vec::new();

        for task in &prev_tasks {
            let start = match task.fixed_start {
                Some(fs) => normalize_schedule_fixed_start(fs, current_min),
                None => current_min,
            };
            let end = start + task.duration_min;

            if end > DAY_MINUTES {
                let overflow_start = if start >= DAY_MINUTES {
                    start - DAY_MINUTES
                } else {
                    0
                };
                let overflow_end = end - DAY_MINUTES;

                overflow.push(OverflowTask {
                    title: task.title.clone(),
                    category_id: task.category_id.clone(),
                    start_min: overflow_start,
                    end_min: overflow_end,
                });
            }

            current_min = end;
        }

        Ok(overflow)
    }

    /// 残り時間（分）を計算
    pub fn remaining_minutes(&self) -> i32 {
        let used_minutes: i64 = self.tasks.iter().map(|t| i64::from(t.duration_min)).sum();
        let remaining = i64::from(DAY_MINUTES) - used_minutes;
        remaining.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    pub fn is_today(&self) -> bool {
        self.date == Local::now().format("%Y-%m-%d").to_string()
    }

    pub fn undo_count(&self) -> usize {
        self.undo_manager.undo_len()
    }

    pub fn redo_count(&self) -> usize {
        self.undo_manager.redo_len()
    }

    pub fn backlog_select_cursor(&self) -> Option<usize> {
        match &self.input_mode {
            InputMode::BacklogSelect { cursor } => Some(*cursor),
            _ => None,
        }
    }
}

struct AddScheduledTaskCommand {
    date: String,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<String>,
    inserted_id: Option<i64>,
}

impl AddScheduledTaskCommand {
    fn new(
        date: String,
        title: String,
        category_id: String,
        duration_min: i32,
        fixed_start: Option<i32>,
        deadline: Option<String>,
    ) -> Self {
        Self {
            date,
            title,
            category_id,
            duration_min,
            fixed_start,
            deadline,
            inserted_id: None,
        }
    }
}

impl Command for AddScheduledTaskCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let id = crate::db::insert_task_with_deadline(
            conn,
            &self.date,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
            self.deadline.as_deref(),
        )?;
        self.inserted_id = Some(id);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        if let Some(id) = self.inserted_id {
            crate::db::delete_task(conn, id)?;
        }
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.execute(conn)
    }
}

struct EditScheduledTaskCommand {
    task_id: i64,
    title: String,
    category_id: String,
    duration_min: i32,
    fixed_start: Option<i32>,
    deadline: Option<String>,
    before: Option<Task>,
    after: Option<Task>,
}

impl EditScheduledTaskCommand {
    fn new(
        task_id: i64,
        title: String,
        category_id: String,
        duration_min: i32,
        fixed_start: Option<i32>,
        deadline: Option<String>,
    ) -> Self {
        Self {
            task_id,
            title,
            category_id,
            duration_min,
            fixed_start,
            deadline,
            before: None,
            after: None,
        }
    }
}

impl Command for EditScheduledTaskCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let before = crate::db::load_task_by_id(conn, self.task_id)?
            .context("編集対象タスクが見つかりません")?;
        crate::db::update_task_with_deadline(
            conn,
            self.task_id,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.fixed_start,
            self.deadline.as_deref(),
        )?;
        let after = crate::db::load_task_by_id(conn, self.task_id)?
            .context("編集後のタスク状態を取得できませんでした")?;
        self.before = Some(before);
        self.after = Some(after);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let before = self.before.as_ref().context("Undo対象がありません")?;
        crate::db::update_task_with_deadline(
            conn,
            before.id,
            &before.title,
            &before.category_id,
            before.duration_min,
            before.fixed_start,
            before.deadline.as_deref(),
        )?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let after = self.after.as_ref().context("Redo対象がありません")?;
        crate::db::update_task_with_deadline(
            conn,
            after.id,
            &after.title,
            &after.category_id,
            after.duration_min,
            after.fixed_start,
            after.deadline.as_deref(),
        )?;
        Ok(())
    }
}

struct MoveToBacklogCommand {
    task_id: i64,
    from_date: Option<String>,
    from_sort: Option<i32>,
}

impl MoveToBacklogCommand {
    fn new(task_id: i64) -> Self {
        Self {
            task_id,
            from_date: None,
            from_sort: None,
        }
    }
}

impl Command for MoveToBacklogCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        if self.from_date.is_none() || self.from_sort.is_none() {
            let Some((date, sort_order, is_backlog)) =
                crate::db::load_task_position(conn, self.task_id)?
            else {
                return Ok(());
            };
            if is_backlog {
                return Ok(());
            }
            self.from_date = Some(date);
            self.from_sort = Some(sort_order);
        }

        crate::db::set_backlog_flag(conn, self.task_id, true)?;
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let date = self.from_date.as_deref().context("元の日付がありません")?;
        let sort = self.from_sort.context("元の順序がありません")?;
        crate::db::restore_task_to_schedule(conn, self.task_id, date, sort)?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        crate::db::set_backlog_flag(conn, self.task_id, true)?;
        Ok(())
    }
}

struct InsertFromBacklogCommand {
    task_id: i64,
    target_date: String,
    target_index: Option<usize>,
    source_backlog_sort: Option<i32>,
    inserted_sort: Option<i32>,
}

impl InsertFromBacklogCommand {
    fn new(task_id: i64, target_date: String, target_index: Option<usize>) -> Self {
        Self {
            task_id,
            target_date,
            target_index,
            source_backlog_sort: None,
            inserted_sort: None,
        }
    }
}

impl Command for InsertFromBacklogCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        if self.source_backlog_sort.is_none() {
            let Some((_, sort_order, is_backlog)) =
                crate::db::load_task_position(conn, self.task_id)?
            else {
                return Ok(());
            };
            if !is_backlog {
                return Ok(());
            }
            self.source_backlog_sort = Some(sort_order);
        }

        if let Some(index) = self.target_index {
            crate::db::insert_backlog_task_at(conn, self.task_id, &self.target_date, index)?;
        } else {
            crate::db::append_backlog_task(conn, self.task_id, &self.target_date)?;
        }

        if let Some((_, sort_order, _)) = crate::db::load_task_position(conn, self.task_id)? {
            self.inserted_sort = Some(sort_order);
        }
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let sort = self
            .source_backlog_sort
            .context("元のバックログ順序がありません")?;
        crate::db::restore_task_to_backlog(conn, self.task_id, sort)?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        if let Some(sort) = self.inserted_sort {
            crate::db::insert_backlog_task_at(
                conn,
                self.task_id,
                &self.target_date,
                sort as usize,
            )?;
        } else if let Some(index) = self.target_index {
            crate::db::insert_backlog_task_at(conn, self.task_id, &self.target_date, index)?;
        } else {
            crate::db::append_backlog_task(conn, self.task_id, &self.target_date)?;
        }
        Ok(())
    }
}

struct AddBacklogCommand {
    title: String,
    category_id: String,
    duration_min: i32,
    deadline: Option<String>,
    inserted_id: Option<i64>,
}

impl AddBacklogCommand {
    fn new(
        title: String,
        category_id: String,
        duration_min: i32,
        deadline: Option<String>,
    ) -> Self {
        Self {
            title,
            category_id,
            duration_min,
            deadline,
            inserted_id: None,
        }
    }
}

impl Command for AddBacklogCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let id = crate::db::insert_backlog_task(
            conn,
            &self.title,
            &self.category_id,
            self.duration_min,
            self.deadline.as_deref(),
        )?;
        self.inserted_id = Some(id);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        if let Some(id) = self.inserted_id {
            crate::db::delete_task(conn, id)?;
        }
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.execute(conn)
    }
}

struct UpdateBacklogCommand {
    task_id: i64,
    title: String,
    category_id: String,
    duration_min: i32,
    deadline: Option<String>,
    before: Option<Task>,
    after: Option<Task>,
}

impl UpdateBacklogCommand {
    fn new(
        task_id: i64,
        title: String,
        category_id: String,
        duration_min: i32,
        deadline: Option<String>,
    ) -> Self {
        Self {
            task_id,
            title,
            category_id,
            duration_min,
            deadline,
            before: None,
            after: None,
        }
    }
}

impl Command for UpdateBacklogCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let before = crate::db::load_task_by_id(conn, self.task_id)?
            .context("編集対象バックログが見つかりません")?;
        crate::db::update_task_with_deadline(
            conn,
            self.task_id,
            &self.title,
            &self.category_id,
            self.duration_min,
            None,
            self.deadline.as_deref(),
        )?;
        let after = crate::db::load_task_by_id(conn, self.task_id)?
            .context("編集後のバックログ状態を取得できませんでした")?;
        self.before = Some(before);
        self.after = Some(after);
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let before = self.before.as_ref().context("Undo対象がありません")?;
        crate::db::update_task_with_deadline(
            conn,
            before.id,
            &before.title,
            &before.category_id,
            before.duration_min,
            None,
            before.deadline.as_deref(),
        )?;
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let after = self.after.as_ref().context("Redo対象がありません")?;
        crate::db::update_task_with_deadline(
            conn,
            after.id,
            &after.title,
            &after.category_id,
            after.duration_min,
            None,
            after.deadline.as_deref(),
        )?;
        Ok(())
    }
}

struct DeleteBacklogCommand {
    task_id: i64,
    backup: Option<(String, String, i32, Option<String>)>,
    restored_id: Option<i64>,
}

impl DeleteBacklogCommand {
    fn new(task_id: i64) -> Self {
        Self {
            task_id,
            backup: None,
            restored_id: None,
        }
    }
}

impl Command for DeleteBacklogCommand {
    fn execute(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let current_id = self.restored_id.unwrap_or(self.task_id);
        let Some(task) = crate::db::load_task_by_id(conn, current_id)? else {
            return Ok(());
        };
        if self.backup.is_none() {
            self.backup = Some((
                task.title.clone(),
                task.category_id.clone(),
                task.duration_min,
                task.deadline.clone(),
            ));
        }
        crate::db::delete_task(conn, current_id)?;
        self.restored_id = None;
        Ok(())
    }

    fn undo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let (title, category_id, duration_min, deadline) =
            self.backup.as_ref().context("Undo対象がありません")?;
        let id = crate::db::insert_backlog_task(
            conn,
            title,
            category_id,
            *duration_min,
            deadline.as_deref(),
        )?;
        self.restored_id = Some(id);
        Ok(())
    }

    fn redo(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.execute(conn)
    }
}

fn parse_absolute_deadline(raw: &str) -> anyhow::Result<String> {
    if raw.len() != 4 && raw.len() != 6 {
        bail!("期限は MMDD または MMDDhh で入力してください");
    }
    if !raw.chars().all(|c| c.is_ascii_digit()) {
        bail!("期限は数字で入力してください");
    }

    let month: u32 = raw[0..2].parse().context("月が不正です")?;
    let day: u32 = raw[2..4].parse().context("日が不正です")?;
    let year = Local::now().year();
    let date = NaiveDate::from_ymd_opt(year, month, day).context("日付が不正です")?;

    if raw.len() == 4 {
        return Ok(date.format("%Y-%m-%d").to_string());
    }

    let hour: u32 = raw[4..6].parse().context("時刻が不正です")?;
    if hour > 23 {
        bail!("時刻は00-23で入力してください");
    }
    Ok(format!("{} {:02}:00", date.format("%Y-%m-%d"), hour))
}

fn parse_deadline_date_time(deadline: &str) -> Option<(NaiveDate, Option<String>)> {
    if let Some((date_part, time_part)) = deadline.split_once(' ') {
        let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
        return Some((date, Some(time_part.to_string())));
    }
    let date = NaiveDate::parse_from_str(deadline, "%Y-%m-%d").ok()?;
    Some((date, None))
}

pub(crate) fn current_minutes() -> i32 {
    let now = Local::now();
    now.hour() as i32 * 60 + now.minute() as i32
}

fn normalize_schedule_fixed_start(fixed_start: i32, current_min: i32) -> i32 {
    if fixed_start >= current_min {
        return fixed_start;
    }

    let shift_days = (current_min - fixed_start + DAY_MINUTES - 1) / DAY_MINUTES;
    fixed_start + shift_days * DAY_MINUTES
}

fn normalize_fixed_start_time(minutes: i32) -> i32 {
    let normalized = (minutes % DAY_MINUTES + DAY_MINUTES) % DAY_MINUTES;
    normalized - normalized % 15
}

fn round_up_to_next_15_minutes(minutes: i32) -> i32 {
    let normalized = (minutes % DAY_MINUTES + DAY_MINUTES) % DAY_MINUTES;
    let remainder = normalized % 15;
    if remainder == 0 {
        normalized
    } else {
        (normalized + (15 - remainder)) % DAY_MINUTES
    }
}
