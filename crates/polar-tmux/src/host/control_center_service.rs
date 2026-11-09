//! Session/target control shell. Native layout never enters this service.
mod editor;
use super::{
    application_state::ApplicationStatePublisher,
    control_center_wire::{wire_action, wire_session_library},
    session_fact_provider::{
        SessionFactProvider, SessionFactSnapshot, SessionOperation, SessionSectionFactSnapshot,
    },
    session_lifecycle::SessionLifecycle,
    session_registry::SessionRegistry,
};
use crate::{
    ssh_connection_manager::{SshConnectionManager, SshRouteKind},
    target_registry::{TargetRegistry, WorkspaceSshTarget},
};
use polar_tmux_protocol::*;
use polar_tmux_workspace::{
    badge_for, project_session_library, ControlActionProjection, ControlActionRole,
    ControlConnectionStatus, ExecutionTargetId, SessionDiscoveryStatus, SessionFact, SessionKey,
    SessionLibraryContext, SessionSection, SessionTargetFact, StableTmuxSessionId,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use tracing::Instrument;
#[derive(Debug, Default)]
struct SessionFactCache {
    revision: u64,
    facts: Vec<SessionFact>,
    targets: Vec<SessionTargetFact>,
    local_discovery: SessionDiscoveryStatus,
    remote_discovery: SessionDiscoveryStatus,
    connection_status: ControlConnectionStatus,
    connection_error: Option<String>,
}

impl SessionFactCache {
    fn snapshot(&self) -> SessionFactSnapshot {
        SessionFactSnapshot {
            facts: self.facts.clone(),
            targets: self.targets.clone(),
            local_discovery: self.local_discovery,
            remote_discovery: self.remote_discovery,
            connection_status: self.connection_status,
            connection_error: self.connection_error.clone(),
        }
    }
}

struct ConnectionDependencies {
    resources: Arc<super::resource_service::ResourceService>,
    settings: polar_station_settings::SettingsRepository,
    gateway: Option<Arc<crate::polar_gateway::PolarGateway>>,
}

pub(crate) struct ControlCenterService {
    #[cfg(feature = "codex-control")]
    pub(crate) codex: std::sync::OnceLock<Arc<super::codex_control::CodexControlService>>,
    connection_dependencies: std::sync::OnceLock<ConnectionDependencies>,
    lifecycle: Arc<SessionLifecycle>,
    revisions: Mutex<HashMap<(String, String), u64>>,
    session_fact_provider: Arc<dyn SessionFactProvider>,
    session_facts: Arc<Mutex<SessionFactCache>>,
    session_registry: SessionRegistry,
    local_projection_refresh: Arc<StdMutex<(bool, bool)>>,
    remote_projection_refresh: Arc<StdMutex<(bool, bool)>>,
    state_events: ApplicationStatePublisher,
    targets: TargetRegistry,
    ssh_connections: SshConnectionManager,
}
impl ControlCenterService {
    pub(crate) fn new_with_runtime_ports_events_and_targets(
        lifecycle: Arc<SessionLifecycle>,
        session_fact_provider: Arc<dyn SessionFactProvider>,
        state_events: ApplicationStatePublisher,
        targets: TargetRegistry,
        ssh_connections: SshConnectionManager,
        session_registry: SessionRegistry,
    ) -> Self {
        let initial = session_fact_provider.initial_snapshot();
        Self {
            #[cfg(feature = "codex-control")]
            codex: std::sync::OnceLock::new(),
            connection_dependencies: std::sync::OnceLock::new(),
            lifecycle,
            revisions: Mutex::default(),
            session_fact_provider,
            session_facts: Arc::new(Mutex::new(SessionFactCache {
                revision: 0,
                facts: initial.facts,
                targets: initial.targets,
                local_discovery: initial.local_discovery,
                remote_discovery: initial.remote_discovery,
                connection_status: initial.connection_status,
                connection_error: initial.connection_error,
            })),
            session_registry,
            local_projection_refresh: Arc::default(),
            remote_projection_refresh: Arc::default(),
            state_events,
            targets,
            ssh_connections,
        }
    }
    pub(crate) fn attach_connection_dependencies(&self, resources: Arc<super::resource_service::ResourceService>, settings: polar_station_settings::SettingsRepository, gateway: Option<Arc<crate::polar_gateway::PolarGateway>>) {
        assert!(self.connection_dependencies.set(ConnectionDependencies { resources, settings, gateway }).is_ok());
    }

    async fn check_connection(&self, id: &str, polar: bool, request_id: &str) -> ConnectionCheckV1 {
        let checked = tokio::time::timeout(std::time::Duration::from_secs(12), async {
            if polar {
                let gateway = self.connection_dependencies.get().and_then(|d| d.gateway.as_ref())
                    .ok_or_else(|| "Polar access is unavailable on this Host.".to_owned())?;
                gateway.check(id).await.map_err(|e| format!("{}: {}", e.code, e.message))?;
                return Ok("Polar socket, pinned identity and Vault chat authorization verified. No message was sent.".to_owned());
            }
            let target = self.targets.get(id).ok_or_else(|| "Machine is no longer configured.".to_owned())?;
            let client = target.managed_client(&self.ssh_connections).await
                .map_err(|_| "SSH connection failed. Check the destination, host key and authenticated account.".to_owned())?;
            let transport = if id == "local" { "Local machine" } else { "SSH connected" };
            match client.availability().await {
                Ok(polar_tmux_core::TmuxAvailability::Available { version }) => Ok(format!("{transport}; {version}. Terminal Sessions are available. Polar chat is checked separately.")),
                Ok(polar_tmux_core::TmuxAvailability::ExecutableMissing { .. }) => Err(format!("{transport}; tmux is not installed. Install tmux to use terminal Sessions. Polar chat does not require tmux.")),
                Err(_) => Err(format!("{transport}; tmux could not be queried. Check its installation and remote PATH. Polar chat is checked separately.")),
            }
        }).await;
        let (ready, summary) = match checked {
            Ok(Ok(message)) => (true, message),
            Ok(Err(message)) => (false, message),
            Err(_) => (false, "Connection check timed out. No configuration or Pane binding was changed.".into()),
        };
        ConnectionCheckV1 { request_id: request_id.into(), target_id: id.into(), polar, ready, summary }
    }

    async fn check_connection_references(&self, change: &ConnectionChangeV1) -> Result<(), ControlCenterErrorV1> {
        let current = self.targets.connections();
        let (id, machine) = match change {
            ConnectionChangeV1::RemoveMachine { id } => (id, true),
            ConnectionChangeV1::RemovePolar { id } => (id, false),
            ConnectionChangeV1::SaveMachine { machine } if current.machines.iter().any(|old| old.id == machine.id && (old.destination != machine.destination || old.route_profile != machine.route_profile)) => (&machine.id, true),
            ConnectionChangeV1::SavePolar { polar } if current.polar_instances.iter().any(|old| old.id == polar.id && (old.execution_target_id != polar.execution_target_id || old.socket_path != polar.socket_path || old.expected_node_id != polar.expected_node_id || old.expected_peer_uid != polar.expected_peer_uid)) => (&polar.id, false),
            _ => return Ok(()),
        };
        let mut references = Vec::new();
        if machine {
            references.extend(self.lifecycle.snapshot().await.sessions.iter().filter(|s| s.execution_target_id.as_str() == id).map(|s| format!("Saved Session: {}", s.launch.session_name)));
            references.extend(self.session_registry.snapshot().await.sessions.iter().filter(|s| s.key.execution_target.as_str() == id).map(|s| format!("Session: {}", s.display_name)));
        }
        if let Some(deps) = self.connection_dependencies.get() {
            if machine {
                references.extend(deps.resources.connection_references(id).await);
                let config = deps.settings.load_codex_control().map_err(|_| control_error("references_unavailable", "Cannot verify Codex references; no connection was changed."))?;
                references.extend(config.endpoints.iter().filter(|e| e.execution_target_id == *id).map(|e| format!("Codex endpoint: {}", e.id)));
            }
            if let Some(gateway) = &deps.gateway { references.extend(gateway.references(id, machine).await); }
        }
        references.sort(); references.dedup();
        if !references.is_empty() {
            return Err(ControlCenterErrorV1 { resume: None, code: "connection_in_use".into(), message: format!("Connection is in use. Close or reassign these references first: {}", references.join("; ")) });
        }
        Ok(())
    }

    
    
    
    
    
    pub(crate) async fn remove_principal(&self, principal: &str) {
        self.revisions
            .lock()
            .await
            .retain(|(owner, _), _| owner != principal);
    }
    pub(crate) async fn dispatch(
        &self,
        principal: &str,
        request: ControlCenterRequestEnvelopeV1,
    ) -> ControlCenterResponseEnvelopeV1 {
        use sha2::{Digest, Sha256};
        if request.validate().is_ok() {
            if let ControlCenterCommandV1::SubmitIntent { intent: ControlCenterIntentV1::CreateAgentSession { session_ids, .. } } = &request.command {
                if !session_ids.is_empty() {
                    let key = (principal.to_owned(), request.frontend_session_id.clone());
                    let revision = *self.revisions.lock().await.entry(key.clone()).or_insert(0);
                    if request.expected_control_revision != Some(revision) {
                        return error_response(&request, revision, "control_projection_stale",
                            "Session controls changed. Review the refreshed state and try again.", Some(self.projection(revision).await));
                    }
                    let mut items = Vec::new();
                    for thread_id in session_ids {
                        let mut child = request.clone();
                        child.request_id = format!("batch-{:x}-{}", Sha256::digest(request.request_id.as_bytes()), items.len());
                        child.expected_control_revision = Some(*self.revisions.lock().await.get(&key).unwrap_or(&revision));
                        if let ControlCenterCommandV1::SubmitIntent { intent: ControlCenterIntentV1::CreateAgentSession { session_id, session_ids, .. } } = &mut child.command {
                            *session_id = Some(thread_id.clone()); session_ids.clear();
                        }
                        // Each item uses the existing durable operation receipt.
                        // No recursive dispatcher/future is constructed for batches.
                        let result = self.dispatch_single(principal, child).await;
                        let (session, error) = match result.result {
                            ControlCenterResultV1::SessionCreated { session, .. } => (Some(session), None),
                            ControlCenterResultV1::Error { error, .. } => (None, Some(error)),
                            _ => (None, Some(ControlCenterErrorV1 { resume: None, code: "session_operation_unknown".into(), message: "Refresh this conversation before retrying.".into() })),
                        };
                        items.push(polar_tmux_protocol::SessionCreationItemV1 { thread_id: thread_id.clone(), session, error });
                    }
                    let revision = *self.revisions.lock().await.get(&key).unwrap_or(&revision);
                    return ControlCenterResponseEnvelopeV1 { protocol_version: PROTOCOL_VERSION, api: ControlCenterApiV1::ControlCenterV1,
                        request_id: request.request_id, control_revision: revision,
                        result: ControlCenterResultV1::SessionBatchCreated { projection: self.projection(revision).await, items } };
                }
            }
        }
        self.dispatch_single(principal, request).await
    }

    async fn dispatch_single(&self, principal: &str, request: ControlCenterRequestEnvelopeV1) -> ControlCenterResponseEnvelopeV1 {
        use super::session_lifecycle::OperationAdmission;
        use sha2::{Digest, Sha256};
        let create = matches!(
            &request.command,
            ControlCenterCommandV1::SubmitIntent {
                intent: ControlCenterIntentV1::CreateSession { .. }
                    | ControlCenterIntentV1::CreateCodexSession { .. }
                    | ControlCenterIntentV1::CreateAgentSession { .. }
                    | ControlCenterIntentV1::QuickCreateSession { .. }
            }
        );
        let restart = matches!(
            &request.command,
            ControlCenterCommandV1::SubmitIntent {
                intent: ControlCenterIntentV1::RestartSession { .. }
            }
        );
        let kill = matches!(&request.command, ControlCenterCommandV1::SubmitIntent { intent: ControlCenterIntentV1::KillSession { .. } | ControlCenterIntentV1::EndAgentSession { .. } });
        let retire = matches!(
            &request.command,
            ControlCenterCommandV1::SubmitIntent {
                intent: ControlCenterIntentV1::UseExistingSession { retire: true, .. }
            }
        );
        if !(create || restart || retire || kill) || request.validate().is_err() {
            return self.dispatch_uncached(principal, request).await;
        }
        let key =
            serde_json::to_string(&(principal, &request.frontend_session_id, &request.request_id))
                .expect("string tuple");
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&request.command).expect("typed intent"))
        );
        let admission = self
            .lifecycle
            .begin_operation(key.clone(), fingerprint)
            .await;
        match admission {
            Ok(OperationAdmission::Started) => {}
            Ok(OperationAdmission::Completed(response)) => {
                return serde_json::from_str(&response).unwrap_or_else(|_| {
                    error_response(
                        &request,
                        request.expected_control_revision.unwrap_or_default(),
                        "session_receipt_invalid",
                        "The saved result cannot be read. Refresh Sessions before retrying.",
                        None,
                    )
                });
            }
            other => {
                let (code, message) = match other {
                    Ok(OperationAdmission::Conflict) => (
                        "request_id_conflict",
                        "This operation ID belongs to another Session request.",
                    ),
                    Ok(OperationAdmission::Unknown) => (
                        "session_operation_unknown",
                        "This operation is in progress or its result is unknown. Refresh Sessions before retrying.",
                    ),
                    _ => (
                        "session_receipt_unavailable",
                        "The operation receipt could not be saved; no change was started.",
                    ),
                };
                return error_response(
                    &request,
                    request.expected_control_revision.unwrap_or_default(),
                    code,
                    message,
                    None,
                );
            }
        }
        let response = self.dispatch_uncached(principal, request.clone()).await;
        if self
            .lifecycle
            .complete_operation(
                &key,
                serde_json::to_string(&response).expect("typed response"),
            )
            .await
            .is_err()
        {
            return error_response(
                &request,
                response.control_revision,
                "session_operation_unknown",
                "The operation result could not be saved. Refresh Sessions before retrying.",
                None,
            );
        }
        response
    }

    async fn dispatch_uncached(
        &self,
        principal: &str,
        request: ControlCenterRequestEnvelopeV1,
    ) -> ControlCenterResponseEnvelopeV1 {
        let key = (principal.to_owned(), request.frontend_session_id.clone());
        let revision = *self.revisions.lock().await.entry(key.clone()).or_insert(0);
        if request.validate().is_err() {
            return error_response(
                &request,
                revision,
                "control_intent_invalid",
                "Invalid Session control request.",
                None,
            );
        }
        if let ControlCenterCommandV1::SessionEditor { request: editor } = request.command.clone() {
            return match self.session_editor(editor).await {
                Ok(result) => ControlCenterResponseEnvelopeV1 { protocol_version: PROTOCOL_VERSION, api: ControlCenterApiV1::ControlCenterV1,
                    request_id: request.request_id, control_revision: revision, result: ControlCenterResultV1::SessionEditor { result } },
                Err(error) => error_response(&request, revision, &error.code, error.message, None),
            };
        }
        if let ControlCenterCommandV1::SubmitIntent { intent } = request.command.clone() {
            if request.expected_control_revision != Some(revision) {
                return error_response(
                    &request,
                    revision,
                    "control_projection_stale",
                    "Session controls changed. Review the refreshed state and try again.",
                    Some(self.projection(revision).await),
                );
            }
            let quick_create = matches!(&intent, ControlCenterIntentV1::QuickCreateSession { .. });
            let creating =
                quick_create || matches!(&intent, ControlCenterIntentV1::CreateSession { .. } | ControlCenterIntentV1::CreateCodexSession { .. } | ControlCenterIntentV1::CreateAgentSession { .. });
            if creating {
                tracing::info!(target: "polar_station_operation", component="Sessions", event="session_create_request", request_id=request.request_id.as_str());
            }
            let check = if let ControlCenterIntentV1::CheckConnection { target_id, polar } = &intent {
                Some(self.check_connection(target_id, *polar, &request.request_id).await)
            } else { None };
            let created = match if check.is_some() { Ok(None) } else { self.apply_intent(principal, &request.request_id, intent).await } {
                Ok(created) => created,
                Err(error) => {
                    return ControlCenterResponseEnvelopeV1 {
                        protocol_version: PROTOCOL_VERSION, api: ControlCenterApiV1::ControlCenterV1,
                        request_id: request.request_id.clone(), control_revision: revision,
                        result: ControlCenterResultV1::Error { error, projection: Some(self.projection(revision).await) },
                    };
                }
            };
            let revision = {
                let mut revisions = self.revisions.lock().await;
                let current = revisions.entry(key).or_insert(revision);
                *current = current.saturating_add(1);
                *current
            };
            let mut projection = self.projection(revision).await;
            projection.connections.last_check = check;
            let mut response = accepted_response(&request, revision, projection);
            if let Some(fact) = created.filter(|_| creating) {
                let session = SessionIdentityV1 {
                    execution_target_id: fact.identity.execution_target_id.to_string(),
                    tmux_session_id: fact.identity.tmux_session_id.to_string(),
                };
                let ControlCenterResultV1::IntentAccepted { projection } = response.result else {
                    unreachable!()
                };
                response.result = ControlCenterResultV1::SessionCreated {
                    projection,
                    session,
                };
            }
            if creating {
                tracing::info!(target: "polar_station_operation", component="Sessions", event="session_create_response", request_id=request.request_id.as_str(), revision);
            }
            response
        } else {
            projection_response(&request, revision, self.projection(revision).await)
        }
    }

    async fn apply_intent(
        &self,
        principal: &str,
        request_id: &str,
        intent: ControlCenterIntentV1,
    ) -> Result<Option<SessionFact>, ControlCenterErrorV1> {
        use ControlCenterIntentV1::*;
        let codex_args=match &intent {
            CreateCodexSession{execution_target_id,endpoint_id,name,working_directory,yolo}=>Some((execution_target_id.clone(),Some(endpoint_id.clone()),Some(name.clone()),working_directory.clone(),None,*yolo)),
            QuickCreateSession{execution_target_id,source_session_id,kind:QuickSessionKindV1::Codex}=>Some((execution_target_id.clone(),None,None,None,source_session_id.clone(),false)),
            _=>None,
        };
        if let ControlCenterIntentV1::CreateAgentSession {
            execution_target_id, provider: polar_tmux_protocol::AgentProviderV1::Codex,
            mode, name, working_directory, endpoint_id: Some(endpoint_id), session_id, yolo, open_local_cli, open_terminal, ..
        } = &intent {
            #[cfg(feature = "codex-control")]
            {
                let service = self.codex.get().ok_or_else(||control_error("codex_unavailable","Codex control is unavailable."))?;
                if *open_terminal && !*open_local_cli {
                    let start = if *mode == polar_tmux_protocol::AgentStartModeV1::Resume {
                        super::codex_session::CodexTerminalStart::Resume { thread_id: session_id.clone().ok_or_else(|| control_error("thread_required", "Select a conversation to resume."))? }
                    } else { super::codex_session::CodexTerminalStart::New { yolo: *yolo } };
                    let fact = service.create_terminal(execution_target_id.clone(), Some(endpoint_id.clone()),
                        if *mode == polar_tmux_protocol::AgentStartModeV1::New { Some(name.clone()) } else { None },
                        working_directory.clone(), None, start).await?;
                    return self.commit_created(Some(fact)).await;
                }
                if *open_local_cli { service.preflight_local_cli_plane(execution_target_id, endpoint_id).await?; }
                let binding = service.open_agent_thread(execution_target_id, endpoint_id, name.clone(), working_directory.clone(),
                    if *mode == polar_tmux_protocol::AgentStartModeV1::Resume { session_id.clone() } else { None }, *yolo).await?;
                if *open_local_cli {
                    let plane = service.create_local_cli_plane(execution_target_id, endpoint_id, &binding).await.map_err(|mut failure| {
                        failure.resume = Some(AgentSessionResumeV1 { execution_target_id: execution_target_id.clone(), endpoint_id: endpoint_id.clone(), thread_id: binding.thread_id.clone() }); failure
                    })?;
                    return self.commit_created(Some(plane)).await;
                }
                self.schedule_projection_refresh("agent_thread_changed");
                return Ok(None);
            }
            #[cfg(not(feature = "codex-control"))]
            {
                let _ = (execution_target_id, mode, name, working_directory, endpoint_id, session_id, yolo, open_local_cli, open_terminal);
                return Err(control_error("codex_unsupported","This Host was built without Codex control."));
            }
        }
        if let Some((target,endpoint,name,directory,source,yolo))=codex_args {
            #[cfg(feature = "codex-control")]
            {
                let service=self.codex.get().ok_or_else(||control_error("codex_unavailable","Codex control is unavailable."))?;
                let fact=service.create_terminal(target,endpoint,name,directory,source,super::codex_session::CodexTerminalStart::New { yolo }).await?;
                return self.commit_created(Some(fact)).await;
            }
            #[cfg(not(feature = "codex-control"))]
            {
                let _=(target,endpoint,name,directory,source,yolo);
                return Err(control_error("codex_unsupported","This Host was built without Codex control."));
            }
        }
        let operation = match intent {
            CreateCodexSession { .. } | CheckConnection { .. } => unreachable!("handled above"),
            CreateAgentSession { execution_target_id, provider, mode, name, working_directory, session_id, .. } => {
                if provider != polar_tmux_protocol::AgentProviderV1::Claude {
                    return Err(control_error("agent_provider_invalid","The selected agent provider is unavailable."));
                }
                SessionOperation::CreateClaude {
                    execution_target_id: target_id(&execution_target_id)?,
                    name,
                    working_directory,
                    resume_id: if mode == polar_tmux_protocol::AgentStartModeV1::Resume { session_id } else { None },
                }
            }
            UpdateConnection { expected_revision, change } => {
                self.check_connection_references(&change).await?;
                let machine_id = match &change { ConnectionChangeV1::SaveMachine { machine } if self.targets.connections().machines.iter().find(|m| m.id == machine.id).is_none_or(|old| old.destination != machine.destination || old.route_profile != machine.route_profile) => Some(machine.id.clone()), _ => None };
                let targets = self.targets.clone();
                tokio::task::spawn_blocking(move || targets.update_connection(&expected_revision, change)).await
                    .map_err(|_| control_error("target_update_failed", "Connection update did not complete."))?
                    .map_err(|error| ControlCenterErrorV1 { resume: None, code: "target_update_failed".into(), message: error.to_string() })?;
                if let Some(target) = machine_id.and_then(|id| self.targets.ssh_targets().into_iter().find(|t| t.target_id == id)) { self.schedule_target_recovery(target); }
                self.session_fact_provider.request_refresh();
                self.schedule_projection_refresh("connections_updated");
                self.state_events.publish(ApplicationStateApiV1::ControlCenterV1);
                self.state_events.publish(ApplicationStateApiV1::ResourcesV1);
                return Ok(None);
            }
            RefreshSessions => {
                self.session_fact_provider.request_refresh();
                self.schedule_projection_refresh("explicit_refresh");
                return Ok(None);
            }
            SetSshTarget {
                target_id,
                target_revision,
                destination,
                route_profile,
            } => {
                let targets = self.targets.clone();
                let target = tokio::task::spawn_blocking(move || {
                    targets.configure_ssh_target(
                        &target_id,
                        target_revision,
                        &destination,
                        route_profile.as_ref(),
                    )
                })
                .await
                .map_err(|_| {
                    control_error(
                        "target_update_failed",
                        "The SSH target update did not complete.",
                    )
                })?
                .map_err(|_| {
                    control_error(
                        "target_update_failed",
                        "The SSH target could not be saved. Refresh targets before retrying.",
                    )
                })?;
                self.schedule_target_recovery(target);
                self.state_events
                    .publish(ApplicationStateApiV1::ControlCenterV1);
                return Ok(None);
            }
            QuickCreateSession {
                execution_target_id,
                source_session_id,
                kind,
            } => SessionOperation::QuickCreate {
                execution_target_id: target_id(&execution_target_id)?,
                source_session_id,
                kind,
            },
            CreateSession {
                execution_target_id,
                name,
                working_directory,
                command,
                auto_restart,
            } => SessionOperation::Create {
                execution_target_id: target_id(&execution_target_id)?,
                name,
                working_directory,
                command,
                auto_restart,
            },
            UseExistingSession {
                execution_target_id,
                tmux_session_id,
                replacement,
                confirmation_id,
                retire,
            } => {
                if confirmation_id
                    != format!(
                        "replace-session-{execution_target_id}-{tmux_session_id}-{}",
                        replacement.tmux_session_id
                    )
                {
                    return Err(control_error(
                        "confirmation_required",
                        "Confirm this exact lost Session before using the existing Session.",
                    ));
                }
                SessionOperation::UseExisting {
                    session: session_key(&execution_target_id, &tmux_session_id)?,
                    replacement,
                    retire,
                }
            }
            RestartSession {
                execution_target_id,
                tmux_session_id,
            } => SessionOperation::Restart {
                session: session_key(&execution_target_id, &tmux_session_id)?,
            },
            EndAgentSession { owner_host_id, endpoint_id, thread_id, confirmation_id } => {
                self.end_agent_confirmed(principal, request_id, &owner_host_id, &endpoint_id, &thread_id, &confirmation_id).await?;
                self.schedule_projection_refresh("agent_ended");
                return Ok(None);
            }
            RenameAgentSession { owner_host_id, endpoint_id, thread_id, name } => {
                let codex = self.codex.get().ok_or_else(|| control_error("codex_unavailable", "Codex control is unavailable."))?;
                codex.rename_agent_thread(principal, owner_host_id, endpoint_id, thread_id, name).await?;
                self.schedule_projection_refresh("agent_renamed");
                return Ok(None);
            }
            RenameSession {
                execution_target_id,
                tmux_session_id,
                name,
            } => SessionOperation::Rename {
                session: session_key(&execution_target_id, &tmux_session_id)?,
                name,
            },
            KillSession { execution_target_id, tmux_session_id, confirmation_id } => {
                return self.end_confirmed(principal, request_id, execution_target_id, tmux_session_id, confirmation_id).await;
            }

            AcknowledgeSessionSummary {
                execution_target_id,
                tmux_session_id,
                summary_revision,
            } => {
                let key = SessionKeyV1 {
                    execution_target: ExecutionTarget::new(execution_target_id).map_err(|_| {
                        control_error(
                            "session_identity_invalid",
                            "The Session identity is invalid.",
                        )
                    })?,
                    tmux_session_id,
                };
                self.session_registry
                    .acknowledge_attention(&key, summary_revision)
                    .await
                    .map_err(|_| {
                        control_error(
                            "attention_update_failed",
                            "The Session summary could not be acknowledged.",
                        )
                    })?
                    .ok_or_else(|| {
                        control_error(
                            "control_projection_stale",
                            "The Session summary changed. Refresh before retrying.",
                        )
                    })?;
                return Ok(None);
            }
            MarkAttentionNotificationHandled {
                execution_target_id,
                tmux_session_id,
                summary_revision,
            } => {
                let key = SessionKeyV1 {
                    execution_target: ExecutionTarget::new(execution_target_id).map_err(|_| {
                        control_error(
                            "session_identity_invalid",
                            "The Session identity is invalid.",
                        )
                    })?,
                    tmux_session_id,
                };
                self.session_registry
                    .mark_attention_notification_handled(&key, summary_revision)
                    .await
                    .map_err(|_| {
                        control_error(
                            "attention_update_failed",
                            "The Session notification could not be updated.",
                        )
                    })?
                    .ok_or_else(|| {
                        control_error(
                            "control_projection_stale",
                            "The Session notification changed. Refresh before retrying.",
                        )
                    })?;
                return Ok(None);
            }
        };
        let created = self
            .session_fact_provider
            .apply_session_operation(operation)
            .await
            .map_err(|error| control_error(error.code, error.message))?;
        self.commit_created(created).await
    }
    async fn commit_created(&self, created: Option<SessionFact>) -> Result<Option<SessionFact>, ControlCenterErrorV1> {
        if let Some(fact) = &created {
            let mut cache = self.session_facts.lock().await;
            cache.facts.retain(|row| row.identity != fact.identity);
            cache.facts.push(fact.clone());
            sort_session_facts(&mut cache.facts);
            cache.revision = cache.revision.saturating_add(1);
            tracing::info!(target: "polar_station_operation", component="Sessions", event="session_create_projection_committed", revision=cache.revision, target_id=fact.identity.execution_target_id.as_str());
            drop(cache);
            self.state_events
                .publish(ApplicationStateApiV1::ControlCenterV1);
        } else {
            // Mutations that do not return a committed fact still use the existing feed.
            self.schedule_projection_refresh("operation");
        }
        Ok(created)
    }

    async fn projection(&self, revision: u64) -> ControlCenterProjectionV1 {
        let snapshot = self.session_facts.lock().await.snapshot();
        let library = project_session_library(&SessionLibraryContext {
            sessions_visible: false,
            selected_session: None,
            sessions: snapshot.facts,
            session_targets: snapshot.targets,
            local_session_discovery: snapshot.local_discovery,
            remote_session_discovery: snapshot.remote_discovery,
        });
        let mut session_groups = Vec::new();
        for target in self.targets.descriptors() {
            let id = target.id.as_str();
            let count = library.rows.iter().filter(|row| row.identity.execution_target_id.as_str() == id).count();
            let observed = self.lifecycle.observation_status(id).await;
            let ssh = self.ssh_connections.current_status_change(id);
            use crate::ssh_connection_manager::SshConnectionStatus;
            let state = match (ssh.map(|s| s.status), observed) {
                (Some(SshConnectionStatus::Offline), _) => SessionTargetStateV1::Offline,
                (Some(SshConnectionStatus::Connecting | SshConnectionStatus::Recovering), _) => SessionTargetStateV1::Loading,
                (_, Some((false, Some(polar_tmux_core::TmuxErrorKind::ExecutableMissing)))) => SessionTargetStateV1::MissingTmux,
                (_, Some((false, _))) => SessionTargetStateV1::Failed,
                (_, Some((true, _))) if count == 0 => SessionTargetStateV1::Empty,
                (_, Some((true, _))) => SessionTargetStateV1::Ready,
                (_, None) => SessionTargetStateV1::Loading,
            };
            let stale = count > 0 && !matches!(state, SessionTargetStateV1::Ready | SessionTargetStateV1::Empty);
            let message = match state {
                SessionTargetStateV1::Loading => Some("Loading Sessions…"),
                SessionTargetStateV1::Empty => Some("No Sessions on this machine."),
                SessionTargetStateV1::Offline => Some("SSH is offline. Saved Sessions are preserved."),
                SessionTargetStateV1::MissingTmux => Some("tmux is not installed on this machine. Polar chat remains independent."),
                SessionTargetStateV1::Failed => Some("Session discovery failed. Retry after checking this machine."),
                SessionTargetStateV1::Ready => None,
            }.map(|m| if stale { format!("{m} Showing last known Sessions.") } else { m.into() });
            session_groups.push(SessionTargetGroupV1 { id: id.into(), title: target.display_name, state, message, stale });
        }
        session_groups.sort_by_key(|g| (g.id != "local", g.title.clone(), g.id.clone()));
        let target = self.targets.ssh_targets().into_iter().next();
        let mut connections = self.targets.connections();
        for machine in &connections.machines {
            use crate::ssh_connection_manager::SshConnectionStatus;
            let observed = self.ssh_connections.current_status_change(&machine.id);
            let status = match observed.as_ref().map(|v| v.status) {
                Some(SshConnectionStatus::Connected) => ControlConnectionStatusV1::Connected,
                Some(SshConnectionStatus::Connecting) => ControlConnectionStatusV1::Connecting,
                Some(SshConnectionStatus::Recovering) => ControlConnectionStatusV1::Recovering,
                Some(SshConnectionStatus::Offline) => ControlConnectionStatusV1::Offline,
                None => ControlConnectionStatusV1::Inactive,
            };
            let route = match observed.and_then(|v| v.route) {
                Some(SshRouteKind::Lan) => Some(ControlConnectionRouteV1::Lan),
                Some(SshRouteKind::Wan) => Some(ControlConnectionRouteV1::Wan),
                _ => None,
            };
            let label = match status {
                ControlConnectionStatusV1::Connected => "Connected",
                ControlConnectionStatusV1::Connecting => "Connecting",
                ControlConnectionStatusV1::Recovering => "Reconnecting",
                ControlConnectionStatusV1::Offline => "Offline",
                _ => "Not connected",
            };
            connections.machine_statuses.push(polar_tmux_protocol::MachineConnectionStatusV1 {
                id: machine.id.clone(),
                connection: ControlConnectionProjectionV1 {
                    label: label.into(), route, status,
                    accessibility_label: format!("{}: SSH {}", machine.display_name, label.to_lowercase()),
                },
            });
        }
        let summary = machine_connection_summary(&connections, snapshot.connection_status);
        let action = |id: &str, label: &str, role| {
            wire_action(&ControlActionProjection::enabled(id, label, role))
        };
        let rename = action(
            "workspace.rename",
            "Rename Workspace",
            ControlActionRole::Secondary,
        );
        let add_tab = action("workspace.tab.add", "Add Tab", ControlActionRole::Primary);
        let restore = wire_action(
            &ControlActionProjection::enabled(
                "workspace.restore",
                "Restore Saved Layout",
                ControlActionRole::Recovery,
            )
            .with_confirmation(
                "restore-saved-layout",
                "Restore Saved Layout?",
                "Restore the saved native Tabs and Panes without changing tmux Sessions.",
                "Restore Saved Layout",
            ),
        );
        let detach = wire_action(&ControlActionProjection::close_window());
        let delete = wire_action(&ControlActionProjection::archive_workspace());
        let actions = vec![
            action("sessions.toggle", "Sessions", ControlActionRole::Toggle),
            action(
                "presentation.layout.toggle",
                "Layout",
                ControlActionRole::Toggle,
            ),
            action(
                "presentation.inbox.toggle",
                "Inbox",
                ControlActionRole::Toggle,
            ),
            action("resources.open", "Resources", ControlActionRole::Secondary),
            action("more.open", "More", ControlActionRole::Secondary),
        ];
        let more_actions = vec![
            action(
                "managed.terminal.font.size",
                "Ghostty Font Size…",
                ControlActionRole::Secondary,
            ),
            action(
                "workspace.create",
                "New Workspace",
                ControlActionRole::Primary,
            ),
        ];
        let mut projected_actions = vec![
            rename.clone(),
            add_tab.clone(),
            restore.clone(),
            detach.clone(),
            delete,
            wire_action(&ControlActionProjection::new_workspace_window()),
        ];
        projected_actions.push(action(
            "session.quick.create.shell",
            "New Shell in Current Tab",
            ControlActionRole::Primary,
        ));
        projected_actions.push(action(
            "session.quick.create.codex",
            "New Codex in Current Tab",
            ControlActionRole::Primary,
        ));
        projected_actions.extend(actions.clone());
        projected_actions.extend(more_actions.clone());
        projected_actions.push(action(
            "iphone.open",
            "iPhone Companion…",
            ControlActionRole::Secondary,
        ));
        projected_actions.push(wire_action(&library.refresh_action));
        projected_actions.extend(
            library
                .rows
                .iter()
                .filter_map(|row| row.restart_action.as_ref())
                .map(wire_action),
        );
        let attention = self.session_registry.attention_snapshot().await;
        let attention_badge = badge_for(attention.iter().map(|(_, attention)| attention));
        let mut sessions = wire_session_library(&library);
        #[cfg(feature = "codex-control")]
        let agent_sessions = if let Some(codex) = self.codex.get() { codex.session_catalog(&mut sessions.rows).await } else { Vec::new() };
        #[cfg(not(feature = "codex-control"))]
        let agent_sessions = Vec::new();
        #[cfg(feature = "codex-control")]
        let codex_endpoint_states = if let Some(codex) = self.codex.get() { codex.endpoint_states().await } else { Default::default() };
        #[cfg(not(feature = "codex-control"))]
        let codex_endpoint_states = Default::default();
        #[cfg(feature = "codex-control")]
        let codex_endpoints = if let Some(codex) = self.codex.get() { codex.catalog_endpoints().await } else { Vec::new() };
        #[cfg(not(feature = "codex-control"))]
        let codex_endpoints = Vec::new();
        ControlCenterProjectionV1 {
            codex_endpoint_states,
            agent_sessions,
            schema_version: 1,
            control_revision: revision,
            connections,
            codex_endpoints,
            session_groups,
            header: ControlHeaderProjectionV1 {
                connection: summary,
                ssh_target_editor: WorkspaceSshTargetEditorProjectionV1 {
                    title: "SSH Target".into(),
                    target_id: target
                        .as_ref()
                        .map_or_else(|| "remote".into(), |t| t.target_id.clone()),
                    target_revision: target.as_ref().map_or(0, |t| t.revision),
                    destination: target
                        .as_ref()
                        .map_or_else(String::new, |t| t.destination.clone()),
                    route_profile: target
                        .as_ref()
                        .and_then(WorkspaceSshTarget::route_profile_projection),
                    destination_label: "Destination".into(),
                    destination_placeholder: "user@host".into(),
                    cancel_label: "Cancel".into(),
                    save_label: "Save".into(),
                },
                rename_action: rename,
                add_tab,
                refresh: restore,
                workspace_menu_detach_action: Some(detach),
                lifecycle_action: None,
            },
            sessions,
            bottom_bar: ControlBottomBarProjectionV1 {
                actions,
                more_actions,
            },
            projected_actions,
            auxiliary_surfaces: AuxiliarySurfacesProjectionV1 {
                resources: AuxiliarySurfaceProjectionV1 {
                    id: "resources".into(),
                    title: "Resources".into(),
                    state: AuxiliarySurfaceStateV1::Available,
                    message: "Edit the Host-owned Resource catalog.".into(),
                },
                iphone: AuxiliarySurfaceProjectionV1 {
                    id: "iphone".into(),
                    title: "iPhone".into(),
                    state: AuxiliarySurfaceStateV1::Available,
                    message: "Show Host pairing and Companion state.".into(),
                },
            },
            banner: None,
            attention_badge,
        }
    }
    // SessionLifecycle owns transport/discovery events. This cache only projects
    // its facts, independently per section so a remote provider cannot block local UI.
    pub(crate) fn refresh_target_projection(&self, target: &ExecutionTarget, reason: &'static str) {
        // Per-machine SSH transitions may leave the worst aggregate and Session
        // rows unchanged. Notify their projection directly, without a discovery read.
        if reason == "transport" {
            self.state_events.publish(ApplicationStateApiV1::ControlCenterV1);
        }
        let section = if target.is_local() {
            SessionSection::Local
        } else {
            SessionSection::Remote
        };
        self.schedule_section_projection(section, reason);
    }

    fn schedule_projection_refresh(&self, reason: &'static str) {
        self.schedule_section_projection(SessionSection::Local, reason);
        self.schedule_section_projection(SessionSection::Remote, reason);
    }

    fn schedule_section_projection(&self, section: SessionSection, reason: &'static str) {
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_requested", section=projection_section_label(section), reason);
        let refresh = match section {
            SessionSection::Local => self.local_projection_refresh.clone(),
            SessionSection::Remote => self.remote_projection_refresh.clone(),
        };
        {
            let mut flags = refresh.lock().expect("Session list refresh lock");
            if flags.0 {
                flags.1 = true;
                tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_coalesced", section=projection_section_label(section), reason);
                return;
            }
            flags.0 = true;
        }
        let provider = Arc::clone(&self.session_fact_provider);
        let cache = Arc::clone(&self.session_facts);
        let state_events = self.state_events.clone();
        tokio::spawn(async move {
            let mut cause = reason;
            loop {
                // The upcoming read covers requests received while queued. Only
                // invalidations during this read require another pass.
                refresh.lock().expect("Session list refresh lock").1 = false;
                let applied =
                    refresh_section_projection(&*provider, &cache, &state_events, section, cause)
                        .await;
                let mut flags = refresh.lock().expect("Session list refresh lock");
                if !applied {
                    flags.1 = true;
                }
                if !flags.1 {
                    flags.0 = false;
                    break;
                }
                flags.1 = false;
                cause = "coalesced";
            }
        });
    }

    fn schedule_target_recovery(&self, target: WorkspaceSshTarget) {
        let ssh_connections = self.ssh_connections.clone();
        let provider = self.session_fact_provider.clone();
        let cache = self.session_facts.clone();
        let state_events = self.state_events.clone();
        let lifecycle = self.lifecycle.clone();
        tokio::spawn(async move {
            ssh_connections.retire(&target.target_id).await;
            // Registry changes are explicit invalidations, including targets
            // configured after the runtime's initial target scan.
            if let Ok(target_id) = ExecutionTargetId::new(target.target_id.clone()) {
                lifecycle.request_refresh(target_id);
            }
            refresh_section_projection(
                &*provider,
                &cache,
                &state_events,
                SessionSection::Remote,
                "target_configuration",
            )
            .await;
        });
    }
}
pub(super) fn machine_connection_summary(
    connections: &polar_tmux_protocol::ConnectionsProjectionV1,
    fallback: ControlConnectionStatus,
) -> ControlConnectionProjectionV1 {
    let total = connections.machine_statuses.len();
    let connected = connections
        .machine_statuses
        .iter()
        .filter(|m| m.connection.status == ControlConnectionStatusV1::Connected)
        .count();
    let status = if connected > 0 && connected < total {
        ControlConnectionStatusV1::Partial
    } else if connected > 0 {
        ControlConnectionStatusV1::Connected
    } else if total > 0 {
        let has = |status| {
            connections
                .machine_statuses
                .iter()
                .any(|m| m.connection.status == status)
        };
        if has(ControlConnectionStatusV1::Recovering) {
            ControlConnectionStatusV1::Recovering
        } else if has(ControlConnectionStatusV1::Connecting) {
            ControlConnectionStatusV1::Connecting
        } else if has(ControlConnectionStatusV1::Offline) {
            ControlConnectionStatusV1::Offline
        } else {
            ControlConnectionStatusV1::Inactive
        }
    } else {
        match fallback {
            ControlConnectionStatus::Inactive => ControlConnectionStatusV1::Inactive,
            ControlConnectionStatus::Connecting => ControlConnectionStatusV1::Connecting,
            ControlConnectionStatus::Connected => ControlConnectionStatusV1::Connected,
            ControlConnectionStatus::Recovering => ControlConnectionStatusV1::Recovering,
            ControlConnectionStatus::Offline => ControlConnectionStatusV1::Offline,
        }
    };
    let label = if total > 0 {
        format!("{connected}/{total} connected")
    } else {
        match status {
            ControlConnectionStatusV1::Connecting => "Connecting",
            ControlConnectionStatusV1::Recovering => "Reconnecting",
            _ => "No SSH machines",
        }
        .into()
    };
    let route = if total == 1 {
        connections.machine_statuses[0].connection.route
    } else {
        None
    };
    ControlConnectionProjectionV1 {
        accessibility_label: format!(
            "SSH machines: {label}. Open Connections for individual machine status."
        ),
        label,
        route,
        status,
    }
}


