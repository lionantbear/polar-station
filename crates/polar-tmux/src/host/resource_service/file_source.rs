//! Source identity only. No text parsing, candidate probing or Resource changes.
use super::*;
use polar_tmux_resources::{
    FileOpenFailure, FileOpenStage, FileOrigin, FileReferenceInput, PathFlavor, ResolvedFileSource,
};

impl ResourceService {
    pub(super) async fn resolve_open_source(
        &self,
        mut origin: FileOrigin,
        input: &FileReferenceInput,
        codex: Option<&super::super::codex_control::CodexControlService>,
        context: Option<SessionFileContext<'_>>,
    ) -> Result<ResolvedFileSource, FileOpenFailure> {
        let unavailable = || {
            FileOpenFailure::new(
                FileOpenStage::Source,
                "source_unavailable",
                "The original source Session is unavailable.",
            )
        };
        let absolute = polar_tmux_resources::parse_file_input(input, PathFlavor::Posix)
            .is_ok_and(|refs| refs.iter().all(|r| r.path.starts_with('/')));
        origin = self
            .session_file_origin(origin, if absolute { "/" } else { "./" }, codex, context)
            .await?
            .0;
        if !self
            .current_targets()
            .contains_key(&origin.execution_target_id)
        {
            return Err(unavailable());
        }
        origin.terminal_session_id = None;
        let home = if input.text().contains("~/") || input.text().trim() == "~" {
            if origin.execution_target_id == crate::target_registry::LOCAL_TARGET_ID {
                std::env::var("HOME").ok()
            } else {
                let registry = self.target_registry.as_ref().ok_or_else(unavailable)?;
                let target = registry
                    .get(&origin.execution_target_id)
                    .ok_or_else(unavailable)?;
                let gate = target.operation_gate();
                let _guard = gate.read().await;
                let ssh = self.candidate_ssh.as_ref().ok_or_else(unavailable)?;
                let lease = target
                    .resource_ssh_lease(ssh)
                    .await
                    .map_err(|_| unavailable())?;
                let transport = if let Some(lease) = &lease {
                    lease.tmux_target().map_err(|_| unavailable())?
                } else {
                    target.configured_target().clone()
                };
                if let Some(lease) = &lease {
                    ssh.validate(lease).await.map_err(|_| unavailable())?;
                }
                let facts = super::super::target_directory::read(
                    &transport,
                    &origin.execution_target_id,
                    None,
                    false,
                )
                .await
                .map_err(|_| unavailable())?;
                if let Some(lease) = &lease {
                    ssh.validate(lease).await.map_err(|_| unavailable())?;
                }
                Some(facts.home)
            }
        } else {
            None
        };
        // These existing local/tmux/SSH adapters are POSIX adapters. Windows
        // grammar support is independent of an unavailable Windows provider.
        Ok(ResolvedFileSource {
            target_configuration_revision: self.file_source_revision(&origin.execution_target_id),
            origin,
            flavor: PathFlavor::Posix,
            home,
        })
    }
}

impl ResourceService {
    /// Shared identity/cwd adapter for both the current flow and legacy wires.
    pub(super) async fn session_file_origin(
        &self,
        mut origin: FileOrigin,
        path: &str,
        codex: Option<&super::super::codex_control::CodexControlService>,
        context: Option<SessionFileContext<'_>>,
    ) -> Result<(FileOrigin, String), FileOpenFailure> {
        let Some(session) = origin.terminal_session_id.clone() else {
            return Ok((origin, path.into()));
        };
        let invalid = || {
            FileOpenFailure::new(
                FileOpenStage::Source,
                "source_unavailable",
                "The original source Session is unavailable.",
            )
        };
        let key = polar_tmux_workspace::SessionKey::new(
            polar_tmux_workspace::ExecutionTargetId::new(origin.execution_target_id.clone())
                .map_err(|_| invalid())?,
            polar_tmux_workspace::StableTmuxSessionId::new(session.clone())
                .map_err(|_| invalid())?,
        );
        if let Some(codex) = codex {
            if let Some(value) = codex
                .terminal_file_reference(&key, path)
                .await
                .map_err(|e| FileOpenFailure::new(FileOpenStage::Source, &e.code, e.message))?
            {
                return Ok(value);
            }
        }
        origin.cwd = if path.starts_with('/') {
            None
        } else {
            Some(
                context
                    .ok_or_else(invalid)?
                    .directory(&origin.execution_target_id, &session)
                    .await
                    .map_err(|_| invalid())?,
            )
        };
        Ok((origin, path.into()))
    }
}
