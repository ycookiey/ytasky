use chrono::{Datelike, Local, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::{
    App, BacklogTab, DeleteChoice, FormField, FormMode, FormTarget, InputMode, PanelFocus,
    RecurrenceFormField, RecurrenceFormState, TaskFormState, ViewMode, current_minutes,
};
use crate::model::{Recurrence, Task, format_deadline, format_duration, format_time};

const DAY_MINUTES: i32 = 24 * 60;

/// カテゴリIDからカラーを返す
fn category_color(cat_id: &str) -> Color {
    match cat_id {
        "sleep" => Color::Rgb(100, 120, 140),
        "meal" => Color::Rgb(200, 180, 50),
        "work" => Color::Rgb(200, 100, 140),
        "study" => Color::Rgb(140, 100, 200),
        "exercise" => Color::Rgb(80, 200, 80),
        "personal" => Color::Rgb(200, 140, 60),
        "break" => Color::Rgb(80, 180, 180),
        "commute" => Color::Rgb(180, 70, 70),
        "errand" => Color::Rgb(60, 180, 150),
        _ => Color::Gray,
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // タイトルバー
        Constraint::Min(0),    // メインエリア
        Constraint::Length(1), // キーバインドバー
    ])
    .split(f.area());

    draw_title_bar(f, chunks[0], app);

    match app.view_mode {
        ViewMode::TableView => {
            let main_chunks =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(30)]).split(chunks[1]);

            draw_multi_day_columns(f, main_chunks[0], app);
            draw_backlog_panel(f, main_chunks[1], app);
        }
        ViewMode::TimelineView => {
            draw_timeline_view(f, chunks[1], app);
        }
        ViewMode::ReportView => {
            draw_report_view(f, chunks[1], app);
        }
    }

    draw_keybindings_bar(f, chunks[2], app);
    draw_modal_if_needed(f, app);
}

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let remaining = app.remaining_minutes();
    let rem_str = format_duration(remaining);
    let now_str = Local::now().format("%H:%M").to_string();
    let today_marker = if app.is_today() { " *" } else { "" };
    let cursor_date = app.cursor_date().to_string();
    let date_label = match app.view_mode {
        ViewMode::ReportView => format!("Report: {cursor_date}"),
        _ => NaiveDate::parse_from_str(&cursor_date, "%Y-%m-%d")
            .map(|date| format!("{cursor_date} ({})", date.format("%a")))
            .unwrap_or(cursor_date),
    };
    let mut title = format!(
        "  {}{}  [{}]  [{}d]    余り {} / 24h    [u:{} r:{}]",
        date_label,
        today_marker,
        now_str,
        app.view_days,
        rem_str,
        app.undo_count(),
        app.redo_count()
    );

    if let Some(msg) = &app.status_message {
        title.push_str(&format!("    | {msg}"));
    }

    let bar =
        Paragraph::new(title).style(Style::default().bg(Color::Rgb(40, 40, 80)).fg(Color::White));
    f.render_widget(bar, area);
}

fn draw_keybindings_bar(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<(&str, &str)> = match &app.input_mode {
        InputMode::TaskForm(_) => {
            vec![("Enter", "次/保存"), ("Tab", "移動"), ("Esc", "キャンセル")]
        }
        InputMode::ConfirmDelete { .. } | InputMode::ConfirmDeleteRecurrence { .. } => {
            vec![("Enter", "削除"), ("Esc", "キャンセル")]
        }
        InputMode::RecurrenceForm(_) => {
            vec![("j/k", "選択"), ("h/l", "曜日"), ("Space", "切替"), ("Tab", "終了日"), ("Enter", "保存"), ("Esc", "キャンセル")]
        }
        InputMode::BacklogSelect { .. } => {
            vec![("j/k", "移動"), ("Enter", "挿入"), ("Esc", "キャンセル")]
        }
        InputMode::Normal => match app.view_mode {
            ViewMode::ReportView => vec![("q/Esc", "閉じる")],
            ViewMode::TimelineView => vec![
                ("j/k", "移動"),
                ("h/l", "日"),
                ("a", "追加"),
                ("e", "編集"),
                ("d", "削除"),
                ("J/K", "並替"),
                ("Space", "実績"),
                ("t", "Table"),
                ("r", "Report"),
                ("1-9", "日数"),
                ("q", "終了"),
            ],
            ViewMode::TableView => match app.focus {
                PanelFocus::Table => vec![
                    ("j/k", "移動"),
                    ("h/l", "日"),
                    ("a", "追加"),
                    ("e", "編集"),
                    ("d", "削除"),
                    ("J/K", "並替"),
                    ("Space", "実績"),
                    ("Tab", "BL"),
                    ("B", "→BL"),
                    ("p", "←BL"),
                    ("t", "TL"),
                    ("r", "Report"),
                    ("1-9", "日数"),
                    ("u/^r", "undo/redo"),
                    ("q", "終了"),
                ],
                PanelFocus::Backlog => vec![
                    ("j/k", "移動"),
                    ("Enter", "挿入"),
                    ("a", "追加"),
                    ("e", "編集"),
                    ("d", "削除"),
                    ("Tab", "Table"),
                    ("q", "終了"),
                ],
            },
        },
    };

    let key_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (idx, (key, desc)) in items.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, key_style));
        spans.push(Span::styled(format!(":{desc}"), desc_style));
    }

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(24, 24, 24)));
    f.render_widget(bar, area);
}

