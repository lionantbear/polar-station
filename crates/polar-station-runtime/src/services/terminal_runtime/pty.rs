//! PTY process mechanics. No SSH, tmux, session or Pane decisions.
use crate::terminal_io::AsyncTerminalFd;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::sync::Arc;

pub(super) struct SpawnedPty {
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub io: Arc<AsyncTerminalFd>,
}

pub(super) fn spawn(command: CommandBuilder, size: PtySize) -> Result<SpawnedPty> {
    let pair = native_pty_system()
        .openpty(size)
        .context("failed to open Host terminal PTY")?;
    // Finish fallible descriptor preparation before creating an owned child.
    let io = Arc::new(AsyncTerminalFd::duplicate(
        pair.master
            .as_raw_fd()
            .context("Host PTY descriptor is missing")?,
    )?);
    let child = pair
        .slave
        .spawn_command(command)
        .context("failed to start Host terminal child")?;
    drop(pair.slave);
    Ok(SpawnedPty {
        master: pair.master,
        child,
        io,
    })
}
