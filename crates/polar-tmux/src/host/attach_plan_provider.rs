use std::{future::Future, pin::Pin};

use polar_tmux_core::TmuxErrorKind;
use polar_tmux_protocol::TmuxAttachPlan;
use polar_tmux_workspace::{WorkspaceError, WorkspaceErrorCode};

use crate::{ssh_connection_manager::SshConnectionManager, target_registry::TargetRegistry};

pub(crate) type AttachPlanFuture =
    Pin<Box<dyn Future<Output = Result<TmuxAttachPlan, WorkspaceError>> + Send>>;

/// Host port for resolving one validated terminal attachment.
///
/// The current implementation uses the registered tmux target. Phase 5B.4
/// replaces its remote transport internals with the shared SSH manager without
/// changing the Workspace or frontend presentation contracts.
pub(crate) trait AttachPlanProvider: Send + Sync {
    fn prepare_attach(
        &self,
        execution_target_id: String,
        tmux_session_id: String,
    ) -> AttachPlanFuture;
}

#[derive(Clone)]
pub(crate) struct TargetRegistryAttachPlanProvider {
    targets: TargetRegistry,
    ssh_connections: SshConnectionManager,
}

impl TargetRegistryAttachPlanProvider {
    pub(crate) fn new(targets: TargetRegistry, ssh_connections: SshConnectionManager) -> Self {
        Self {
            targets,
            ssh_connections,
        }
    }
}

impl AttachPlanProvider for TargetRegistryAttachPlanProvider {
    fn prepare_attach(
        &self,
        execution_target_id: String,
        tmux_session_id: String,
    ) -> AttachPlanFuture {
        let target = self.targets.get(&execution_target_id);
        let ssh_connections = self.ssh_connections.clone();
        Box::pin(async move {
            let target = target.ok_or_else(|| {
                WorkspaceError::new(
                    WorkspaceErrorCode::RuntimeMissing,
                    "the Workspace execution target is not registered on this Host",
                )
            })?;
            target
                .managed_client(&ssh_connections)
                .await
                .map_err(|_| {
                    WorkspaceError::new(
                        WorkspaceErrorCode::RuntimeMissing,
                        "the shared SSH master is unavailable for attachment",
                    )
                })?
                .prepare_attach(&tmux_session_id)
                .await
                .map_err(|error| workspace_attach_error(error.kind()))
        })
    }
}

fn workspace_attach_error(kind: TmuxErrorKind) -> WorkspaceError {
    let message = match kind {
        TmuxErrorKind::InvalidInput => "the Workspace tmux attachment request is invalid",
        TmuxErrorKind::ExecutableMissing => "tmux is unavailable on the Workspace execution target",
        TmuxErrorKind::NoServer => "no tmux server is running on the Workspace execution target",
        TmuxErrorKind::TargetNotFound => "the Workspace tmux session is no longer available",
        TmuxErrorKind::UnsupportedTarget => {
            "the Workspace execution target does not support tmux attachment"
        }
        TmuxErrorKind::TargetUnavailable => "the Workspace execution target is unavailable",
        TmuxErrorKind::SshMasterUnavailable => {
            "the shared SSH connection is unavailable for attachment"
        }
        TmuxErrorKind::CommandFailed | TmuxErrorKind::SessionAlreadyExists => {
            "the Workspace execution target rejected the tmux attachment check"
        }
        TmuxErrorKind::ParseFailed => {
            "the Workspace execution target returned an unsupported tmux response"
        }
        TmuxErrorKind::Timeout => "the Workspace tmux attachment check timed out",
    };
    WorkspaceError::new(WorkspaceErrorCode::RuntimeMissing, message)
}






