//! Existing per-user Station control endpoint selection.
use std::{env, path::PathBuf};
use anyhow::{Context, Result};
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("POLAR_TMUX_IPC_SOCKET") {
        return Ok(PathBuf::from(path));
    }

    #[cfg(target_os = "linux")]
    {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR is required unless POLAR_TMUX_IPC_SOCKET is set")?;
        Ok(PathBuf::from(runtime).join("polar-tmux/host.sock"))
    }

    #[cfg(target_os = "macos")]
    {
        let runtime = env::var_os("TMPDIR")
            .context("TMPDIR is required unless POLAR_TMUX_IPC_SOCKET is set")?;
        Ok(PathBuf::from(runtime).join("polar-tmux/host.sock"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("this Unix platform has no Polar Tmux local IPC path contract")
    }
}
