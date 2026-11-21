//! Discover literal labels for the picker. OpenSSH itself resolves and verifies the route.
use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};

pub async fn configured_aliases() -> Vec<String> {
    tokio::task::spawn_blocking(|| {
        let mut aliases = BTreeSet::new();
        let mut seen = BTreeSet::new();
        if let Some(home) = std::env::home_dir() {
            let root = home.join(".ssh");
            read_labels(&root, &root.join("config"), &mut seen, &mut aliases);
        }
        let root = Path::new("/etc/ssh");
        read_labels(root, &root.join("ssh_config"), &mut seen, &mut aliases);
        aliases.into_iter().take(128).collect()
    })
    .await
    .unwrap_or_default()
}

fn read_labels(
    root: &Path,
    path: &Path,
    seen: &mut BTreeSet<std::path::PathBuf>,
    aliases: &mut BTreeSet<String>,
) {
    if seen.len() >= 64 {
        return;
    }
    let Ok(path) = path.canonicalize() else { return };
    if !seen.insert(path.clone()) {
        return;
    }
    let Ok(metadata) = path.metadata() else { return };
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(path) else { return };
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some(split) = line.find(|ch: char| ch.is_ascii_whitespace() || ch == '=') else {
            continue;
        };
        let key = &line[..split];
        let args = line[split..].trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '=');
        if key.eq_ignore_ascii_case("host") {
            for label in args.split_ascii_whitespace().map(|word| word.trim_matches('"')) {
                if !label.contains('@') && crate::is_canonical_ssh_destination(label) {
                    aliases.insert(label.to_ascii_lowercase());
                }
            }
        } else if key.eq_ignore_ascii_case("include") {
            for pattern in args.split_ascii_whitespace().map(|word| word.trim_matches('"')) {
                let path = if let Some(relative) = pattern.strip_prefix("~/") {
                    let Some(home) = std::env::home_dir() else { continue };
                    home.join(relative)
                } else {
                    root.join(pattern)
                };
                if let Some(paths) = path.to_str().and_then(|pattern| glob::glob(pattern).ok()) {
                    for path in paths.take(64).flatten() {
                        read_labels(root, &path, seen, aliases);
                    }
                }
            }
        }
    }
}

/// Verify SSH independently of any software installed on the target.
pub async fn check_setup_destination(destination: &str) -> std::io::Result<bool> {
    let spec = crate::direct_command(destination, "true".into(), crate::RemotePty::None)?;
    let mut ssh = tokio::process::Command::new(spec.program);
    ssh.args([
        "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
        "-o", "ClearAllForwardings=yes", "-o", "PermitLocalCommand=no",
        "-o", "RemoteCommand=none", "--",
    ])
        .args(spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(10), ssh.status()).await??;
    Ok(status.success())
}

/// OpenSSH handles normal interactive host-key and authentication prompts.
pub fn setup_login_command(destination: &str) -> std::io::Result<String> {
    let spec = crate::direct_command(destination, "true".into(), crate::RemotePty::Required)?;
    let command = std::iter::once(spec.program)
        .chain(["-o".into(), "ClearAllForwardings=yes".into(), "-o".into(),
                "PermitLocalCommand=no".into(), "-o".into(), "RemoteCommand=none".into()])
        .chain(spec.args)
        .map(|value| crate::quote_posix(&value))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("exec {command}"))
}


