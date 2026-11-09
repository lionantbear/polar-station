use super::session_lifecycle::SessionLifecycle;
pub(crate) use super::session_lifecycle::{SessionOperation, SessionOperationError};

use std::{future::Future, pin::Pin, sync::Arc};

use polar_tmux_core::TmuxErrorKind;
use polar_tmux_protocol::ExecutionTargetKind;
use polar_tmux_workspace::{
    ControlConnectionStatus, ExecutionTargetId, SessionDiscoveryStatus, SessionFact, SessionKey,
    SessionSection, SessionTargetFact, StableTmuxSessionId, TransportStatus,
};
use tracing::warn;

use crate::{
    ssh_connection_manager::{SshConnectionManager, SshConnectionStatus},
    target_registry::TargetRegistry,
};

pub(crate) type SessionFactFuture =
    Pin<Box<dyn Future<Output = SessionSectionFactSnapshot> + Send + 'static>>;
pub(crate) type SessionOperationFuture = Pin<
    Box<dyn Future<Output = Result<Option<SessionFact>, SessionOperationError>> + Send + 'static>,
>;

#[derive(Debug, Clone)]
pub(crate) struct PreparedSessionEnd {
    pub binding: polar_tmux_core::TmuxSessionBinding,
    pub preview: polar_tmux_protocol::SessionEndPreviewV1,
}
pub(crate) type SessionEndFuture = Pin<Box<dyn Future<Output = Result<PreparedSessionEnd, SessionOperationError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionFactSnapshot {
    pub facts: Vec<SessionFact>,
    pub targets: Vec<SessionTargetFact>,
    pub local_discovery: SessionDiscoveryStatus,
    pub remote_discovery: SessionDiscoveryStatus,
    pub connection_status: ControlConnectionStatus,
    pub connection_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSectionFactSnapshot {
    pub section: SessionSection,
    pub facts: Vec<SessionFact>,
    pub targets: Vec<SessionTargetFact>,
    pub discovery: SessionDiscoveryStatus,
    pub connection_status: Option<ControlConnectionStatus>,
    pub error_message: Option<String>,
}

impl SessionFactSnapshot {
    
}

/// Host port for privacy-minimal tmux session discovery.
///
/// Implementations may use registered local and remote targets, but the
/// returned facts deliberately exclude destinations, sockets, TTYs, PIDs,
/// current commands, and working directories.
pub(crate) trait SessionFactProvider: Send + Sync {
    fn initial_snapshot(&self) -> SessionFactSnapshot;
    fn request_refresh(&self) {}
    fn list_session_facts(&self, section: SessionSection) -> SessionFactFuture;
    fn prepare_session_end(&self, _session: SessionKey) -> SessionEndFuture {
        Box::pin(async { Err(SessionOperationError::failed()) })
    }
    fn apply_session_operation(&self, operation: SessionOperation) -> SessionOperationFuture;
}





#[derive(Clone)]
pub(crate) struct TargetRegistrySessionFactProvider {
    lifecycle: Option<Arc<SessionLifecycle>>,
    targets: TargetRegistry,
    ssh_connections: SshConnectionManager,
}

impl TargetRegistrySessionFactProvider {
    pub(crate) fn new(targets: TargetRegistry, ssh_connections: SshConnectionManager) -> Self {
        Self {
            targets,
            ssh_connections,
            lifecycle: None,
        }
    }

    pub(crate) fn with_lifecycle(mut self, lifecycle: Arc<SessionLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }
}

impl SessionFactProvider for TargetRegistrySessionFactProvider {
    fn request_refresh(&self) {
        if let Some(lifecycle) = &self.lifecycle {
            for descriptor in self.targets.descriptors() {
                if let Ok(target) = ExecutionTargetId::new(descriptor.id.to_string()) {
                    lifecycle.request_refresh(target);
                }
            }
        }
    }

    fn initial_snapshot(&self) -> SessionFactSnapshot {
        let targets = session_target_facts(&self.targets);
        let has_remote_target = targets
            .iter()
            .any(|target| target.section == SessionSection::Remote);
        SessionFactSnapshot {
            facts: Vec::new(),
            targets,
            local_discovery: SessionDiscoveryStatus::Loading,
            remote_discovery: if has_remote_target {
                SessionDiscoveryStatus::Loading
            } else {
                SessionDiscoveryStatus::Ready
            },
            connection_status: if has_remote_target {
                ControlConnectionStatus::Connecting
            } else {
                ControlConnectionStatus::Inactive
            },
            connection_error: None,
        }
    }

    fn list_session_facts(&self, requested_section: SessionSection) -> SessionFactFuture {
        let targets = self.targets.clone();
        let ssh_connections = self.ssh_connections.clone();
        let lifecycle = self.lifecycle.clone();
        Box::pin(async move {
            let mut facts = Vec::new();
            let mut remote_target_ids = Vec::new();
            let mut targets_facts = Vec::new();
            let mut failed = false;
            let mut error_message = None;
            for descriptor in targets.descriptors() {
                let target_id = descriptor.id.as_str().to_owned();
                let target_label = descriptor.display_name.clone();
                let section = if descriptor.kind == ExecutionTargetKind::Local {
                    SessionSection::Local
                } else {
                    SessionSection::Remote
                };
                if section != requested_section {
                    continue;
                }
                if section == SessionSection::Remote {
                    remote_target_ids.push(target_id.clone());
                }
                if let Ok(id) = ExecutionTargetId::new(target_id.clone()) {
                    targets_facts.push(SessionTargetFact {
                        id,
                        label: target_label.clone(),
                        section,
                    });
                }
                if let Some(lifecycle) = &lifecycle {
                    let (observed, available) = lifecycle.list_sessions(&target_id).await;
                    facts.extend(observed);
                    failed |= !available;
                    if !available {
                        error_message = Some(
                            "Session observation is unavailable; saved work is preserved."
                                .to_owned(),
                        );
                    }
                    continue;
                }
                let Some(target) = targets.get(&target_id) else {
                    failed = true;
                    error_message = Some("The execution target is unavailable.".to_owned());
                    continue;
                };
                let client = match target.managed_client(&ssh_connections).await {
                    Ok(client) => client,
                    Err(error) => {
                        failed = true;
                        error_message = Some(error.sanitized_message().to_owned());
                        warn!(
                            execution_target_id = %target_id,
                            %error,
                            "Control Center session discovery skipped unavailable target"
                        );
                        continue;
                    }
                };
                let sessions = match client.list_sessions().await {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        failed = true;
                        error_message = Some(session_discovery_error_message(&error));
                        warn!(
                            execution_target_id = %target_id,
                            %error,
                            "Control Center session discovery failed"
                        );
                        continue;
                    }
                };
                let Ok(execution_target_id) = ExecutionTargetId::new(target_id.clone()) else {
                    continue;
                };
                let clients = client.list_clients().await;
                for session in sessions {
                    let identity = session.id.clone();
                    let Ok(tmux_session_id) = StableTmuxSessionId::new(identity) else {
                        continue;
                    };
                    facts.push(SessionFact {
                        identity: SessionKey::new(execution_target_id.clone(), tmux_session_id),
                        session_kind: polar_tmux_workspace::SessionKind::from_terminal_name(&session.name).map_or(polar_tmux_workspace::SessionKind::Terminal, |(kind, _)| kind),
                        original_agent_id: polar_tmux_workspace::SessionKind::from_terminal_name(&session.name).map(|(_, id)| id.to_owned()),
                        display_name: session.name,
                        target_label: target_label.clone(),
                        section,
                        attached_clients: clients
                            .as_ref()
                            .map(|clients| {
                                clients
                                    .iter()
                                    .filter(|client| {
                                        client.session_id == session.id && !client.control_mode
                                    })
                                    .count() as u32
                            })
                            .unwrap_or_else(|_| u32::from(session.attached)),
                        transport_status: TransportStatus::Online,
                        recovery_status: None,
                        replacement: None,
                    });
                }
            }
            if let Some(lifecycle) = &lifecycle {
                for record in lifecycle.snapshot().await.sessions {
                    let section = if record.execution_target_id.as_str() == "local" {
                        SessionSection::Local
                    } else {
                        SessionSection::Remote
                    };
                    if section != requested_section
                        || record.status == polar_tmux_workspace::SessionRecoveryStatus::Ended
                    {
                        continue;
                    }
                    let identity = SessionKey::new(
                        record.execution_target_id.clone(),
                        StableTmuxSessionId::new(record.reference()).expect("managed reference"),
                    );
                    if facts.iter().any(|fact| fact.identity == identity) {
                        continue;
                    }
                    facts.push(SessionFact {
                        identity,
                        session_kind: polar_tmux_workspace::SessionKind::from_restore_spec(&record.launch.process),
                        original_agent_id: record.launch.process.original_agent_id().map(str::to_owned),
                        display_name: record.launch.session_name,
                        target_label: record.execution_target_id.to_string(),
                        section,
                        attached_clients: 0,
                        transport_status: TransportStatus::Offline,
                        recovery_status: None,
                        replacement: None,
                    });
                }
            }
            facts.sort_by(|left, right| {
                left.section
                    .cmp(&right.section)
                    .then_with(|| left.target_label.cmp(&right.target_label))
                    .then_with(|| {
                        left.identity
                            .execution_target_id
                            .cmp(&right.identity.execution_target_id)
                    })
                    .then_with(|| left.display_name.cmp(&right.display_name))
                    .then_with(|| {
                        left.identity
                            .tmux_session_id
                            .cmp(&right.identity.tmux_session_id)
                    })
            });
            let connection_status = (requested_section == SessionSection::Remote).then(|| {
                if remote_target_ids.is_empty() {
                    ControlConnectionStatus::Inactive
                } else {
                    match ssh_connections
                        .aggregate_status(remote_target_ids.iter().map(String::as_str))
                    {
                        SshConnectionStatus::Connecting => ControlConnectionStatus::Connecting,
                        SshConnectionStatus::Connected => ControlConnectionStatus::Connected,
                        SshConnectionStatus::Recovering => ControlConnectionStatus::Recovering,
                        SshConnectionStatus::Offline => ControlConnectionStatus::Offline,
                    }
                }
            });
            SessionSectionFactSnapshot {
                section: requested_section,
                facts,
                targets: targets_facts,
                discovery: if failed {
                    SessionDiscoveryStatus::Failed
                } else {
                    SessionDiscoveryStatus::Ready
                },
                connection_status,
                error_message,
            }
        })
    }

    fn prepare_session_end(&self, session: SessionKey) -> SessionEndFuture {
        let targets = self.targets.clone(); let ssh = self.ssh_connections.clone(); let lifecycle = self.lifecycle.clone();
        Box::pin(async move {
            let target = targets.get(session.execution_target_id.as_str()).ok_or_else(SessionOperationError::target_not_found)?;
            let gate = target.operation_gate(); let _guard = gate.read().await;
            let client = target.managed_client(&ssh).await.map_err(|_| SessionOperationError::failed())?;
            let lifecycle = lifecycle.ok_or_else(SessionOperationError::failed)?;
            lifecycle.prepare_end(&client, session.execution_target_id.as_str(), session.tmux_session_id.as_str(), &target.descriptor().display_name)
                .await.map_err(|_| SessionOperationError::failed())
        })
    }

    fn apply_session_operation(&self, operation: SessionOperation) -> SessionOperationFuture {
        let targets = self.targets.clone();
        let ssh_connections = self.ssh_connections.clone();
        let lifecycle = self.lifecycle.clone();
        Box::pin(async move {
            if let Some(lifecycle) = lifecycle {
                return lifecycle
                    .apply_operation(operation, &targets, &ssh_connections)
                    .await;
            }

            let target_id = match &operation {
                SessionOperation::QuickCreate {
                    execution_target_id,
                    ..
                } => execution_target_id,
                SessionOperation::Create {
                    execution_target_id,
                    ..
                } | SessionOperation::CreateClaude {
                    execution_target_id,
                    ..
                } => execution_target_id,
                SessionOperation::UseExisting { session, .. }
                | SessionOperation::Restart { session }
                | SessionOperation::Rename { session, .. }
                | SessionOperation::Kill { session, .. } => &session.execution_target_id,
            }
            .clone();
            let target = targets
                .get(target_id.as_str())
                .ok_or_else(SessionOperationError::target_not_found)?;
            let client = target
                .managed_client(&ssh_connections)
                .await
                .map_err(|error| {
                    warn!(
                        execution_target_id = %target_id,
                        %error,
                        "Control Center Session operation could not acquire managed target"
                    );
                    SessionOperationError::failed()
                })?;
            let gate = target.operation_gate();
            let _operation = gate.write().await;
            let result = match operation {
                SessionOperation::QuickCreate { .. }
                | SessionOperation::CreateClaude { .. }
                | SessionOperation::UseExisting { .. }
                | SessionOperation::Restart { .. } => return Err(SessionOperationError::missing()),
                SessionOperation::Create {
                    name,
                    working_directory,
                    command,
                    ..
                } => client
                    .create_session(&name, working_directory.as_deref(), command.as_deref())
                    .await
                    .map(|_| ()),
                SessionOperation::Rename { session, name } => client
                    .rename_session(session.tmux_session_id.as_str(), &name)
                    .await
                    .map(|_| ()),
                SessionOperation::Kill { session, binding } => {
                    if let Some(binding) = binding {
                        client.kill_bound_session(&binding).await
                    } else {
                        // Explicit callers without a preview still target one exact physical ID.
                        client.kill_session(session.tmux_session_id.as_str()).await
                    }
                }
            };
            result.map(|()| None).map_err(|error| {
                warn!(
                    execution_target_id = %target_id,
                    %error,
                    "Control Center Session operation failed"
                );
                SessionOperationError::from_tmux(&error)
            })
        })
    }
}



