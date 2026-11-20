//! Select one native credential directory without copying credentials or runtime state.
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};

pub(crate) fn resolve(explicit: Option<&Path>, profile_home: &Path) -> Result<PathBuf> {
    select(
        explicit,
        profile_home,
        std::env::var_os("CODEX_HOME").map(PathBuf::from).as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
}

fn select(
    explicit: Option<&Path>,
    profile: &Path,
    native: Option<&Path>,
    user: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(home) = explicit {
        ensure!(home.is_absolute(), "auth_home must be absolute");
        validate_file(home)?;
        return Ok(home.to_path_buf());
    }
    let mut candidates = vec![profile.to_path_buf()];
    if let Some(home) = native.filter(|p| !p.as_os_str().is_empty()) {
        ensure!(home.is_absolute(), "CODEX_HOME must be absolute");
        candidates.push(home.to_path_buf());
    }
    if let Some(home) = user {
        candidates.push(home.join(".codex"));
    }
    for home in candidates {
        if validate_file(&home)? {
            return Ok(home);
        }
    }
    Ok(profile.to_path_buf())
}

fn validate_file(home: &Path) -> Result<bool> {
    let path = home.join("auth.json");
    let metadata = match std::fs::metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("inspect native authentication source"),
    };
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= 256 * 1024,
        "native authentication source has invalid type or size"
    );
    let bytes = std::fs::read(path).context("read native authentication source")?;
    // Do not expose parser errors that can contain credential bytes.
    crate::codex_app_server_bootstrap::validate_native_auth(&bytes)?;
    Ok(true)
}


