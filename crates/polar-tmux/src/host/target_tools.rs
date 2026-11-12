//! Missing-tool setup on the existing verified execution-target transport.
use anyhow::{Context, Result};
use polar_tmux_core::{TmuxAvailability, TmuxClient, TmuxTarget};
use polar_tmux_protocol::{SessionTargetToolsV1, SessionToolStateV1 as State};
use std::{io::Write, process::Stdio, time::Duration};
pub(super) const CODEX_RESOLVE:&str="station_codex=codex; if ! command -v codex >/dev/null 2>&1 && [ -x \"$HOME/.local/bin/codex\" ]; then station_codex=\"$HOME/.local/bin/codex\"; fi";
pub(super) async fn inspect(client: &TmuxClient, id: &str, enabled: bool) -> SessionTargetToolsV1 {
    let tmux = async {
        match tokio::time::timeout(Duration::from_secs(2), client.availability()).await {
            Ok(Ok(TmuxAvailability::Available { .. })) => State::Available,
            Ok(Ok(TmuxAvailability::ExecutableMissing { .. })) => State::Missing,
            _ => State::Unavailable,
        }
    };
    let codex = async {
        let script=format!("{CODEX_RESOLVE}; if command -v \"$station_codex\" >/dev/null 2>&1; then printf available; else printf missing; fi");
        match tokio::time::timeout(
            Duration::from_secs(2),
            super::target_directory::target_output(client.target(), &script),
        )
        .await
        {
            Ok(Ok(b)) if b == b"available" => State::Available,
            Ok(Ok(b)) if b == b"missing" => State::Missing,
            _ => State::Unavailable,
        }
    };
    let (tmux, codex) = tokio::join!(tmux, codex);
    SessionTargetToolsV1 {
        execution_target_id: id.into(),
        tmux,
        codex,
        codex_control_enabled: enabled,
    }
}
pub(super) fn script(tmux: bool, codex: bool, codex_service: bool) -> String {
    format!(
        "set -- {} {} {}\n{}",
        u8::from(tmux),
        u8::from(codex),
        u8::from(codex_service),
        include_str!("target_tool_setup.sh").replace("@CODEX_RESOLVE@", CODEX_RESOLVE)
    )
}
pub(super) async fn launch(
    target: &TmuxTarget,
    tmux: bool,
    codex: bool,
    codex_service: bool,
) -> Result<()> {
    let command =
        super::target_directory::target_command(target, &script(tmux, codex, codex_service), true)?;
    let quote = |x: &std::ffi::OsStr| -> Result<String> {
        Ok(format!(
            "'{}'",
            x.to_str()
                .context("Terminal argument is not UTF-8")?
                .replace('\'', "'\\''")
        ))
    };
    let mut argv = vec![quote(command.as_std().get_program())?];
    for a in command.as_std().get_args() {
        argv.push(quote(a)?)
    }
    let dir = tempfile::Builder::new()
        .prefix("polar-station-tools-")
        .tempdir()?;
    let path = dir.path().join("setup.command");
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)?;
    writeln!(file,"#!/bin/sh\ntrap 'rm -f -- \"$0\"; rmdir -- \"$(dirname -- \"$0\")\"' EXIT\n{}\nstation_status=$?\nprintf '\\nSetup exited with status %s. Return to Polar Station to check the machine. Press Enter to close.\\n' \"$station_status\"\nread -r station_close\nexit \"$station_status\"",argv.join(" "))?;
    #[cfg(target_os = "macos")]
    let mut terminal = {
        let mut c = tokio::process::Command::new("/usr/bin/open");
        c.args(["-a", "Terminal"]).arg(&path);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut terminal = {
        let terminal = ["xdg-terminal-exec", "x-terminal-emulator"]
            .into_iter()
            .find_map(|name| {
                std::env::var_os("PATH").and_then(|p| {
                    std::env::split_paths(&p)
                        .map(|p| p.join(name))
                        .find(|p| p.is_file())
                })
            })
            .context("No system terminal launcher is available")?;
        let mut c = tokio::process::Command::new(&terminal);
        if terminal
            .file_name()
            .is_some_and(|n| n == "x-terminal-emulator")
        {
            c.arg("-e");
        }
        c.arg(&path);
        c
    };
    terminal
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = terminal
        .spawn()
        .context("Cannot open the system terminal")?;
    let owned = dir.keep();
    tokio::spawn(async move {
        if !child.wait().await.is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_dir(owned);
        }
    });
    Ok(())
}


