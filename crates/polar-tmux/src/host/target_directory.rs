//! Target-owned directory facts; no terminal or independent SSH connection.
use anyhow::{Context, Result};
use polar_tmux_core::TmuxTarget;
use polar_tmux_protocol::SessionDirectoryV1;
use std::{path::Path, process::Stdio};
use tokio::{io::AsyncReadExt, process::Command};

// Escaped JSON remains below the existing 1 MiB local IPC frame limit.
const MAX_DIRECTORY_BYTES: usize = 128 * 1024;

pub(super) async fn read(
    target: &TmuxTarget,
    id: &str,
    path: Option<&str>,
    list: bool,
) -> Result<SessionDirectoryV1> {
    let directory = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        if matches!(target, TmuxTarget::Local) {
            read_local(id, path, list).await
        } else {
            read_remote(target, id, path, list).await
        }
    })
    .await
    .context("Directory lookup timed out")??;
    check_size(&directory)?;
    Ok(directory)
}

pub(super) fn check_size(directory: &SessionDirectoryV1) -> Result<()> {
    let bytes = directory.home.len()
        + directory.path.len()
        + directory.parent.as_deref().map_or(0, str::len)
        + directory.directories.iter().map(String::len).sum::<usize>();
    anyhow::ensure!(
        bytes <= MAX_DIRECTORY_BYTES && directory.directories.len() <= 4096,
        "Too many folders; enter a more specific path"
    );
    Ok(())
}

async fn read_local(id: &str, path: Option<&str>, list: bool) -> Result<SessionDirectoryV1> {
    let home = std::env::var("HOME").context("Target account Home is unavailable")?;
    let requested = match path {
        None | Some("~") => home.clone(),
        Some(p) if p.starts_with("~/") => format!("{home}/{}", &p[2..]),
        Some(p) => p.to_owned(),
    };
    let path = tokio::fs::canonicalize(&requested).await?;
    anyhow::ensure!(
        tokio::fs::metadata(&path).await?.is_dir(),
        "Not a directory"
    );
    let path = path
        .to_str()
        .context("Directory path is not UTF-8")?
        .to_owned();
    let mut directory = facts(id, home, path, Vec::new());
    if list {
        let mut entries = tokio::fs::read_dir(&directory.path).await?;
        let mut bytes = directory.home.len() + directory.path.len() * 2;
        while let Some(entry) = entries.next_entry().await? {
            if tokio::fs::metadata(entry.path())
                .await
                .is_ok_and(|m| m.is_dir())
            {
                if let Some(path) = entry
                    .path()
                    .to_str()
                    .filter(|p| !p.chars().any(char::is_control))
                {
                    bytes += path.len();
                    anyhow::ensure!(
                        bytes <= MAX_DIRECTORY_BYTES && directory.directories.len() < 4096,
                        "Too many folders; enter a more specific path"
                    );
                    directory.directories.push(path.to_owned());
                }
            }
        }
        directory.directories.sort();
    }
    Ok(directory)
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn remote_script(path: Option<&str>, list: bool) -> String {
    let directory = path.map_or_else(|| "$HOME".into(), quote);
    let listing = if list {
        "for entry in \"$PWD\"/* \"$PWD\"/.[!.]* \"$PWD\"/..?*; do if [ -d \"$entry\" ]; then printf '%s\\0' \"$entry\"; fi; done"
    } else {
        ""
    };
    format!(
        "station_dir={directory}; case \"$station_dir\" in '~') station_dir=$HOME;; '~/'*) station_dir=$HOME/${{station_dir#'~/'}};; esac; CDPATH= cd -P \"$station_dir\" || exit 2; printf '%s\\0%s\\0' \"$HOME\" \"$PWD\"; {listing}"
    )
}
async fn read_remote(
    target: &TmuxTarget,
    id: &str,
    path: Option<&str>,
    list: bool,
) -> Result<SessionDirectoryV1> {
    let bytes = target_output(target, &remote_script(path, list)).await?;
    let mut fields = bytes.split(|b| *b == 0).filter(|s| !s.is_empty());
    let home = String::from_utf8(
        fields
            .next()
            .context("Target Home is unavailable")?
            .to_vec(),
    )?;
    let path = String::from_utf8(
        fields
            .next()
            .context("Target directory is unavailable")?
            .to_vec(),
    )?;
    let mut directories = fields
        .filter_map(|b| std::str::from_utf8(b).ok())
        .filter(|s| !s.chars().any(char::is_control))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    directories.sort();
    directories.dedup();
    Ok(facts(id, home, path, directories))
}
/// Read-only target query on the existing verified transport, bounded like directory reads.
pub(super) async fn target_output(target: &TmuxTarget, script: &str) -> Result<Vec<u8>> {
    let mut command=target_command(target,script,false)?;
    tokio::time::timeout(std::time::Duration::from_secs(10), output(&mut command))
        .await.context("Target query timed out")?

}
pub(super) fn target_command(target:&TmuxTarget,script:&str,interactive:bool)->Result<Command> {
    let command = match target {
        TmuxTarget::MultiplexedSsh {
            destination,
            control_socket,
        } => {
            let mut c = Command::new("ssh");
            c.args([
                if interactive {"-tt"} else {"-T"},
                "-S",
                control_socket,
                "-o",
                "ControlMaster=no",
                "-o",
                "BatchMode=yes",
                "-o",
                "ProxyCommand=/usr/bin/false",
                destination,
                &format!("/bin/sh -c {}", quote(script)),
            ]);
            c
        }
        TmuxTarget::Wsl { distribution } => {
            let mut c = Command::new("wsl.exe");
            if let Some(d) = distribution {
                c.args(["-d", d]);
            }
            c.args(["--", "sh", "-c", script]);
            c
        }
        TmuxTarget::Local => { let mut c=Command::new("/bin/sh");c.args(["-lc",script]);c },
        TmuxTarget::Ssh { .. } => {
            anyhow::bail!("A verified shared SSH lease is required")
        }
    };
    Ok(command)
}
async fn output(command: &mut Command) -> Result<Vec<u8>> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .context("Directory output is unavailable")?
        .take(MAX_DIRECTORY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(
        bytes.len() <= MAX_DIRECTORY_BYTES,
        "Too many folders; enter a more specific path"
    );
    anyhow::ensure!(
        child.wait().await?.success(),
        "Directory is unavailable or permission was denied"
    );
    Ok(bytes)
}
fn facts(id: &str, home: String, path: String, directories: Vec<String>) -> SessionDirectoryV1 {
    SessionDirectoryV1 {
        execution_target_id: id.into(),
        home,
        parent: Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned()),
        path,
        directories,
    }
}


