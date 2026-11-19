//! Imported bindings use the normal terminal attach owner and guard. No store.
use anyhow::{ensure, Context, Result};
use polar_station_protocol::TerminalSourceV1;
use sha2::{Digest, Sha256};

pub(super) fn check_host(
    targets: &crate::target_registry::TargetRegistry,
    target: &str,
    source: &TerminalSourceV1,
) -> Result<()> {
    source.validate()?;
    ensure!(
        targets.resolve_host_ref(&source.host).as_deref() == Some(target),
        "Configure or review the source Host before opening this imported Session"
    );
    Ok(())
}

pub(super) fn check_session(
    source: &TerminalSourceV1,
    actual: &polar_tmux_core::TmuxSessionBinding,
) -> Result<()> {
    let expected = source
        .session
        .as_ref()
        .context("Original Session identity unavailable")?;
    ensure!(expected.session_id == actual.session_id
        && expected.created_unix_seconds == actual.created_unix_seconds
        && expected.server_generation == format!("{:x}", Sha256::digest(actual.server_generation.as_bytes())),
        "The original tmux Session has changed; the imported reference was not attached to its replacement");
    Ok(())
}


