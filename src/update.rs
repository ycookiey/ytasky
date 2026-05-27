//! `ytasky update`: Scoop でインストールした ytasky の自己更新。
//!
//! ytasky.exe を実行したまま `scoop update` を呼ぶと、Windows のファイルロックと
//! Scoop の running-process チェックの双方に阻まれる (自己ロック)。これを避けるため:
//!   1. 背景に溜まった `ytasky mcp` プロセスを kill する
//!   2. 自分 (この update プロセス) の消滅を待ってから `scoop update ytasky` を
//!      実行する detached な PowerShell helper を別コンソールに起動する
//!   3. 自分は即終了する
//! helper が `scoop update` を呼ぶ時点で ytasky.exe は1つも生きていないため、
//! ロックも running 判定も発生しない。
//!
//! 注: 別ターミナルで TUI を開いたままだと、その ytasky.exe が Scoop の
//! running チェックに引っかかる。update は背景の mcp のみを kill 対象とする。

use anyhow::Result;

pub fn run() -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("`ytasky update` は Scoop (Windows) 専用です");
    }
    #[cfg(target_os = "windows")]
    {
        let killed = kill_mcp_processes();
        if killed > 0 {
            println!("background の ytasky mcp を {killed} 個終了した");
        }
        spawn_detached_updater()?;
        println!(
            "別ウィンドウで `scoop update ytasky` を開始した。完了後に ytasky を再起動して反映を確認。"
        );
        Ok(())
    }
}

/// 自分以外の `ytasky mcp` プロセスを列挙して kill する。kill 成功数を返す。
#[cfg(target_os = "windows")]
fn kill_mcp_processes() -> usize {
    use sysinfo::{ProcessesToUpdate, System, get_current_pid};

    let me = get_current_pid().ok();
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut count = 0;
    for (pid, proc_) in sys.processes() {
        if Some(*pid) == me {
            continue;
        }
        let name = proc_.name().to_string_lossy().to_ascii_lowercase();
        if name != "ytasky.exe" && name != "ytasky" {
            continue;
        }
        // コマンドライン引数に `mcp` を含むものだけを対象 (TUI/CLI は除外)。
        let is_mcp = proc_
            .cmd()
            .iter()
            .any(|arg| arg.to_string_lossy() == "mcp");
        if is_mcp && proc_.kill() {
            count += 1;
        }
    }
    count
}

/// 自分の PID が消えるのを待ってから `scoop update ytasky` を実行する
/// 独立コンソールの PowerShell プロセスを起動する。
#[cfg(target_os = "windows")]
fn spawn_detached_updater() -> Result<()> {
    use anyhow::Context;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use sysinfo::get_current_pid;

    // 新しいコンソールで起動し、scoop の進捗をユーザーが見られるようにする。
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let my_pid = get_current_pid()
        .map_err(|e| anyhow::anyhow!("自 PID 取得失敗: {e}"))?
        .as_u32();

    // 自分 (update プロセス) の消滅を待ってから update を走らせる。
    let script = format!(
        "while (Get-Process -Id {my_pid} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 100 }}; scoop update ytasky"
    );

    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .context("scoop update helper の起動失敗 (PowerShell が PATH にあるか確認)")?;
    Ok(())
}
