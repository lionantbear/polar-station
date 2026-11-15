//! Explicit, conflict-safe Codex notify integration for Polar Station.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexNotifyConfigStatus {
    Missing,
    Configured,
    Conflict,
    Invalid,
    Unsafe,
}

pub(crate) fn default_codex_config_path() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(root).join("config.toml"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|root| root.join(".codex/config.toml"))
        .ok_or_else(|| "Codex configuration directory is unavailable".to_owned())
}

pub(crate) fn station_notify_command() -> Result<Vec<String>, String> {
    let executable = std::env::current_exe()
        .map_err(|_| "Polar Station executable path is unavailable".to_owned())?;
    if !executable.is_absolute() {
        return Err("Polar Station executable path is not absolute".to_owned());
    }
    Ok(vec![
        executable.to_string_lossy().into_owned(),
        "codex-notify".to_owned(),
    ])
}

pub(crate) fn inspect_codex_notify(path: &Path, expected: &[String]) -> CodexNotifyConfigStatus {
    if unsafe_path(path) {
        return CodexNotifyConfigStatus::Unsafe;
    }
    let text = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CodexNotifyConfigStatus::Missing;
        }
        Err(_) => return CodexNotifyConfigStatus::Invalid,
    };
    let Ok(document) = text.parse::<DocumentMut>() else {
        return CodexNotifyConfigStatus::Invalid;
    };
    match document.get("notify") {
        None => CodexNotifyConfigStatus::Missing,
        Some(item) if notify_matches(item, expected) => CodexNotifyConfigStatus::Configured,
        Some(_) => CodexNotifyConfigStatus::Conflict,
    }
}

pub(crate) fn configure_codex_notify(
    path: &Path,
    expected: &[String],
) -> Result<CodexNotifyConfigStatus, String> {
    match inspect_codex_notify(path, expected) {
        CodexNotifyConfigStatus::Configured => return Ok(CodexNotifyConfigStatus::Configured),
        CodexNotifyConfigStatus::Conflict => return Ok(CodexNotifyConfigStatus::Conflict),
        CodexNotifyConfigStatus::Invalid => return Ok(CodexNotifyConfigStatus::Invalid),
        CodexNotifyConfigStatus::Unsafe => return Ok(CodexNotifyConfigStatus::Unsafe),
        CodexNotifyConfigStatus::Missing => {}
    }
    let mut document = match fs::read_to_string(path) {
        Ok(value) => value
            .parse::<DocumentMut>()
            .map_err(|_| "Codex configuration is invalid TOML".to_owned())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
        Err(_) => return Err("Codex configuration could not be read".to_owned()),
    };
    if document.get("notify").is_some() {
        return Ok(CodexNotifyConfigStatus::Conflict);
    }
    let mut command = Array::new();
    for argument in expected {
        command.push(argument.as_str());
    }
    document["notify"] = Item::Value(Value::Array(command));
    atomic_replace(path, document.to_string().as_bytes())?;
    Ok(CodexNotifyConfigStatus::Configured)
}

fn notify_matches(item: &Item, expected: &[String]) -> bool {
    let Some(array) = item.as_array() else {
        return false;
    };
    array.len() == expected.len()
        && array
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_str() == Some(expected.as_str()))
}

fn unsafe_path(path: &Path) -> bool {
    // Reject the file and its direct configuration directory when either is a
    // symlink. Platform roots such as macOS `/var -> /private/var` are outside
    // the Station-owned replacement boundary and must not make every valid
    // per-user configuration appear unsafe.
    for candidate in [Some(path), path.parent()].into_iter().flatten() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return true,
        }
    }
    false
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Codex configuration path has no parent".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|_| "Codex configuration directory could not be created".to_owned())?;
    if unsafe_path(path) {
        return Err("Codex configuration path is unsafe".to_owned());
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Codex configuration filename is invalid".to_owned())?;
    let temporary = parent.join(format!(
        ".{file_name}.polar-station-{}.tmp",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "Codex configuration temporary file could not be created".to_owned())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "Codex configuration temporary file could not be written".to_owned())?;
        file.sync_all().map_err(|_| {
            "Codex configuration temporary file could not be synchronized".to_owned()
        })?;
        if unsafe_path(path) {
            return Err("Codex configuration path changed during update".to_owned());
        }
        fs::rename(&temporary, path)
            .map_err(|_| "Codex configuration could not be atomically replaced".to_owned())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}


