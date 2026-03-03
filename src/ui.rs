use chrono::{Datelike, Local, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::{
    App, FormField, FormMode, FormTarget, InputMode, PanelFocus, TaskFormState, ViewMode,
    current_minutes,
};
use crate::model::{Task, format_deadline, format_duration, format_time};

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
    ])
    .split(f.area());

    draw_title_bar(f, chunks[0], app);

    match app.view_mode {
        ViewMode::TableView => {
            let main_chunks = Layout::horizontal([
                Constraint::Length(24),
                Constraint::Min(0),
                Constraint::Length(30),
            ])
            .split(chunks[1]);

            draw_sidebar(f, main_chunks[0], app);
            draw_task_table(f, main_chunks[1], app);
            draw_backlog_panel(f, main_chunks[2], app);
        }
        ViewMode::TimelineView => {
            draw_timeline_view(f, chunks[1], app);
        }
        ViewMode::ReportView => {
            draw_report_view(f, chunks[1], app);
        }
    }

    draw_modal_if_needed(f, app);
}

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let remaining = app.remaining_minutes();
    let rem_str = format_duration(remaining);
    let now_str = Local::now().format("%H:%M").to_string();
    let date_label = match app.view_mode {
        ViewMode::ReportView => format!("Report: {}", app.date),
        _ => NaiveDate::parse_from_str(&app.date, "%Y-%m-%d")
            .map(|date| format!("{} ({})", app.date, date.format("%a")))
            .unwrap_or_else(|_| app.date.clone()),
    };
    let mut title = format!(
        "  {}  [{}]    余り {} / 24h    [u:{} r:{}]",
        date_label,
        now_str,
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

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();

    // 前日からのはみ出し
    for overflow in &app.overflow_tasks {
        let time_str = format_time(overflow.start_min);
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {time_str} "),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("░ {}", overflow.title),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    let schedule = build_schedule(app);
    let buffers = build_buffers(app, &schedule);
    let mut buffer_idx = 0usize;

    // タイムライン: タスクを時刻順に表示
    for (start, _, task) in &schedule {
        if let Some((buffer_start, buffer_end)) = buffers.get(buffer_idx)
            && *buffer_end == *start
        {
            let time_str = format_time(*buffer_start);
            let buffer_minutes = buffer_end - buffer_start;
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {time_str} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("▒ buffer {buffer_minutes}m"),
                    Style::default().fg(Color::Rgb(120, 120, 150)),
                ),
            ]));
            buffer_idx += 1;
        }

        let time_str = format_time(*start);
        let color = category_color(&task.category_id);

        // 簡易ブロック表示
        let block_char = if task.actual_end.is_some() {
            "▓"
        } else if task.actual_start.is_some() {
            "▒"
        } else {
            "░"
        };
        let title = if app.is_today() {
            if let Some(actual_start) = task.actual_start {
                if task.actual_end.is_none() {
                    let elapsed = (current_minutes() - actual_start).max(0);
                    format!("{} ({}m)", task.title, elapsed)
                } else {
                    task.title.clone()
                }
            } else {
                task.title.clone()
            }
        } else {
            task.title.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!(" {time_str} "),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("{block_char} {title}"), Style::default().fg(color)),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from("  タスクなし"));
    }

    let sidebar =
        Paragraph::new(lines).block(Block::default().borders(Borders::RIGHT).title(" Timeline "));
    f.render_widget(sidebar, area);
}

fn draw_task_table(f: &mut Frame, area: Rect, app: &App) {
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

    let schedule = build_schedule(app);
    let buffers = build_buffers(app, &schedule);

    let mut rows: Vec<Row> = Vec::new();

    // 前日からのはみ出しタスクを先頭に表示
    for overflow in &app.overflow_tasks {
        rows.push(
            Row::new(vec![
                Cell::from(""),
                Cell::from(format_time(overflow.start_min)),
                Cell::from(format_duration(overflow.end_min - overflow.start_min)),
                Cell::from(overflow.title.clone()),
                Cell::from(category_name(app, &overflow.category_id)),
                Cell::from("(前日)"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        );
    }

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
            if app.is_today() {
                let elapsed = (current_minutes() - actual_start).max(0);
                format!("▶{}", format_duration(elapsed))
            } else {
                "▶now".to_string()
            }
        } else {
            "--".to_string()
        };

        let color = category_color(&task.category_id);
        let style = if i == app.cursor {
            Style::default().bg(Color::Rgb(50, 50, 80)).fg(Color::White)
        } else {
            Style::default().fg(color)
        };

        rows.push(
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(format_time(*start)),
                Cell::from(format_duration(task.duration_min)),
                Cell::from(task.title.clone()),
                Cell::from(category_name(app, &task.category_id)),
                Cell::from(status),
            ])
            .style(style),
        );
    }

    let widths = [
        Constraint::Length(3),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Min(10),
        Constraint::Length(14),
        Constraint::Length(7),
    ];

    // 最後のタスクの下に終了時刻を表示
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

    let mut block = Block::default().borders(Borders::ALL).title(" Tasks ");
    if app.view_mode == ViewMode::TableView
        && app.focus == PanelFocus::Table
        && app.backlog_select_cursor().is_none()
    {
        block = block.border_style(Style::default().fg(Color::Yellow));
    }

    let table = Table::new(rows, widths).header(header).block(block);

    f.render_widget(table, area);
}

