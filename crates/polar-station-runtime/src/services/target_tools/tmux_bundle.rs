//! Immutable upstream payloads; staging reuses the selected target's SSH lease.
use anyhow::{Context, Result};
use polar_tmux_core::TmuxTarget;
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

fn payload(os: &str, arch: &str) -> Result<&'static [u8]> {
    Ok(match (os, arch) {
        ("Linux", "x86_64") => include_bytes!("tmux-3.8/tmux-3.8-linux-x86_64.tar.gz"),
        ("Linux", "aarch64" | "arm64") => include_bytes!("tmux-3.8/tmux-3.8-linux-arm64.tar.gz"),
        ("Darwin", "x86_64") => include_bytes!("tmux-3.8/tmux-3.8-macos-x86_64.tar.gz"),
        ("Darwin", "arm64") => include_bytes!("tmux-3.8/tmux-3.8-macos-arm64.tar.gz"),
        _ => anyhow::bail!("No official tmux 3.8 package for {os}/{arch}"),
    })
}
fn quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }

async fn upload(target: &TmuxTarget, path: &str, bytes: &[u8]) -> Result<()> {
    // Publish an archive only after its complete stdin transfer. No terminal bytes
    // or large IPC/argv frames are used to transfer the payload.
    let script = format!("set -eu; umask 077; mkdir -p \"$HOME/.cache/polar-station\"; f=$(mktemp \"$HOME/.cache/polar-station/tmux-stage.XXXXXXXX\"); trap 'rm -f -- \"$f\"' EXIT HUP INT TERM; cat > \"$f\"; mv -f -- \"$f\" {}", quote(path));
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut command = super::super::target_directory::target_command(target, &script, false)?;
        let mut child = command.kill_on_drop(true).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
        let mut input = child.stdin.take().context("Payload stdin unavailable")?;
        input.write_all(bytes).await?;
        input.shutdown().await?;
        drop(input);
        let output = child.wait_with_output().await?;
        anyhow::ensure!(output.status.success(), "Cannot stage official tmux payload: {}", String::from_utf8_lossy(&output.stderr));
        Ok::<_, anyhow::Error>(())
    }).await.context("Official tmux transfer timed out")?
}

pub(super) async fn stage(target: &TmuxTarget) -> Result<String> {
    let facts = super::super::target_directory::target_output(target,
        "printf '%s\\n%s\\n%s\\n' \"$(uname -s)\" \"$(uname -m)\" \"$HOME\"; if [ \"$(uname -s)\" = Darwin ]; then sw_vers -productVersion; fi").await?;
    let facts = std::str::from_utf8(&facts)?;
    let mut lines = facts.lines();
    let os = lines.next().context("Target OS unavailable")?;
    let arch = lines.next().context("Target CPU unavailable")?;
    let home = lines.next().filter(|s| s.starts_with('/') && !s.chars().any(char::is_control)).context("Target Home unavailable")?;
    if os == "Darwin" {
        let major = lines.next().and_then(|v| v.split('.').next()).and_then(|v| v.parse::<u32>().ok()).context("Target macOS version unavailable")?;
        anyhow::ensure!(major >= 15, "Official tmux 3.8 requires macOS 15 or later");
    }
    let bytes = payload(os, arch)?;
    let licenses = include_bytes!("tmux-3.8/LICENSES.tar.gz");
    let digest = format!("{:x}", Sha256::digest(bytes));
    let license_digest = format!("{:x}", Sha256::digest(licenses));
    let archive = format!("{home}/.cache/polar-station/tmux-3.8-{digest}.tar.gz");
    let notices = format!("{home}/.cache/polar-station/tmux-notices-{license_digest}.tar.gz");
    upload(target, &archive, bytes).await?;
    upload(target, &notices, licenses).await?;
    Ok(format!("export STATION_TMUX_ARCHIVE={} STATION_TMUX_SHA256={} STATION_TMUX_LICENSES={} STATION_TMUX_LICENSES_SHA256={}", quote(&archive), quote(&digest), quote(&notices), quote(&license_digest)))
}


