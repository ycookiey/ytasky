mod app;
mod cli;
mod db;
mod gcal;
mod history;
mod mcp;
mod model;
mod ui;

use std::{io, time::Duration};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;

fn main() -> Result<()> {
    let args = cli::Cli::parse();

    match args.command {
        None => run_tui(),
        Some(cli::Commands::Mcp) => run_mcp(),
        Some(cli::Commands::Gcal { action }) => run_gcal(action),
        Some(cmd) => {
            let conn = db::init()?;
            cli::run(cmd, &conn)
        }
    }
}

#[tokio::main]
async fn run_mcp() -> Result<()> {
    use rmcp::{ServiceExt, transport::stdio};

    let conn = db::init()?;
    let server = mcp::YtaskyMcp::new(conn);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[tokio::main]
async fn run_gcal(action: cli::GcalAction) -> Result<()> {
    let conn = db::init()?;
    cli::run_gcal(action, &conn).await
}

fn run_tui() -> Result<()> {
    let conn = db::init()?;

    // GCal同期（設定済みの場合のみ、失敗してもTUI起動はブロックしない）
    let sync_message = if db::gcal_is_configured(&conn).unwrap_or(false) {
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(gcal::sync_all(&conn)) {
            Ok(r) => Some(format!("GCal同期完了: {}件", r.events_synced)),
            Err(e) => Some(format!("GCal同期失敗: {e}")),
        }
    } else {
        None
    };

    let mut app = app::App::new(conn)?;
    if let Some(msg) = sync_message {
        app.status_message = Some(msg);
    }

    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Event loop
    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        if event::poll(Duration::from_secs(10))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