fn control_error(code: &str, message: impl Into<String>) -> ControlCenterErrorV1 {
    ControlCenterErrorV1 {
        resume: None,
        code: code.into(),
        message: message.into(),
    }
}
fn target_id(value: &str) -> Result<ExecutionTargetId, ControlCenterErrorV1> {
    ExecutionTargetId::new(value).map_err(|_| {
        control_error(
            "session_identity_invalid",
            "The target identity is invalid.",
        )
    })
}
fn session_key(target: &str, session: &str) -> Result<SessionKey, ControlCenterErrorV1> {
    Ok(SessionKey::new(
        target_id(target)?,
        StableTmuxSessionId::new(session).map_err(|_| {
            control_error(
                "session_identity_invalid",
                "The Session identity is invalid.",
            )
        })?,
    ))
}
fn projection_section_label(section: SessionSection) -> &'static str {
    match section {
        SessionSection::Local => "local",
        SessionSection::Remote => "remote",
    }
}

#[tracing::instrument(target = "polar_station_operation", name = "session_projection_refresh", skip_all, fields(component="Sessions", section=projection_section_label(section), reason=reason))]
async fn refresh_section_projection(
    provider: &dyn SessionFactProvider,
    cache: &Mutex<SessionFactCache>,
    events: &ApplicationStatePublisher,
    section: SessionSection,
    reason: &'static str,
) -> bool {
    tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_execute", section=projection_section_label(section), reason);
    let expected_revision = cache.lock().await.revision;
    let snapshot = provider.list_session_facts(section)
        .instrument(tracing::info_span!(target: "polar_station_operation", "session_projection_read_facts", component="Sessions"))
        .await;
    apply_session_section_fact_snapshot(cache, events, snapshot, expected_revision)
        .instrument(tracing::info_span!(target: "polar_station_operation", "session_projection_apply", component="Sessions"))
        .await
}

