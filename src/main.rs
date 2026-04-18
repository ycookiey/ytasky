mod app;
mod cli;
mod db;
mod history;
mod init;
#[cfg(feature = "mcp")]
mod mcp;
mod model;
mod recurrence;
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
        #[cfg(feature = "mcp")]
        Some(cli::Commands::Mcp) => run_mcp(),
        #[cfg(not(feature = "mcp"))]
        Some(cli::Commands::Mcp) => {
            eprintln!("ytasky: MCP feature is not enabled. Rebuild with --features mcp.");
            std::process::exit(1);
        }
        Some(cli::Commands::Init { force, yes }) => init::run_init(force, yes),
        Some(cmd) => {
            let mut db = db::open()?;
            cli::run(cmd, &mut db)
        }
    }
}

#[cfg(feature = "mcp")]
fn run_mcp() -> Result<()> {
    use rmcp::ServiceExt;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let db = db::open()?;
        let server = mcp::YtaskyMcpServer::new(db);
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| anyhow::anyhow!("MCP serve error: {e}"))?;
        service
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP wait error: {e}"))?;
        Ok(())
    })
}

fn run_tui() -> Result<()> {
    // panic hook: raw mode terminal を確実に復旧
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original_hook(info);
    }));

    let db = db::open()?;
    let mut app = app::App::new(db)?;

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