fn draw_multi_day_columns(f: &mut Frame, area: Rect, app: &App) {
    if app.view_days == 0 {
        return;
    }

    let constraints: Vec<Constraint> = (0..app.view_days)
        .map(|_| Constraint::Ratio(1, app.view_days as u32))
        .collect();
    let columns = Layout::horizontal(constraints).split(area);

    for (col_idx, (date, tasks)) in app.day_tasks.iter().enumerate() {
        if col_idx >= columns.len() {
            break;
        }
        let is_active = col_idx == app.col_cursor && app.focus == PanelFocus::Table;
        let cursor_row = if is_active {
            Some(app.row_cursor)
        } else {
            None
        };
        draw_day_column(f, columns[col_idx], app, date, tasks, is_active, cursor_row);
    }
}

fn draw_day_column(
    f: &mut Frame,
    area: Rect,
    app: &App,
    date: &str,
    tasks: &[Task],
    is_active: bool,
    cursor_row: Option<usize>,
) {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("時刻"),
        Cell::from("時間"),
        Cell::from("タスク"),
        Cell::from("カテゴリ"),
        Cell::from("状態"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let schedule = build_schedule(tasks);
    let buffers = build_buffers(&schedule);
    let mut rows: Vec<Row> = Vec::new();

    let today = Local::now().format("%Y-%m-%d").to_string();
    let is_today = date == today;
    let compact = area.width < 42;

    let mut buffer_idx = 0usize;
    for (i, (start, _, task)) in schedule.iter().enumerate() {
        if let Some((buffer_start, buffer_end)) = buffers.get(buffer_idx)
            && *buffer_end == *start
        {
            let buffer_minutes = buffer_end - buffer_start;
            rows.push(
                Row::new(vec![
                    Cell::from(""),
                    Cell::from(format_time(*buffer_start)),
                    Cell::from(format_duration(buffer_minutes)),
                    Cell::from(format!("-- buffer {buffer_minutes}m --")),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .style(Style::default().fg(Color::Rgb(120, 120, 150))),
            );
            buffer_idx += 1;
        }

        let status = if task.actual_end.is_some() {
            "done".to_string()
        } else if let Some(actual_start) = task.actual_start {
            if is_today {
                let elapsed = (current_minutes() - actual_start).max(0);
                format!("▶{}", format_duration(elapsed))
            } else {
                "▶now".to_string()
            }
        } else {
            "--".to_string()
        };

        let color = category_color(&task.category_id);
        let style = if Some(i) == cursor_row {
            Style::default().bg(Color::Rgb(50, 50, 80)).fg(Color::White)
        } else {
            Style::default().fg(color)
        };
        let category_text = if compact {
            compact_category_name(app, &task.category_id)
        } else {
            category_name(app, &task.category_id)
        };
        let title_text = if task.recurrence_id.is_some() {
            format!("↻ {}", task.title)
        } else {
            task.title.clone()
        };

        rows.push(
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(format_time(*start)),
                Cell::from(format_duration(task.duration_min)),
                Cell::from(title_text),
                Cell::from(category_text),
                Cell::from(status),
            ])
            .style(style),
        );
    }

    if rows.is_empty() {
        rows.push(
            Row::new(vec![
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
                Cell::from("タスクなし"),
                Cell::from(""),
                Cell::from(""),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        );
    }

    if let Some((_, end, _)) = schedule.last() {
        rows.push(
            Row::new(vec![
                Cell::from(""),
                Cell::from(format_time(*end)),
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        );
    }

    let widths = if compact {
        [
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(5),
        ]
    } else {
        [
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(10),
            Constraint::Length(14),
            Constraint::Length(7),
        ]
    };

    let title_style = if is_today {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            format!(" {} ", format_column_date(date)),
            title_style,
        )));
    if is_active && app.backlog_select_cursor().is_none() {
        block = block.border_style(Style::default().fg(Color::Yellow));
    }

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn format_column_date(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| format!("{:02}/{:02} ({})", d.month(), d.day(), d.format("%a")))
        .unwrap_or_else(|_| date.to_string())
}

fn compact_category_name(app: &App, cat_id: &str) -> String {
    app.categories
        .iter()
        .find(|cat| cat.id == cat_id)
        .map(|cat| cat.icon.clone())
        .unwrap_or_else(|| cat_id.chars().take(3).collect())
}

fn draw_backlog_panel(f: &mut Frame, area: Rect, app: &App) {
    let select_cursor = app.backlog_select_cursor();
    let focused = app.focus == PanelFocus::Backlog || select_cursor.is_some();

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(backlog_tab_title(app));
    if focused {
        block = block.border_style(Style::default().fg(Color::Yellow));
    }

    match app.backlog_tab {
        BacklogTab::Backlog => {
            let cursor = select_cursor.unwrap_or(app.backlog_cursor);
            let today = Local::now().format("%Y-%m-%d").to_string();
            let mut rows: Vec<Row> = Vec::new();

            if app.backlog_tasks.is_empty() {
                rows.push(
                    Row::new(vec![
                        Cell::from("バックログなし"),
                        Cell::from("---").style(Style::default().fg(Color::DarkGray)),
                    ])
                    .style(Style::default().fg(Color::DarkGray)),
                );
            } else {
                for (idx, task) in app.backlog_tasks.iter().enumerate() {
                    let (deadline_text, deadline_color) =
                        backlog_deadline_style(task.deadline.as_deref(), &today);
                    let row_style = if idx == cursor {
                        Style::default().bg(Color::Rgb(50, 50, 80)).fg(Color::White)
                    } else {
                        Style::default()
                    };
                    rows.push(
                        Row::new(vec![
                            Cell::from(task.title.clone()),
                            Cell::from(deadline_text).style(Style::default().fg(deadline_color)),
                        ])
                        .style(row_style),
                    );
                }
            }

            let table = Table::new(rows, [Constraint::Min(1), Constraint::Length(8)]).block(block);
            f.render_widget(table, area);
        }
        BacklogTab::Recurrences => {
            let mut rows: Vec<Row> = Vec::new();

            if app.recurrences.is_empty() {
                rows.push(
                    Row::new(vec![
                        Cell::from("繰り返しなし"),
                        Cell::from("---"),
                        Cell::from("---"),
                    ])
                    .style(Style::default().fg(Color::DarkGray)),
                );
            } else {
                for (idx, recurrence) in app.recurrences.iter().enumerate() {
                    let row_style = if idx == app.recurrence_cursor {
                        Style::default().bg(Color::Rgb(50, 50, 80)).fg(Color::White)
                    } else {
                        Style::default()
                    };
                    rows.push(
                        Row::new(vec![
                            Cell::from(recurrence.title.clone()),
                            Cell::from(recurrence_pattern_label(recurrence)),
                            Cell::from(recurrence_days_summary(recurrence)),
                        ])
                        .style(row_style),
                    );
                }
            }

            let table = Table::new(
                rows,
                [
                    Constraint::Min(8),
                    Constraint::Length(8),
                    Constraint::Length(10),
                ],
            )
            .block(block);
            f.render_widget(table, area);
        }
    }
}

fn draw_timeline_view(f: &mut Frame, area: Rect, app: &App) {
    if app.view_days == 0 {
        return;
    }

    let constraints: Vec<Constraint> = (0..app.view_days)
        .map(|_| Constraint::Ratio(1, app.view_days as u32))
        .collect();
    let columns = Layout::horizontal(constraints).split(area);

    for (col_idx, (date, tasks)) in app.day_tasks.iter().enumerate() {
        if col_idx >= columns.len() {
            break;
        }
        let is_active = col_idx == app.col_cursor;
        let cursor_row = if is_active {
            Some(app.row_cursor)
        } else {
            None
        };
        draw_timeline_column(f, columns[col_idx], app, date, tasks, is_active, cursor_row);
    }
}

fn draw_timeline_column(
    f: &mut Frame,
    area: Rect,
    _app: &App,
    date: &str,
    tasks: &[Task],
    is_active: bool,
    cursor_row: Option<usize>,
) {
    let schedule = build_schedule(tasks);
    let buffers = build_buffers(&schedule);

    let today = Local::now().format("%Y-%m-%d").to_string();
    let is_today = date == today;
    let title_style = if is_today {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let mut block_widget = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            format!(" {} ", format_column_date(date)),
            title_style,
        )));
    if is_active {
        block_widget = block_widget.border_style(Style::default().fg(Color::Yellow));
    }

    let inner = block_widget.inner(area);
    let compact = inner.width < 30;
    let label_width = if compact { 6 } else { 7 };
    let bar_width = inner.width.saturating_sub(label_width) as usize;

    let max_end = schedule
        .iter()
        .map(|(_, end, _)| *end)
        .max()
        .unwrap_or(DAY_MINUTES)
        .max(DAY_MINUTES);
    let total_rows = ((max_end + 29) / 30).max(48) as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(total_rows);
    let mut cursor_start_row: Option<usize> = None;
    let now_min = is_today.then(current_minutes);

    for row_idx in 0..total_rows {
        let slot_start = (row_idx as i32) * 30;
        let slot_end = slot_start + 30;

        if let Some(now_min) = now_min
            && slot_start <= now_min
            && now_min < slot_end
        {
            let now_text = format!("─── {} ───", format_time(now_min));
            let padded = format!("{:^width$}", now_text, width = bar_width);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:width$}", "", width = label_width as usize),
                    Style::default().fg(Color::Red),
                ),
                Span::styled(
                    padded,
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        let label = if slot_start % 60 == 0 {
            if compact {
                format!(" {:>5}", format_time(slot_start))
            } else {
                format!(" {:>5} ", format_time(slot_start))
            }
        } else {
            " ".repeat(label_width as usize)
        };

        // バッファ区間
        if let Some((buffer_start, buffer_end)) = buffers
            .iter()
            .find(|(start, end)| *start < slot_end && *end > slot_start)
        {
            let is_start = slot_start <= *buffer_start && *buffer_start < slot_end;
            let text = if is_start {
                format!("  buffer {}m", buffer_end - buffer_start)
            } else {
                String::new()
            };
            let padded = format!("{:<width$}", text, width = bar_width);

            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    padded,
                    Style::default()
                        .bg(Color::Rgb(40, 40, 50))
                        .fg(Color::Rgb(170, 170, 190)),
                ),
            ]));
            continue;
        }

        // 通常スケジュール
        if let Some((idx, (task_start, _, task))) = schedule
            .iter()
            .enumerate()
            .find(|(_, (start, end, _))| *start < slot_end && *end > slot_start)
        {
            let is_start = slot_start <= *task_start && *task_start < slot_end;
            let is_selected = cursor_row.is_some_and(|row| idx == row);

            if is_selected && cursor_start_row.is_none() {
                cursor_start_row = Some(lines.len());
            }

            let base_color = category_color(&task.category_id);
            let (bg, fg) = if task.actual_end.is_some() {
                (muted_color(base_color), Color::Rgb(180, 180, 180))
            } else {
                (base_color, Color::White)
            };

            let bg = if is_selected { brighten_color(bg) } else { bg };

            let marker = if is_selected && is_start { "▸" } else { " " };
            let text = if is_start {
                let task_title = if task.recurrence_id.is_some() {
                    format!("↻ {}", task.title)
                } else {
                    task.title.clone()
                };
                format!("{marker} {task_title}")
            } else {
                String::new()
            };
            let padded = format!("{:<width$}", text, width = bar_width);

            let mut style = Style::default().bg(bg).fg(fg);
            if task.actual_start.is_some() && task.actual_end.is_none() {
                style = style.add_modifier(Modifier::BOLD);
            }

            let label_style = if is_selected && is_start {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(padded, style),
            ]));
        } else {
            // 空きスロット
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<width$}", "·", width = bar_width),
                    Style::default().fg(Color::Rgb(50, 50, 50)),
                ),
            ]));
        }
    }

    let visible_rows = inner.height as usize;
    let scroll_y = if cursor_row.is_some() {
        if let Some(cr) = cursor_start_row {
            if cr < visible_rows / 3 {
                0
            } else {
                (cr - visible_rows / 3) as u16
            }
        } else {
            0
        }
    } else {
        0
    };

    let timeline = Paragraph::new(lines)
        .block(block_widget)
        .scroll((scroll_y, 0));

    f.render_widget(timeline, area);
}