async fn apply_session_section_fact_snapshot(
    cache: &Mutex<SessionFactCache>,
    state_events: &ApplicationStatePublisher,
    snapshot: SessionSectionFactSnapshot,
    expected_revision: u64,
) -> bool {
    let section = snapshot.section;
    let changed = {
        let mut cache = cache.lock().await;
        if cache.revision != expected_revision {
            tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_stale_discarded", expected_revision, current_revision=cache.revision);
            return false;
        }
        let before = cache.snapshot();
        let SessionSectionFactSnapshot {
            section,
            facts,
            targets,
            discovery,
            connection_status,
            error_message,
        } = snapshot;

        cache.targets.retain(|target| target.section != section);
        cache.targets.extend(targets);
        cache.targets.sort_by(|left, right| {
            left.section
                .cmp(&right.section)
                .then_with(|| left.id.cmp(&right.id))
        });

        // A failed remote refresh may still contain facts from healthy
        // targets. Replace the section with those partial facts, but retain
        // the last usable rows when every target failed.
        if discovery == SessionDiscoveryStatus::Ready || !facts.is_empty() {
            cache.facts.retain(|fact| fact.section != section);
            cache.facts.extend(facts);
            sort_session_facts(&mut cache.facts);
        }

        match section {
            SessionSection::Local => cache.local_discovery = discovery,
            SessionSection::Remote => cache.remote_discovery = discovery,
        }
        if let Some(connection_status) = connection_status {
            cache.connection_status = connection_status;
        }
        if section == SessionSection::Remote {
            cache.connection_error = error_message;
        }

        if before == cache.snapshot() {
            false
        } else {
            cache.revision = cache.revision.saturating_add(1);
            true
        }
    };
    if changed {
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_changed", section=projection_section_label(section));
        state_events.publish(ApplicationStateApiV1::ControlCenterV1);
    } else {
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_projection_unchanged", section=projection_section_label(section));
    }
    true
}

