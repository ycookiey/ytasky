use anyhow::{Context, Result, bail};
use chrono::Local;
use clap::{Parser, Subcommand};
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
    /// 操作履歴
    History {
        /// 表示件数 (default: 20)
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// テーブルフィルタ
        #[arg(short, long)]
        table: Option<String>,
    },
    /// MCP サーバー起動
    Mcp,
    /// ybasey schema を初期化
    Init {
        /// 既存 schema を上書き (data dir ごと再作成)
        #[arg(long)]
        force: bool,
        /// 確認プロンプトを skip
        #[arg(long, short)]
        yes: bool,
    },
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

pub fn run(cmd: Commands, db: &mut ybasey::Database) -> Result<()> {
    match cmd {
        Commands::List { date } => {
            let date = date.unwrap_or_else(today);
            let tasks = db::load_tasks(db, &date)?;
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
            let id = db::insert_task(db, &date, &title, &category, duration, fixed)?;
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
            let task = db::load_task_by_id(db, id)?
                .context("タスクが見つからない")?;

            let new_title = title.as_deref().unwrap_or(&task.title);
            let new_category = category.as_deref().unwrap_or(&task.category_id);
            let new_duration = duration.unwrap_or(task.duration_min);
            let new_fixed = match &fixed_start {
                Some(s) if s == "none" => None,
                Some(s) => Some(parse_time(s)?),
                None => task.fixed_start,
            };

            db::update_task(db, id, new_title, new_category, new_duration, new_fixed)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::Delete { id } => {
            db::delete_task(db, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::Start { id } => {
            let mins = current_minutes();
            db::update_actual(db, id, Some(mins), None)?;
            print_json(&serde_json::json!({
                "ok": true,
                "id": id,
                "actual_start": model::format_time(mins),
            }));
        }
        Commands::Done { id } => {
            let mins = current_minutes();
            // actual_start が未設定なら同時に設定
            let current_start: Option<i32> = db::load_task_by_id(db, id)?
                .context("タスクが見つからない")?
                .actual_start;
            let start = current_start.unwrap_or(mins);
            db::update_actual(db, id, Some(start), Some(mins))?;
            print_json(&serde_json::json!({
                "ok": true,
                "id": id,
                "actual_start": model::format_time(start),
                "actual_end": model::format_time(mins),
            }));
        }
        Commands::Move { id, after } => {
            db::swap_sort_order(db, id, after)?;
            print_json(&serde_json::json!({ "ok": true }));
        }
        Commands::Report { date } => {
            let date = date.unwrap_or_else(today);
            let by_cat = db::report_by_category(db, &date)?;
            let by_title = db::report_by_title(db, &date)?;
            print_json(&serde_json::json!({
                "date": date,
                "by_category": by_cat,
                "by_title": by_title,
            }));
        }
        Commands::Categories => {
            let cats = db::load_categories(db)?;
            print_json(&cats);
        }
        Commands::Backlog => {
            let tasks = db::load_backlog_tasks(db)?;
            print_json(&tasks);
        }
        Commands::AddBacklog {
            title,
            category,
            duration,
            deadline,
        } => {
            let id =
                db::insert_backlog_task(db, &title, &category, duration, deadline.as_deref())?;
            print_json(&serde_json::json!({ "id": id }));
        }
        Commands::EditBacklog {
            id,
            title,
            category,
            duration,
            deadline,
        } => {
            let task = db::load_task_by_id(db, id)?.context("タスクが見つからない")?;

            let new_title = title.as_deref().unwrap_or(&task.title);
            let new_category = category.as_deref().unwrap_or(&task.category_id);
            let new_duration = duration.unwrap_or(task.duration_min);
            let new_deadline = match &deadline {
                Some(s) if s == "none" => None,
                Some(s) => Some(s.as_str()),
                None => task.deadline.as_deref(),
            };

            db::update_task_with_deadline(
                db,
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
            db::delete_task(db, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::ScheduleBacklog { id, date, position } => {
            let date = date.unwrap_or_else(today);
            match position {
                Some(pos) => db::insert_backlog_task_at(db, id, &date, pos)?,
                None => db::append_backlog_task(db, id, &date)?,
            }
            print_json(&serde_json::json!({ "ok": true, "id": id, "date": date }));
        }
        Commands::ToBacklog { id } => {
            db::set_backlog_flag(db, id, true)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::ListRecurrences => {
            let recurrences = db::load_recurrences(db)?;
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
                db,
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
            let recurrences = db::load_recurrences(db)?;
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
                db,
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
            db::delete_recurrence(db, id)?;
            print_json(&serde_json::json!({ "ok": true, "id": id }));
        }
        Commands::History { limit, table } => {
            let entries = db::query_history(db, table.as_deref(), limit)?;
            for entry in &entries {
                println!("{entry}");
            }
            if entries.is_empty() {
                println!("(履歴なし)");
            }
        }
        Commands::Mcp => {
            // MCP は main.rs から直接呼ぶ
            unreachable!();
        }
        Commands::Init { .. } => {
            // Init は main.rs から直接呼ぶ (rusqlite Connection 不要)
            unreachable!();
        }
    }
    Ok(())
}
