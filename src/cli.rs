use anyhow::{Context, Result, bail};
use chrono::Local;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension};

use crate::{db, model};

#[derive(Parser)]
#[command(name = "ytasky", about = "Terminal time-blocking scheduler")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// タスク一覧
    List {
        /// 対象日 (YYYY-MM-DD, デフォルト: 今日)
        #[arg(short, long)]
        date: Option<String>,
    },
    /// タスク追加
    Add {
        /// タスク名
        #[arg(short, long)]
        title: String,
        /// カテゴリID (work, study, sleep, meal, exercise, personal, break, commute, errand)
        #[arg(short, long)]
        category: String,
        /// 所要時間(分)
        #[arg(short = 'D', long)]
        duration: i32,
        /// 対象日 (YYYY-MM-DD)
        #[arg(long)]
        date: Option<String>,
        /// 固定開始時刻 (HH:MM)
        #[arg(short, long)]
        fixed_start: Option<String>,
    },
    /// タスク編集
    Edit {
        /// タスクID
        id: i64,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        category: Option<String>,
        #[arg(short = 'D', long)]
        duration: Option<i32>,
        #[arg(short, long)]
        fixed_start: Option<String>,
    },
    /// タスク削除
    Delete {
        /// タスクID
        id: i64,
    },
    /// タスク開始
    Start {
        /// タスクID
        id: i64,
    },
    /// タスク完了
    Done {
        /// タスクID
        id: i64,
    },
    /// タスク並び替え
    Move {
        /// 移動するタスクID
        id: i64,
        /// この位置(sort_order)の後に移動
        #[arg(long)]
        after: i64,
    },
    /// 日次レポート
    Report {
        /// 対象日 (YYYY-MM-DD)
        #[arg(short, long)]
        date: Option<String>,
    },
    /// カテゴリ一覧
    Categories,
    /// バックログ一覧
    Backlog,
    /// バックログにタスク追加
    AddBacklog {
        /// タスク名
        #[arg(short, long)]
        title: String,
        /// カテゴリID
        #[arg(short, long)]
        category: String,
        /// 所要時間(分)
        #[arg(short = 'D', long)]
        duration: i32,
        /// 期限 (YYYY-MM-DD or "YYYY-MM-DD HH:MM")
        #[arg(long)]
        deadline: Option<String>,
    },
    /// バックログタスク編集
    EditBacklog {
        /// タスクID
        id: i64,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        category: Option<String>,
        #[arg(short = 'D', long)]
        duration: Option<i32>,
        /// 期限 ("none"で削除)
        #[arg(long)]
        deadline: Option<String>,
    },
    /// バックログタスク削除
    DeleteBacklog {
        /// タスクID
        id: i64,
    },
    /// バックログからスケジュールに挿入
    ScheduleBacklog {
        /// バックログタスクID
        id: i64,
        /// 対象日 (YYYY-MM-DD)
        #[arg(long)]
        date: Option<String>,
        /// 挿入位置 (0始まり、省略で末尾)
        #[arg(long)]
        position: Option<usize>,
    },
    /// スケジュールタスクをバックログに送る
    ToBacklog {
        /// タスクID
        id: i64,
    },
    /// 繰り返しルール一覧
    ListRecurrences,
    /// 繰り返しルール追加
    AddRecurrence {
        /// タスク名
        #[arg(short, long)]
        title: String,
        /// カテゴリID
        #[arg(short, long)]
        category: String,
        /// 所要時間(分)
        #[arg(short = 'D', long)]
        duration: i32,
        /// 固定開始時刻 (HH:MM)
        #[arg(short, long)]
        fixed_start: Option<String>,
        /// daily, weekly, monthly
        #[arg(short, long)]
        pattern: String,
        /// JSON: {\"days\": [1,3,5]}
        #[arg(long)]
        pattern_data: Option<String>,
        /// 開始日 (YYYY-MM-DD)
        #[arg(long)]
        start_date: String,
        /// 終了日 (YYYY-MM-DD)
        #[arg(long)]
        end_date: Option<String>,
    },
    /// 繰り返しルール編集
    EditRecurrence {
        /// ルールID
        id: i64,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        category: Option<String>,
        #[arg(short = 'D', long)]
        duration: Option<i32>,
        #[arg(short, long)]
        fixed_start: Option<String>,
        #[arg(short, long)]
        pattern: Option<String>,
        #[arg(long)]
        pattern_data: Option<String>,
        #[arg(long)]
        start_date: Option<String>,
        #[arg(long)]
        end_date: Option<String>,
    },
    /// 繰り返しルール削除
    DeleteRecurrence {
        /// ルールID
        id: i64,
    },
    /// Google Calendar 連携
    Gcal {
        #[command(subcommand)]
        action: GcalAction,
    },
    /// MCP サーバー起動
    Mcp,
}