fn draw_backlog_panel(f: &mut Frame, area: Rect, app: &App) {
    let select_cursor = app.backlog_select_cursor();
    let cursor = select_cursor.unwrap_or(app.backlog_cursor);
    let focused = app.focus == PanelFocus::Backlog || select_cursor.is_some();

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

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Backlog ({}) ", app.backlog_tasks.len()));
    if focused {
        block = block.border_style(Style::default().fg(Color::Yellow));
    }

    let table = Table::new(rows, [Constraint::Min(1), Constraint::Length(8)]).block(block);
    f.render_widget(table, area);
}

fn draw_timeline_view(f: &mut Frame, area: Rect, app: &App) {
    // レイアウト: タイムライン | サイドバー
    let chunks = Layout::horizontal([Constraint::Min(0), Constraint::Length(24)]).split(area);

    draw_timeline_main(f, chunks[0], app);
    draw_timeline_sidebar(f, chunks[1], app);
}

fn draw_timeline_main(f: &mut Frame, area: Rect, app: &App) {
    let schedule = build_schedule(app);
    let buffers = build_buffers(app, &schedule);

    let block_widget = Block::default()
        .borders(Borders::ALL)
        .title(" Timeline View ");
    let inner = block_widget.inner(area);
    let bar_width = inner.width.saturating_sub(7) as usize; // 7 = " HH:MM "

    let max_end = schedule
        .iter()
        .map(|(_, end, _)| *end)
        .chain(app.overflow_tasks.iter().map(|o| o.end_min))
        .max()
        .unwrap_or(DAY_MINUTES)
        .max(DAY_MINUTES);
    let total_rows = ((max_end + 29) / 30).max(48) as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(total_rows);
    let mut cursor_start_row: Option<usize> = None;
    let now_min = app.is_today().then(current_minutes);

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
                Span::styled("       ", Style::default().fg(Color::Red)),
                Span::styled(
                    padded,
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // 時刻ラベル (毎正時のみ)
        let label = if slot_start % 60 == 0 {
            format!(" {:>5} ", format_time(slot_start))
        } else {
            "       ".to_string()
        };

        // オーバーフロータスク
        if let Some(ov) = app
            .overflow_tasks
            .iter()
            .find(|o| o.start_min < slot_end && o.end_min > slot_start)
        {
            let is_start = slot_start <= ov.start_min && ov.start_min < slot_end;
            let color = muted_color(category_color(&ov.category_id));
            let text = if is_start {
                format!("  {} ", ov.title)
            } else {
                String::new()
            };
            let padded = format!("{:<width$}", text, width = bar_width);

            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    padded,
                    Style::default().bg(color).fg(Color::Rgb(200, 200, 200)),
                ),
            ]));
            continue;
        }

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
            let is_selected = idx == app.cursor;

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
                format!("{marker} {}", task.title)
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

    // カーソル位置に基づいてスクロール
    let visible_rows = inner.height as usize;
    let scroll_y = if let Some(cr) = cursor_start_row {
        if cr < visible_rows / 3 {
            0
        } else {
            (cr - visible_rows / 3) as u16
        }
    } else {
        0
    };

    let timeline = Paragraph::new(lines)
        .block(block_widget)
        .scroll((scroll_y, 0));

    f.render_widget(timeline, area);
}

fn draw_timeline_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        " カテゴリ集計",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        " ────────────────────",
        Style::default().fg(Color::DarkGray),
    )));

    for report in &app.category_reports {
        let color = category_color(&report.category_id);
        lines.push(Line::from(vec![
            Span::styled(" █ ", Style::default().fg(color)),
            Span::styled(
                format!("{:<8}", report.category_name),
                Style::default().fg(color),
            ),
            Span::styled(
                format_duration(report.planned_min),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    if !app.category_reports.is_empty() {
        lines.push(Line::from(""));
        let total: i32 = app.category_reports.iter().map(|r| r.planned_min).sum();
        lines.push(Line::from(vec![
            Span::styled(" 合計 ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} / 24h", format_duration(total)),
                Style::default().fg(Color::White),
            ),
        ]));

        let remaining = app.remaining_minutes();
        let rem_color = if remaining < 0 {
            Color::Red
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::styled(" 余り ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_duration(remaining), Style::default().fg(rem_color)),
        ]));
    }

    let sidebar = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::LEFT | Borders::TOP | Borders::BOTTOM)
            .title(" Stats "),
    );
    f.render_widget(sidebar, area);
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

fn build_schedule(app: &App) -> Vec<(i32, i32, &Task)> {
    let mut schedule: Vec<(i32, i32, &Task)> = Vec::with_capacity(app.tasks.len());
    let mut current_min = app.overflow_tasks.last().map(|o| o.end_min).unwrap_or(0);

    for task in &app.tasks {
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

fn build_buffers(app: &App, schedule: &[(i32, i32, &Task)]) -> Vec<(i32, i32)> {
    let mut buffers = Vec::new();
    let mut previous_end = app
        .overflow_tasks
        .last()
        .map(|overflow| overflow.end_min)
        .unwrap_or(0);

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

fn draw_modal_if_needed(f: &mut Frame, app: &App) {
    match &app.input_mode {
        InputMode::Normal => {}
        InputMode::TaskForm(form) => draw_task_form_modal(f, app, form),
        InputMode::ConfirmDelete { title, .. } => draw_delete_modal(f, app, title),
        InputMode::BacklogSelect { .. } => {}
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

fn category_name(app: &App, cat_id: &str) -> String {
    app.categories
        .iter()
        .find(|cat| cat.id == cat_id)
        .map(|cat| format!("{} {}", cat.icon, cat.name))
        .unwrap_or_else(|| cat_id.to_string())
}
