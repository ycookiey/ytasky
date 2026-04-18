use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub date: String,
    pub sort_order: i32,
    pub title: String,
    pub category_id: String,
    pub duration_min: i32,
    pub fixed_start: Option<i32>,
    pub actual_start: Option<i32>,
    pub actual_end: Option<i32>,
    pub recurrence_id: Option<i64>,
    pub is_backlog: bool,
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recurrence {
    pub id: i64,
    pub title: String,
    pub category_id: String,
    pub duration_min: i32,
    pub fixed_start: Option<i32>,
    pub pattern: String,
    pub pattern_data: Option<String>,
    pub start_date: String,
    pub end_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternData {
    pub days: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RecurrenceException {
    pub recurrence_id: i64,
    pub date: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OverflowTask {
    pub title: String,
    pub category_id: String,
    pub start_min: i32,
    pub end_min: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryReport {
    pub category_id: String,
    pub category_name: String,
    pub planned_min: i32,
    pub actual_min: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TitleReport {
    pub title: String,
    pub category_id: String,
    pub planned_min: i32,
    pub actual_min: i32,
}

/// 分を "HH:MM" に変換（24:00以降も対応）
pub fn format_time(minutes: i32) -> String {
    let wrapped = minutes.rem_euclid(24 * 60);
    let h = wrapped / 60;
    let m = wrapped % 60;
    format!("{h:02}:{m:02}")
}

/// 分を "XhYY" / "Xm" に変換（負値対応）
pub fn format_duration(minutes: i32) -> String {
    let sign = if minutes < 0 { "-" } else { "" };
    let abs = minutes.saturating_abs();
    let h = abs / 60;
    let m = abs % 60;
    if h > 0 {
        format!("{sign}{h}h{m:02}")
    } else {
        format!("{sign}{m}m")
    }
}

/// 期限を短い表示に整形
/// - 過去: !Nd
/// - 当日: 今日
/// - 7日以内: Nd
/// - それ以外: M/D または M/D HH:MM
pub fn format_deadline(deadline: &str, today: &str) -> String {
    let today_date = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    let (date_part, time_part) = split_deadline(deadline);
    let Some(deadline_date) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok() else {
        return deadline.to_string();
    };

    let Some(today_date) = today_date else {
        return format_deadline_fallback(deadline_date, time_part);
    };

    let delta_days = (deadline_date - today_date).num_days();
    if delta_days < 0 {
        return format!("!{}d", -delta_days);
    }
    if delta_days == 0 {
        return "今日".to_string();
    }
    if delta_days <= 7 {
        return format!("{delta_days}d");
    }

    format_deadline_fallback(deadline_date, time_part)
}

fn split_deadline(deadline: &str) -> (&str, Option<&str>) {
    if let Some((date, time)) = deadline.split_once(' ') {
        (date, Some(time))
    } else {
        (deadline, None)
    }
}

fn format_deadline_fallback(date: NaiveDate, time_part: Option<&str>) -> String {
    let md = format!("{}/{}", date.month(), date.day());
    match time_part {
        Some(time) if !time.trim().is_empty() => format!("{md} {time}"),
        _ => md,
    }
}
