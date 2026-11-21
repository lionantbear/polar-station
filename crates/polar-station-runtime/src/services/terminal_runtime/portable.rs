//! Imported bindings use the normal terminal attach owner and guard. No store.
use anyhow::{ensure, Context, Result};
use polar_station_protocol::{ObservedTmuxSessionV1, TerminalSourceV1};
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
    ensure!(*expected == observed_identity(actual),
        "The original tmux Session has changed; the imported reference was not attached to its replacement");
    Ok(())
}

pub(super) fn observed_identity(actual: &polar_tmux_core::TmuxSessionBinding) -> ObservedTmuxSessionV1 {
    ObservedTmuxSessionV1 {
        server_generation: format!("{:x}", Sha256::digest(actual.server_generation.as_bytes())),
        session_id: actual.session_id.clone(),
        created_unix_seconds: actual.created_unix_seconds,
    }
}

pub(super) async fn check_original_agent(
    client: &polar_tmux_core::TmuxClient,
    session_id: &str,
    source: &TerminalSourceV1,
) -> Result<()> {
    if let Some(original) = &source.original_agent_id {
        let sessions = client.list_sessions().await?;
        let current = sessions.iter().find(|session| session.id == session_id)
            .context("The original agent Session is no longer present")?;
        ensure!(polar_station_workspace::SessionKind::from_terminal_name(&current.name)
            .is_some_and(|(_, id)| id == original), "The original agent Session no longer matches");
    }
    Ok(())
}


