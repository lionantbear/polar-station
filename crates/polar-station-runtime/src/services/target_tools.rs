//! Missing-tool setup on the existing verified execution-target transport.
mod tmux_bundle;
use anyhow::{Context, Result};
use polar_tmux_core::{TmuxAvailability, TmuxClient, TmuxTarget};
use polar_station_protocol::{
    SessionSetupChecksV1, SessionTargetToolsV1, SessionToolStateV1 as State,
    SetupAuthenticationV1 as Auth,
};
use std::{io::Write, process::Stdio, time::Duration};
pub(super) const CODEX_RESOLVE:&str="station_codex=codex; if ! command -v codex >/dev/null 2>&1 && [ -x \"$HOME/.local/bin/codex\" ]; then station_codex=\"$HOME/.local/bin/codex\"; fi";
pub(super) const CLAUDE_RESOLVE:&str="station_claude=claude; if ! command -v claude >/dev/null 2>&1 && [ -x \"$HOME/.local/bin/claude\" ]; then station_claude=\"$HOME/.local/bin/claude\"; fi";
pub(super) async fn inspect(client: &TmuxClient, id: &str, enabled: bool, diagnostic: bool) -> SessionTargetToolsV1 {
    let tmux = async {
        let state = match tokio::time::timeout(Duration::from_secs(2), client.availability()).await {
            Ok(Ok(TmuxAvailability::Available { .. })) => State::Available,
            Ok(Ok(TmuxAvailability::ExecutableMissing { .. })) => State::Missing,
            _ => State::Unavailable,
        };
        let bytes = if diagnostic && state == State::Missing {
            tokio::time::timeout(Duration::from_millis(700), tmux_bundle::size(client.target()))
                .await.ok().flatten()
        } else {
            None
        };
        (state, bytes)
    };
    let codex = inspect_agent(client.target(), CODEX_RESOLVE, "station_codex",
        (enabled && diagnostic).then_some(r#"
            if status=$("$station_codex" login status 2>&1); then printf ready
            elif [ "$status" = 'Not logged in' ]; then printf sign_in
            else printf failed; fi
        "#));
    let claude = inspect_agent(client.target(), CLAUDE_RESOLVE, "station_claude",
        diagnostic.then_some(r#"
            if ! "$station_claude" auth status --help >/dev/null 2>&1; then
                printf unsupported
            else
                code=0; status=$("$station_claude" auth status 2>/dev/null) || code=$?
                printf '%s\n%s' "$code" "$status"
            fi
        "#));
    let ((tmux, tmux_archive_bytes), (codex, codex_check), (claude, claude_check)) = tokio::join!(tmux, codex, claude);
    let setup_checks = diagnostic.then(|| SessionSetupChecksV1 {
        codex_auth: codex_check.as_deref().map(decode_codex_auth),
        codex_server_ready: None,
        ssh_aliases: Vec::new(),
        tmux_archive_bytes,
        claude_auth: claude_check.as_deref().map(decode_claude_auth),
    });
    SessionTargetToolsV1 {
        native_setup:true,
        execution_target_id: id.into(),
        tmux,
        codex,
        codex_control_enabled: enabled,
        default_local_codex_endpoint: None,
        claude: Some(claude),
        setup_checks,
    }
}
async fn inspect_agent(
    target: &TmuxTarget,
    resolve: &str,
    executable: &str,
    auth_check: Option<&str>,
) -> (State, Option<String>) {
    let checks = auth_check.unwrap_or("");
    let script = format!(r#"{resolve}
        if command -v "${executable}" >/dev/null 2>&1; then
            printf 'available\n'
            {checks}
        else
            printf 'missing\n'
        fi
    "#);
    let Ok(Ok(output)) = tokio::time::timeout(
        Duration::from_secs(2),
        super::target_directory::target_output(target, &script),
    ).await else {
        return (State::Unavailable, None);
    };
    let Ok(output) = std::str::from_utf8(&output) else {
        return (State::Unavailable, None);
    };
    match output.split_once('\n') {
        Some(("missing", _)) => (State::Missing, None),
        Some(("available", status)) => (State::Available, auth_check.map(|_| status.trim().into())),
        _ => (State::Unavailable, None),
    }
}

fn decode_codex_auth(status: &str) -> Auth {
    match status {
        "ready" => Auth::Ready,
        "sign_in" => Auth::SignInRequired,
        _ => Auth::CheckFailed,
    }
}

fn decode_claude_auth(status: &str) -> Auth {
    if status == "unsupported" {
        return Auth::Unsupported;
    }
    let Some((exit, json)) = status.split_once('\n') else {
        return Auth::CheckFailed;
    };
    let Ok(result) = serde_json::from_str::<serde_json::Value>(json) else {
        return Auth::CheckFailed;
    };
    match (exit, result.get("loggedIn").and_then(serde_json::Value::as_bool)) {
        ("0", Some(true)) => Auth::Ready,
        ("1", Some(false)) => Auth::SignInRequired,
        _ => Auth::CheckFailed,
    }
}

pub(super) fn script(tmux: bool, codex: bool, codex_service: bool, claude: bool, claude_login: bool) -> String {
    format!(
        "set -- {} {} {} {} {}\n{}",
        u8::from(tmux),
        u8::from(codex),
        u8::from(codex_service),
        u8::from(claude),
        u8::from(claude_login),
        include_str!("target_tool_setup.sh").replace("@TMUX_INSTALL@", include_str!("target_tools/install_tmux.sh")).replace("@CODEX_RESOLVE@", CODEX_RESOLVE).replace("@CLAUDE_RESOLVE@", CLAUDE_RESOLVE)
    )
}
pub(super) async fn prepared_script(target: &TmuxTarget, tmux: bool, codex: bool, codex_service: bool, claude: bool, claude_login: bool) -> Result<String> {
    let prefix = if tmux { tmux_bundle::stage(target).await? } else { String::new() };
    Ok(format!("{prefix}\n{}", script(tmux, codex, codex_service, claude, claude_login)))
}
/// Prepare the original target command without starting a terminal or installer.
pub(super) fn prepare(
    target:&TmuxTarget, id:&str, label:&str, script:&str,
)->Result<polar_station_protocol::ToolSetupLaunchV1> {
    let setup=polar_station_protocol::ToolSetupLaunchV1 {
        id:crate::host_identity::random_uuid_v4(),execution_target_id:id.into(),
        title:format!("Setup · {label}"),
        command:quoted_command(target,script)?,
    };
    setup.validate()?;
    Ok(setup)
}
fn quoted_command(target:&TmuxTarget,script:&str)->Result<String> {
    let command = super::target_directory::target_command(target, script, true)?;
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
    Ok(argv.join(" "))
}
pub(super) async fn launch(
    target: &TmuxTarget,
    tmux: bool,
    codex: bool,
    codex_service: bool,
    claude: bool,
    claude_login: bool,
) -> Result<()> {
    let script=prepared_script(target,tmux,codex,codex_service,claude,claude_login).await?;
    let command=quoted_command(target,&script)?;
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
    writeln!(file,"#!/bin/sh\ntrap 'rm -f -- \"$0\"; rmdir -- \"$(dirname -- \"$0\")\"' EXIT\n{}\nstation_status=$?\nprintf '\\nSetup exited with status %s. Return to Polar Station to check the machine. Press Enter to close.\\n' \"$station_status\"\nread -r station_close\nexit \"$station_status\"",command)?;
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


