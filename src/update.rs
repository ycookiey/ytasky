//! `ytasky update`: self-update of a Scoop-installed ytasky.
//!
//! Running `scoop update` while ytasky.exe runs is blocked both by the Windows file
//! lock and by Scoop's running-process check. To avoid this, update:
//!   1. Kills the background `ytasky mcp` processes
//!   2. Starts a detached PowerShell helper in a new console that waits for this
//!      process to exit and then runs `scoop update ytasky`
//!   3. Exits immediately
//! When the helper calls `scoop update`, no ytasky.exe is running, so neither the
//! lock nor the running-process check applies.
//!
//! A TUI left open in another terminal still trips Scoop's running-process check:
//! update kills only the background mcp processes.

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

/// Kills every `ytasky mcp` process except this one; returns how many were killed.
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
        // Only processes with an `mcp` argument (TUI/CLI processes are left alone).
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

/// Starts a PowerShell process in its own console that waits for this PID to exit
/// and then runs `scoop update ytasky`.
#[cfg(target_os = "windows")]
fn spawn_detached_updater() -> Result<()> {
    use anyhow::Context;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use sysinfo::get_current_pid;

    // A new console lets the user follow scoop's progress.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let my_pid = get_current_pid()
        .map_err(|e| anyhow::anyhow!("自 PID 取得失敗: {e}"))?
        .as_u32();

    // Wait for this update process to exit before updating.
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