fn projection_response(
    request: &ControlCenterRequestEnvelopeV1,
    control_revision: u64,
    projection: ControlCenterProjectionV1,
) -> ControlCenterResponseEnvelopeV1 {
    ControlCenterResponseEnvelopeV1 {
        protocol_version: PROTOCOL_VERSION,
        api: ControlCenterApiV1::ControlCenterV1,
        request_id: request.request_id.clone(),
        control_revision,
        result: ControlCenterResultV1::Projection { projection },
    }
}

fn accepted_response(
    request: &ControlCenterRequestEnvelopeV1,
    control_revision: u64,
    projection: ControlCenterProjectionV1,
) -> ControlCenterResponseEnvelopeV1 {
    ControlCenterResponseEnvelopeV1 {
        protocol_version: PROTOCOL_VERSION,
        api: ControlCenterApiV1::ControlCenterV1,
        request_id: request.request_id.clone(),
        control_revision,
        result: ControlCenterResultV1::IntentAccepted { projection },
    }
}

fn error_response(
    request: &ControlCenterRequestEnvelopeV1,
    control_revision: u64,
    code: &str,
    message: impl Into<String>,
    projection: Option<ControlCenterProjectionV1>,
) -> ControlCenterResponseEnvelopeV1 {
    ControlCenterResponseEnvelopeV1 {
        protocol_version: PROTOCOL_VERSION,
        api: ControlCenterApiV1::ControlCenterV1,
        request_id: request.request_id.clone(),
        control_revision,
        result: ControlCenterResultV1::Error {
            error: ControlCenterErrorV1 {
                resume: None,
                code: code.to_owned(),
                message: message.into(),
            },
            projection,
        },
    }
}

fn sort_session_facts(facts: &mut [SessionFact]) {
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
}