#[derive(Subcommand)]
pub enum GcalAction {
    /// OAuth2認証
    Auth {
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        client_secret: String,
    },
    /// 同期実行
    Sync,
    /// カレンダー一覧
    Calendars,
    /// カレンダーの有効/無効切替
    ToggleCalendar {
        /// カレンダーID
        calendar_id: String,
    },
    /// 設定状態を表示
    Status,
    /// 認証情報を削除
    Logout,
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn current_minutes() -> i32 {
    let now = Local::now();
    now.format("%H").to_string().parse::<i32>().unwrap() * 60
        + now.format("%M").to_string().parse::<i32>().unwrap()
}

/// HH:MM → 分に変換
fn parse_time(s: &str) -> Result<i32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        bail!("時刻形式が不正: {s} (HH:MM を使用)");
    }
    let h: i32 = parts[0].parse().context("時が不正")?;
    let m: i32 = parts[1].parse().context("分が不正")?;
    Ok(h * 60 + m)
}

fn print_json(value: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

pub fn run(cmd: Commands, conn: &Connection) -> Result<()> {
    match cmd {
        Commands::List { date } => {
            let date = date.unwrap_or_else(today);
            let tasks = db::load_tasks(conn, &date)?;
            print_json(&tasks);
        }
        Commands::Add {
            title,
            category,
            duration,
            date,
            fixed_start,
        } => {
            let date = date.unwrap_or_else(today);
            let fixed = fixed_start.map(|s| parse_time(&s)).transpose()?;
            let id = db::insert_task(conn, &date, &title, &category, duration, fixed)?;
            print_json(&serde_json::json!({ "id": id, "date": date }));
        }
        Commands::Edit {
            id,
            title,
            category,
            duration,
            fixed_start,
        } => {
            // 現在の値を取得して、指定されたフィールドだけ更新
            let tasks = db::load_tasks(
                conn,
                &conn
                    .query_row("SELECT date FROM tasks WHERE id = ?1", [id], |r| {
                        r.get::<_, String>(0)
                    })
                    .context("タスクが見つからない")?,
            )?;
            let task = tasks
                .iter()
                .find(|t| t.id == id)
                .context("タスクが見つからない")?;

            let new_title = title.as_deref().unwrap_or(&task.title);
            let new_category = category.as_deref().unwrap_or(&task.category_id);
            let new_duration = duration.unwrap_or(task.duration_min);
            let new_fixed = match &fixed_start {
                Some(s) if s == "none" => None,
                Some(s) => Some(parse_time(s)?),
                None => task.fixed_start,
            };

            db::update_task(conn, id, new_title, new_category, new_duration, new_fixed)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::Delete { id } => {
            db::delete_task(conn, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::Start { id } => {
            let mins = current_minutes();
            db::update_actual(conn, id, Some(mins), None)?;
            print_json(&serde_json::json!({
                "ok": true,
                "id": id,
                "actual_start": model::format_time(mins),
            }));
        }
        Commands::Done { id } => {
            let mins = current_minutes();
            // actual_start が未設定なら同時に設定
            let current_start: Option<i32> = conn
                .query_row("SELECT actual_start FROM tasks WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .context("タスクが見つからない")?
                .flatten();
            let start = current_start.unwrap_or(mins);
            db::update_actual(conn, id, Some(start), Some(mins))?;
            print_json(&serde_json::json!({
                "ok": true,
                "id": id,
                "actual_start": model::format_time(start),
                "actual_end": model::format_time(mins),
            }));
        }
        Commands::Move { id, after } => {
            db::swap_sort_order(conn, id, after)?;
            print_json(&serde_json::json!({ "ok": true }));
        }
        Commands::Report { date } => {
            let date = date.unwrap_or_else(today);
            let by_cat = db::report_by_category(conn, &date)?;
            let by_title = db::report_by_title(conn, &date)?;
            print_json(&serde_json::json!({
                "date": date,
                "by_category": by_cat,
                "by_title": by_title,
            }));
        }
        Commands::Categories => {
            let cats = db::load_categories(conn)?;
            print_json(&cats);
        }
        Commands::Backlog => {
            let tasks = db::load_backlog_tasks(conn)?;
            print_json(&tasks);
        }
        Commands::AddBacklog {
            title,
            category,
            duration,
            deadline,
        } => {
            let id =
                db::insert_backlog_task(conn, &title, &category, duration, deadline.as_deref())?;
            print_json(&serde_json::json!({ "id": id }));
        }
        Commands::EditBacklog {
            id,
            title,
            category,
            duration,
            deadline,
        } => {
            let task = db::load_task_by_id(conn, id)?.context("タスクが見つからない")?;

            let new_title = title.as_deref().unwrap_or(&task.title);
            let new_category = category.as_deref().unwrap_or(&task.category_id);
            let new_duration = duration.unwrap_or(task.duration_min);
            let new_deadline = match &deadline {
                Some(s) if s == "none" => None,
                Some(s) => Some(s.as_str()),
                None => task.deadline.as_deref(),
            };

            db::update_task_with_deadline(
                conn,
                id,
                new_title,
                new_category,
                new_duration,
                task.fixed_start,
                new_deadline,
            )?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::DeleteBacklog { id } => {
            db::delete_task(conn, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::ScheduleBacklog { id, date, position } => {
            let date = date.unwrap_or_else(today);
            match position {
                Some(pos) => db::insert_backlog_task_at(conn, id, &date, pos)?,
                None => db::append_backlog_task(conn, id, &date)?,
            }
            print_json(&serde_json::json!({ "ok": true, "id": id, "date": date }));
        }
        Commands::ToBacklog { id } => {
            db::set_backlog_flag(conn, id, true)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::ListRecurrences => {
            let recurrences = db::load_recurrences(conn)?;
            print_json(&recurrences);
        }
        Commands::AddRecurrence {
            title,
            category,
            duration,
            fixed_start,
            pattern,
            pattern_data,
            start_date,
            end_date,
        } => {
            let fixed = fixed_start.map(|s| parse_time(&s)).transpose()?;
            let id = db::insert_recurrence(
                conn,
                &title,
                &category,
                duration,
                fixed,
                &pattern,
                pattern_data.as_deref(),
                &start_date,
                end_date.as_deref(),
            )?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::EditRecurrence {
            id,
            title,
            category,
            duration,
            fixed_start,
            pattern,
            pattern_data,
            start_date,
            end_date,
        } => {
            let recurrences = db::load_recurrences(conn)?;
            let current = recurrences
                .into_iter()
                .find(|item| item.id == id)
                .context("繰り返しルールが見つからない")?;

            let new_title = title.unwrap_or(current.title);
            let new_category = category.unwrap_or(current.category_id);
            let new_duration = duration.unwrap_or(current.duration_min);
            let new_fixed_start = match fixed_start.as_deref() {
                Some("none") => None,
                Some(s) => Some(parse_time(s)?),
                None => current.fixed_start,
            };
            let new_pattern = pattern.unwrap_or(current.pattern);
            let new_pattern_data = match pattern_data.as_deref() {
                Some("none") => None,
                Some(data) => Some(data),
                None => current.pattern_data.as_deref(),
            };
            let new_start_date = start_date.unwrap_or(current.start_date);
            let new_end_date = match end_date.as_deref() {
                Some("none") => None,
                Some(date) => Some(date),
                None => current.end_date.as_deref(),
            };

            db::update_recurrence(
                conn,
                id,
                &new_title,
                &new_category,
                new_duration,
                new_fixed_start,
                &new_pattern,
                new_pattern_data,
                &new_start_date,
                new_end_date,
            )?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::DeleteRecurrence { id } => {
            db::delete_recurrence(conn, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::Gcal { .. } => {
            // Gcal は main.rs から直接呼ぶ (async)
            unreachable!();
        }
        Commands::Mcp => {
            // MCP は main.rs から直接呼ぶ
            unreachable!();
        }
    }
    Ok(())
}

pub async fn run_gcal(action: GcalAction, conn: &Connection) -> Result<()> {
    match action {
        GcalAction::Auth {
            client_id,
            client_secret,
        } => {
            crate::gcal::run_auth_flow(conn, &client_id, &client_secret).await?;

            // 認証後にカレンダー一覧を取得して保存
            println!("カレンダー一覧を取得中...");
            let cals = crate::gcal::fetch_and_store_calendars(conn).await?;
            println!("{}個のカレンダーを取得", cals.len());
            for cal in &cals {
                let name = cal.summary.as_deref().unwrap_or(&cal.id);
                println!("  [x] {name}");
            }
        }
        GcalAction::Sync => {
            if !db::gcal_is_configured(conn)? {
                bail!(
                    "GCalが未設定。先に `ytasky gcal auth --client-id=... --client-secret=...` を実行"
                );
            }
            println!("同期中...");
            let result = crate::gcal::sync_all(conn).await?;
            println!(
                "同期完了: {}件同期, {}件削除",
                result.events_synced, result.events_deleted
            );
        }
        GcalAction::Calendars => {
            if !db::gcal_is_configured(conn)? {
                bail!("GCalが未設定");
            }
            // refresh from API
            let _ = crate::gcal::fetch_and_store_calendars(conn).await;
            let calendars = db::gcal_load_calendars(conn)?;
            if calendars.is_empty() {
                println!("カレンダーなし");
            } else {
                for cal in &calendars {
                    let mark = if cal.enabled { "x" } else { " " };
                    println!("[{mark}] {} ({})", cal.name, cal.calendar_id);
                }
            }
        }
        GcalAction::ToggleCalendar { calendar_id } => {
            let calendars = db::gcal_load_calendars(conn)?;
            let cal = calendars
                .iter()
                .find(|c| c.calendar_id == calendar_id)
                .context("カレンダーが見つからない")?;
            let new_enabled = !cal.enabled;
            db::gcal_set_calendar_enabled(conn, &calendar_id, new_enabled)?;
            let status = if new_enabled { "有効" } else { "無効" };
            println!("{}: {} → {status}", cal.name, calendar_id);
        }
        GcalAction::Status => {
            let configured = db::gcal_is_configured(conn)?;
            println!("認証: {}", if configured { "済み" } else { "未設定" });
            if configured {
                if let Some(last) = db::gcal_get_config(conn, "last_sync")? {
                    println!("最終同期: {last}");
                }
                let sync_days = db::gcal_get_config(conn, "sync_days")?
                    .unwrap_or_else(|| "30".to_string());
                println!("同期日数: {sync_days}");
                let default_cat = db::gcal_get_config(conn, "default_category")?
                    .unwrap_or_else(|| "personal".to_string());
                println!("カテゴリ: {default_cat}");
                let calendars = db::gcal_load_calendars(conn)?;
                let enabled_count = calendars.iter().filter(|c| c.enabled).count();
                println!(
                    "カレンダー: {enabled_count}/{} 有効",
                    calendars.len()
                );
            }
        }
        GcalAction::Logout => {
            db::gcal_clear_all(conn)?;
            println!("GCal認証情報を削除した");
        }
    }
    Ok(())
}