fn session_discovery_error_message(error: &polar_tmux_core::TmuxError) -> String {
    match error.kind() {
        TmuxErrorKind::ExecutableMissing => "tmux is unavailable on the local host.".to_owned(),
        TmuxErrorKind::NoServer => "The tmux server is not running.".to_owned(),
        TmuxErrorKind::TargetNotFound => "The tmux target was not found.".to_owned(),
        TmuxErrorKind::UnsupportedTarget => "The execution target is unsupported.".to_owned(),
        TmuxErrorKind::TargetUnavailable | TmuxErrorKind::SshMasterUnavailable => {
            "The SSH target is unavailable.".to_owned()
        }
        TmuxErrorKind::Timeout => "Session discovery timed out.".to_owned(),
        TmuxErrorKind::CommandFailed | TmuxErrorKind::SessionAlreadyExists => {
            "Session discovery failed.".to_owned()
        }
        TmuxErrorKind::ParseFailed => "Session data could not be read.".to_owned(),
        TmuxErrorKind::InvalidInput => "Session discovery received invalid input.".to_owned(),
    }
}









fn session_target_facts(targets: &TargetRegistry) -> Vec<SessionTargetFact> {
    targets
        .descriptors()
        .into_iter()
        .filter_map(|descriptor| {
            Some(SessionTargetFact {
                id: ExecutionTargetId::new(descriptor.id.as_str()).ok()?,
                label: descriptor.display_name,
                section: if descriptor.kind == ExecutionTargetKind::Local {
                    SessionSection::Local
                } else {
                    SessionSection::Remote
                },
            })
        })
        .collect()
}


