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
    /// MCP サーバー起動
    Mcp,
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
        Commands::Mcp => {
            // MCP は main.rs から直接呼ぶ
            unreachable!();
        }
    }
    Ok(())
}