fn draw_report_view(f: &mut Frame, area: Rect, app: &App) {
    let chunks =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let category_header = Row::new(vec![
        Cell::from("カテゴリ"),
        Cell::from("予定"),
        Cell::from("実績"),
        Cell::from("差分"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let mut category_rows: Vec<Row> = app
        .category_reports
        .iter()
        .map(|report| {
            let diff = report.actual_min - report.planned_min;
            let diff_style = if diff > 0 {
                Style::default().fg(Color::Red)
            } else if diff < 0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };

            Row::new(vec![
                Cell::from(report.category_name.clone())
                    .style(Style::default().fg(category_color(&report.category_id))),
                Cell::from(format_duration(report.planned_min)),
                Cell::from(format_duration(report.actual_min)),
                Cell::from(format_signed_duration(diff)).style(diff_style),
            ])
        })
        .collect();

    if category_rows.is_empty() {
        category_rows.push(Row::new(vec![
            Cell::from("データなし"),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("-"),
        ]));
    }

    let category_table = Table::new(
        category_rows,
        [
            Constraint::Min(12),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(category_header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Category Report "),
    );
    f.render_widget(category_table, chunks[0]);

    let title_header = Row::new(vec![
        Cell::from("タスク名"),
        Cell::from("カテゴリ"),
        Cell::from("予定"),
        Cell::from("実績"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let mut title_rows: Vec<Row> = app
        .title_reports
        .iter()
        .map(|report| {
            Row::new(vec![
                Cell::from(report.title.clone()),
                Cell::from(category_name(app, &report.category_id))
                    .style(Style::default().fg(category_color(&report.category_id))),
                Cell::from(format_duration(report.planned_min)),
                Cell::from(format_duration(report.actual_min)),
            ])
        })
        .collect();

    if title_rows.is_empty() {
        title_rows.push(Row::new(vec![
            Cell::from("データなし"),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("-"),
        ]));
    }

    let title_table = Table::new(
        title_rows,
        [
            Constraint::Min(16),
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(title_header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Title Report "),
    );
    f.render_widget(title_table, chunks[1]);
}

fn build_schedule(tasks: &[Task]) -> Vec<(i32, i32, &Task)> {
    let mut schedule: Vec<(i32, i32, &Task)> = Vec::with_capacity(tasks.len());
    let mut current_min = 0;

    for task in tasks {
        let start = match task.fixed_start {
            Some(fixed_start) => normalize_fixed_start(fixed_start, current_min),
            None => current_min,
        };
        let end = start + task.duration_min;
        schedule.push((start, end, task));
        current_min = end;
    }

    schedule
}

fn build_buffers(schedule: &[(i32, i32, &Task)]) -> Vec<(i32, i32)> {
    let mut buffers = Vec::new();
    let mut previous_end = 0;

    for (start, end, _) in schedule {
        if previous_end < *start {
            buffers.push((previous_end, *start));
        }
        previous_end = *end;
    }

    buffers
}

fn normalize_fixed_start(fixed_start: i32, current_min: i32) -> i32 {
    if fixed_start >= current_min {
        return fixed_start;
    }

    let shift_days = (current_min - fixed_start + DAY_MINUTES - 1) / DAY_MINUTES;
    fixed_start + shift_days * DAY_MINUTES
}

fn format_signed_duration(minutes: i32) -> String {
    if minutes > 0 {
        format!("+{}", format_duration(minutes))
    } else if minutes < 0 {
        format!("-{}", format_duration(-minutes))
    } else {
        format_duration(0)
    }
}

fn backlog_deadline_style(deadline: Option<&str>, today: &str) -> (String, Color) {
    let Some(deadline) = deadline.filter(|d| !d.trim().is_empty()) else {
        return ("---".to_string(), Color::DarkGray);
    };

    let date_part = deadline.split_once(' ').map(|(d, _)| d).unwrap_or(deadline);
    let Some(today_date) = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok() else {
        return (format_deadline(deadline, today), Color::DarkGray);
    };
    let Some(deadline_date) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok() else {
        return (deadline.to_string(), Color::DarkGray);
    };

    let diff = (deadline_date - today_date).num_days();
    if diff < 0 {
        return (format!("!{}d", -diff), Color::Red);
    }
    if diff == 0 {
        return ("today".to_string(), Color::Yellow);
    }
    if diff <= 7 {
        return (format!("{diff}d"), Color::Yellow);
    }
    (
        format!("{}/{}", deadline_date.month(), deadline_date.day()),
        Color::DarkGray,
    )
}

fn backlog_tab_title(app: &App) -> Line<'static> {
    let backlog_style = if app.backlog_tab == BacklogTab::Backlog {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let recur_style = if app.backlog_tab == BacklogTab::Recurrences {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };

    Line::from(vec![
        Span::styled(
            format!("[Backlog ({})]", app.backlog_tasks.len()),
            backlog_style,
        ),
        Span::raw(" "),
        Span::styled(format!("[Recur ({})]", app.recurrences.len()), recur_style),
    ])
}

fn recurrence_pattern_label(recurrence: &Recurrence) -> String {
    match recurrence.pattern.as_str() {
        "daily" => "daily".to_string(),
        "weekly" => "weekly".to_string(),
        "monthly" => "monthly".to_string(),
        _ => recurrence.pattern.clone(),
    }
}

fn recurrence_days_summary(recurrence: &Recurrence) -> String {
    let days = recurrence
        .pattern_data
        .as_deref()
        .and_then(|raw| serde_json::from_str::<crate::model::PatternData>(raw).ok())
        .and_then(|parsed| parsed.days)
        .unwrap_or_default();

    match recurrence.pattern.as_str() {
        "daily" => "毎日".to_string(),
        "weekly" => {
            let labels = ["月", "火", "水", "木", "金", "土", "日"];
            let mut out = String::new();
            for day in days {
                if (1..=7).contains(&day) {
                    out.push_str(labels[(day - 1) as usize]);
                }
            }
            if out.is_empty() {
                "---".to_string()
            } else {
                out
            }
        }
        "monthly" => {
            if days.is_empty() {
                return "---".to_string();
            }
            let mut sorted = days;
            sorted.sort_unstable();
            sorted
                .iter()
                .map(|day| day.to_string())
                .collect::<Vec<_>>()
                .join(",")
                + "日"
        }
        _ => "---".to_string(),
    }
}

fn draw_modal_if_needed(f: &mut Frame, app: &App) {
    match &app.input_mode {
        InputMode::Normal => {}
        InputMode::TaskForm(form) => draw_task_form_modal(f, app, form),
        InputMode::ConfirmDelete { title, .. } => draw_delete_modal(f, app, title),
        InputMode::ConfirmDeleteRecurrence { title, choice, .. } => {
            draw_delete_recurrence_modal(f, app, title, choice)
        }
        InputMode::BacklogSelect { .. } => {}
        InputMode::RecurrenceForm(form) => draw_recurrence_form_modal(f, app, form),
    }
}

fn draw_task_form_modal(f: &mut Frame, app: &App, form: &TaskFormState) {
    let area = centered_rect(64, 72, f.area());
    f.render_widget(Clear, area);

    let title = match (form.target, form.mode) {
        (FormTarget::Backlog, FormMode::Add) => " Add Backlog ",
        (FormTarget::Backlog, FormMode::Edit) => " Edit Backlog ",
        (_, FormMode::Add) => " Add Task ",
        (_, FormMode::Edit) => " Edit Task ",
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    f.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let active_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(Color::White);
    let muted_style = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "Category (j/k to select):",
        muted_style,
    ))];

    for (idx, cat) in app.categories.iter().enumerate() {
        let marker = if idx == form.category_idx { ">" } else { " " };
        let style = if form.field == FormField::Category && idx == form.category_idx {
            active_style
        } else {
            normal_style
        };

        lines.push(Line::from(Span::styled(
            format!(" {marker} {} {} ({})", cat.icon, cat.name, cat.id),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Duration(min): ", muted_style),
        Span::styled(
            form.duration_input.as_str(),
            if form.field == FormField::Duration {
                active_style
            } else {
                normal_style
            },
        ),
        Span::styled("  (15min unit)", muted_style),
    ]));

    if form.target == FormTarget::Schedule {
        let fixed_start = match form.fixed_start {
            Some(minutes) => format_time(minutes),
            None => "なし".to_string(),
        };

        let mut fixed_start_line = vec![
            Span::styled("Fixed Start: ", muted_style),
            Span::styled(
                fixed_start,
                if form.field == FormField::FixedStart {
                    active_style
                } else {
                    normal_style
                },
            ),
        ];
        if form.field == FormField::FixedStart {
            fixed_start_line.push(Span::styled(
                "  (j/k:±15m h/l:±1h H/L:±6h n:なし)",
                muted_style,
            ));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(fixed_start_line));
    }

    lines.push(Line::from(""));
    let deadline_display = if !form.deadline_input.is_empty() {
        form.deadline_input.clone()
    } else {
        form.deadline.clone().unwrap_or_else(|| "---".to_string())
    };
    let mut deadline_line = vec![
        Span::styled("Deadline: ", muted_style),
        Span::styled(
            deadline_display,
            if form.field == FormField::Deadline {
                active_style
            } else {
                normal_style
            },
        ),
    ];
    if form.field == FormField::Deadline {
        deadline_line.push(Span::styled(
            "  (MMDD/MMDDhh, Nd/Nw/Nm, j/k/h/l/H/L, n)",
            muted_style,
        ));
    }
    lines.push(Line::from(deadline_line));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Title: ", muted_style),
        Span::styled(
            if form.title.is_empty() {
                "(empty)"
            } else {
                form.title.as_str()
            },
            if form.field == FormField::Title {
                active_style
            } else {
                normal_style
            },
        ),
    ]));

    lines.push(Line::from(""));
    let tab_help = if form.target == FormTarget::Schedule {
        "Category->Duration->FixedStart->Deadline->Title"
    } else {
        "Category->Duration->Deadline->Title"
    };
    lines.push(Line::from(Span::styled(
        format!("Enter: Next/Save  Tab: {tab_help}  Esc: Cancel"),
        muted_style,
    )));

    if let Some(msg) = &app.status_message {
        lines.push(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_delete_modal(f: &mut Frame, app: &App, title: &str) {
    let area = centered_rect(52, 28, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Delete Task ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    f.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let mut lines = vec![
        Line::from(format!("\"{title}\" を削除しますか？")),
        Line::from(""),
        Line::from(Span::styled(
            "Enter: Delete  Esc: Cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    if let Some(msg) = &app.status_message {
        lines.push(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_recurrence_form_modal(f: &mut Frame, app: &App, form: &RecurrenceFormState) {
    let area = centered_rect(64, 54, f.area());
    f.render_widget(Clear, area);

    let task_title = find_task_title(app, form.task_id).unwrap_or("(unknown task)");
    let title_suffix = if form.editing_recurrence_id.is_some() {
        "編集"
    } else {
        "設定"
    };
    let block = Block::default()
        .title(format!(" 繰り返し{title_suffix}: {task_title} "))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    f.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let active = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);
    let muted = Style::default().fg(Color::DarkGray);
    let selected_day = Style::default().fg(Color::Cyan);
    let cursor_day = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::UNDERLINED);

    let in_pattern = form.field == RecurrenceFormField::Pattern;
    let pattern_labels = ["毎日", "毎週", "毎月"];
    let day_labels = ["月", "火", "水", "木", "金", "土", "日"];

    let mut lines: Vec<Line> = Vec::new();

    for (idx, label) in pattern_labels.iter().enumerate() {
        let is_current = form.pattern_idx == idx;
        let arrow = if is_current { "> " } else { "  " };
        let label_style = if in_pattern && is_current {
            active
        } else if is_current {
            selected_day
        } else {
            normal
        };

        let mut spans = vec![
            Span::styled(arrow, if in_pattern && is_current { active } else { normal }),
            Span::styled(*label, label_style),
        ];

        // weekly行: 曜日ボタン（weeklyパターン時のみ選択表示）
        if idx == 1 {
            spans.push(Span::raw("    "));
            let show_selection = form.pattern == "weekly";
            for (di, dl) in day_labels.iter().enumerate() {
                let day_num = (di + 1) as u8;
                let is_selected = show_selection && form.weekly_days.contains(&day_num);
                let is_cursor = in_pattern && is_current && form.day_cursor == di;
                let style = if is_cursor && is_selected {
                    cursor_day
                } else if is_cursor {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::UNDERLINED)
                } else if is_selected {
                    selected_day
                } else {
                    muted
                };
                spans.push(Span::styled(format!("[{dl}]"), style));
            }
        }

        // monthly行: 日付入力（monthlyパターン時のみ値表示）
        if idx == 2 {
            spans.push(Span::raw("    "));
            let day_text = if form.pattern != "monthly" || form.monthly_days.is_empty() {
                "--".to_string()
            } else {
                format!("{:>2}", form.monthly_days[0])
            };
            let day_style = if in_pattern && is_current {
                active
            } else {
                normal
            };
            spans.push(Span::styled(format!("[{day_text}]"), day_style));
            spans.push(Span::styled(" 日", normal));
        }

        lines.push(Line::from(spans));
    }

    // 終了日
    lines.push(Line::from(""));
    let end_display = if !form.end_date_input.is_empty() {
        form.end_date_input.clone()
    } else {
        form.end_date.clone().unwrap_or_else(|| "なし".to_string())
    };
    let end_style = if form.field == RecurrenceFormField::EndDate {
        active
    } else {
        normal
    };
    lines.push(Line::from(vec![
        Span::styled("  終了日  ", muted),
        Span::styled(format!("[{end_display:<14}]"), end_style),
    ]));

    // ヘルプ
    lines.push(Line::from(""));
    let help = if form.field == RecurrenceFormField::Pattern {
        match form.pattern.as_str() {
            "weekly" => "j/k:選択 h/l:曜日移動 Space:切替 Tab:終了日 Enter:保存",
            "monthly" => "j/k:選択 0-9:日入力 BS:削除 Tab:終了日 Enter:保存",
            _ => "j/k:選択 Tab:終了日 Enter:保存",
        }
    } else {
        "0-9/-:日付入力 j/k:±1日 h/l:±7日 n:なし Tab:戻る Enter:保存"
    };
    lines.push(Line::from(Span::styled(help, muted)));

    if let Some(msg) = &app.status_message {
        lines.push(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_delete_recurrence_modal(f: &mut Frame, app: &App, title: &str, choice: &DeleteChoice) {
    let area = centered_rect(58, 32, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" 繰り返しタスク削除: \"{title}\" "))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    f.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let this_day_style = if matches!(choice, DeleteChoice::ThisDayOnly) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let delete_rule_style = if matches!(choice, DeleteChoice::DeleteRule) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("[この日のみ]", this_day_style),
            Span::raw("  "),
            Span::styled("[ルールごと削除]", delete_rule_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "h/l: 選択  Enter: 確定  Esc: 取消",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    if let Some(msg) = &app.status_message {
        lines.push(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Red),
        )));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

fn find_task_title(app: &App, task_id: i64) -> Option<&str> {
    for (_, tasks) in &app.day_tasks {
        if let Some(task) = tasks.iter().find(|task| task.id == task_id) {
            return Some(task.title.as_str());
        }
    }
    app.backlog_tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(|task| task.title.as_str())
}

fn category_name(app: &App, cat_id: &str) -> String {
    app.categories
        .iter()
        .find(|cat| cat.id == cat_id)
        .map(|cat| format!("{} {}", cat.icon, cat.name))
        .unwrap_or_else(|| cat_id.to_string())
}

fn brighten_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as u16 + 50).min(255) as u8,
            (g as u16 + 50).min(255) as u8,
            (b as u16 + 50).min(255) as u8,
        ),
        _ => Color::White,
    }
}

fn muted_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let dark = (80u16, 80u16, 80u16);
            Color::Rgb(
                ((r as u16 + dark.0 * 2) / 3) as u8,
                ((g as u16 + dark.1 * 2) / 3) as u8,
                ((b as u16 + dark.2 * 2) / 3) as u8,
            )
        }
        _ => Color::DarkGray,
    }
}
