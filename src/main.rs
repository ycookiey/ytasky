mod app;
mod cli;
mod db;
#[cfg(feature = "gcal")]
mod gcal;
mod history;
mod init;
#[cfg(feature = "mcp")]
mod mcp;
mod model;
mod recurrence;
mod ui;
mod update;

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
        // update は ytasky.exe を置換するため DB を開かない (ロックを掴まない)。
        Some(cli::Commands::Update) => update::run(),
        // gcal-login / gcal-logout は OAuth token のみ扱い DB を必要としない。
        // 未 init 環境でも認証できるよう db::open() を経由しない。
        #[cfg(feature = "gcal")]
        Some(cli::Commands::GcalLogin) => {
            gcal::auth::login()?;
            println!("{}", serde_json::json!({ "ok": true }));
            Ok(())
        }
        #[cfg(feature = "gcal")]
        Some(cli::Commands::GcalLogout) => {
            gcal::auth::logout()?;
            println!("{}", serde_json::json!({ "ok": true }));
            Ok(())
        }
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
        // 親 (MCP クライアント = Claude Code 等) の消滅を監視し、消えたら自律終了する。
        // 通常はクライアント切断で stdin が EOF になり waiting() が返るが、親が
        // 異常終了して stdin が閉じられないと孤児として残るため、その保険。
        spawn_parent_watchdog();

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

/// 起動時の親 PID を取得し、15 秒間隔で生存を監視する tokio タスクを起動する。
/// 親が消滅したらプロセスを終了する (孤児 MCP サーバーの蓄積を防ぐ)。
#[cfg(feature = "mcp")]
fn spawn_parent_watchdog() {
    use sysinfo::{ProcessesToUpdate, System, get_current_pid};

    let Some(parent_pid) = (|| {
        let me = get_current_pid().ok()?;
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[me]), true);
        sys.process(me)?.parent()
    })() else {
        return;
    };

    tokio::spawn(async move {
        let mut sys = System::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            sys.refresh_processes(ProcessesToUpdate::Some(&[parent_pid]), true);
            if sys.process(parent_pid).is_none() {
                eprintln!(
                    "ytasky mcp: 親プロセス {} の消滅を検知。終了します。",
                    parent_pid.as_u32()
                );
                std::process::exit(0);
            }
        }
    });
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

        if event::poll(Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
        // バックグラウンド (gcal lazy sync 等) からの結果を取り込む
        app.poll_background_sync();

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
