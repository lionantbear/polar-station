//! Durable Session identity and recovery facts. Native layout is not stored here.
pub(crate) mod runtime;
use super::session_registry::{DiscoveredSession, SessionRegistry};
use anyhow::{Context, Result};
use polar_tmux_core::{TmuxClient, TmuxErrorKind};
use polar_tmux_protocol::{ExecutionTarget, SessionLifecycleV1};
use polar_tmux_workspace::{
    decide_recovery, SessionFact, SessionKey, SessionRecoveryDecision, SessionSection, SessionKind,
    TransportStatus,
};
use polar_tmux_workspace::{
    ExecutionTargetId, ManagedSessionId, ManagedSessionRecord, ProcessRestoreSpec,
    SessionLaunchSpec, SessionOperationReceipt, SessionRecoveryStatus, SessionRuntimeState,
    StableTmuxSessionId,
};
use std::collections::BTreeMap;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;
use tracing::Instrument;

use super::session_store::SessionStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionOperation {
    QuickCreate {
        execution_target_id: ExecutionTargetId,
        source_session_id: Option<String>,
        kind: polar_tmux_protocol::QuickSessionKindV1,
    },
    Create {
        execution_target_id: ExecutionTargetId,
        name: String,
        working_directory: Option<String>,
        command: Option<String>,
        auto_restart: bool,
    },
    CreateClaude {
        execution_target_id: ExecutionTargetId,
        name: String,
        working_directory: Option<String>,
        resume_id: Option<String>,
    },
    UseExisting {
        session: SessionKey,
        replacement: polar_tmux_protocol::SessionReplacementV1,
        retire: bool,
    },
    Restart {
        session: SessionKey,
    },
    Rename {
        session: SessionKey,
        name: String,
    },
    Kill {
        session: SessionKey,
        binding: Option<polar_tmux_core::TmuxSessionBinding>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionOperationError {
    pub code: &'static str,
    pub message: &'static str,
}

impl SessionOperationError {
    

    pub(super) fn target_not_found() -> Self {
        Self {
            code: "control_session_target_not_found",
            message: "The selected execution target is not configured.",
        }
    }

    pub(super) fn failed() -> Self {
        Self {
            code: "control_session_operation_failed",
            message: "The tmux Session operation failed.",
        }
    }

    pub(super) fn from_tmux(error: &polar_tmux_core::TmuxError) -> Self {
        Self::from_tmux_kind(error.kind())
    }

    pub(super) fn from_tmux_kind(kind: TmuxErrorKind) -> Self {
        if kind == TmuxErrorKind::SessionAlreadyExists {
            Self {
                code: "control_session_already_exists",
                message: "A Session with this name already exists. Choose another name, or cancel and add the existing Session from Sessions.",
            }
        } else {
            Self::failed()
        }
    }

    pub(super) fn missing() -> Self {
        Self {
            code: "control_session_missing",
            message: "The tmux Session no longer exists.",
        }
    }
}

#[derive(Clone)]
pub(crate) enum SessionRefreshRequest {
    Observe(ExecutionTargetId),
    Project(ExecutionTargetId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshAdmission {
    Queued,
    Running { invalidated: bool },
}

pub(crate) struct SessionLifecycle {
    store: SessionStore,
    state: Mutex<SessionRuntimeState>,
    observations: Mutex<BTreeMap<String, ObservedTarget>>,
    observation_published: tokio::sync::Notify,
    refreshes: StdMutex<BTreeMap<String, RefreshAdmission>>,
    refresh_requests: tokio::sync::broadcast::Sender<SessionRefreshRequest>,
    
}

pub(crate) enum OperationAdmission {
    Started,
    Completed(String),
    Unknown,
    Conflict,
}

/// One complete generation-verified observation, shared by recovery and projection.
#[derive(Clone)]
struct TargetObservation {
    generation: Option<String>,
    sessions: Vec<polar_tmux_protocol::TmuxSession>,
    markers: BTreeMap<String, Option<String>>,
    clients: Vec<polar_tmux_protocol::TmuxClientInfo>,
    panes: Vec<(String, String, bool)>,
}

struct ObservedTarget {
    facts: Vec<SessionFact>,
    available: bool,
    failure_kind: Option<polar_tmux_core::TmuxErrorKind>,
    snapshot: TargetObservation,
    transport: Option<polar_tmux_protocol::SshTransportGenerationV1>,
}

impl TargetObservation {
    fn decision(&self, record: &ManagedSessionRecord) -> SessionRecoveryDecision {
        let same_server = self.generation.as_deref() == Some(record.server_generation.as_str());
        let marker = record.id.to_string();
        let matches = self
            .markers
            .values()
            .filter(|value| value.as_deref() == Some(marker.as_str()))
            .count();
        let occupied = self.sessions.iter().any(|session| {
            if record.status == SessionRecoveryStatus::Ending {
                same_server && session.id == record.runtime_tmux_session_id.as_str()
                    && session.created_unix_seconds == record.created_unix_seconds
            } else {
                session.name == record.launch.session_name
                    || (same_server && session.id == record.runtime_tmux_session_id.as_str())
            }
        });
        decide_recovery(record, same_server, matches, occupied)
    }

    async fn read(client: &TmuxClient) -> Result<Self> {
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_observation_execute");
        let observed = client.observe_sessions().await?;
        let generation = observed.generation;
        let discovered = observed.sessions;
        let mut sessions = Vec::with_capacity(discovered.len());
        let mut markers = BTreeMap::new();
        for (session, marker) in discovered {
            anyhow::ensure!(
                markers.insert(session.id.clone(), marker).is_none(),
                "duplicate Session in observation"
            );
            sessions.push(session);
        }
        Ok(Self {
            generation,
            sessions,
            markers,
            clients: observed.clients,
            panes: observed.panes,
        })
    }
}

impl SessionLifecycle {
    /// Resolve the actual source Session directory through the existing target
    /// client. Native OSC7 state is not authoritative across tmux attachment.
    pub(crate) async fn source_directory(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
    ) -> Result<String> {
        let generation = client.server_generation().await?;
        let source = self.resolve(client, target, reference).await?;
        let panes = client.list_panes(&source).await?;
        let pane = panes.iter().find(|pane| pane.active && !pane.dead)
            .context("source Session has no live active Pane")?;
        anyhow::ensure!(std::path::Path::new(&pane.current_path).is_absolute(),
            "source directory is unavailable");
        anyhow::ensure!(client.server_generation().await? == generation, "source server changed");
        Ok(pane.current_path.clone())
    }

    pub(crate) async fn apply_operation(
        &self,
        operation: SessionOperation,
        targets: &crate::target_registry::TargetRegistry,
        ssh: &crate::ssh_connection_manager::SshConnectionManager,
    ) -> Result<Option<SessionFact>, SessionOperationError> {
        let target_id = match &operation {
            SessionOperation::QuickCreate {
                execution_target_id,
                ..
            } => execution_target_id,
            SessionOperation::Create {
                execution_target_id,
                ..
            } => execution_target_id,
            SessionOperation::CreateClaude { execution_target_id, .. } => execution_target_id,
            SessionOperation::UseExisting { session, .. }
            | SessionOperation::Restart { session }
            | SessionOperation::Rename { session, .. }
            | SessionOperation::Kill { session, .. } => &session.execution_target_id,
        }
        .clone();
        let target = targets
            .get(target_id.as_str())
            .ok_or_else(SessionOperationError::target_not_found)?;
        let gate = target.operation_gate();
        let _operation = gate.write().await;
        let replacing = matches!(&operation, SessionOperation::UseExisting { .. });
        let result: Result<Option<SessionFact>> = async {
            let client = target.managed_client(ssh).await?;
            match operation {
                SessionOperation::QuickCreate {
                    source_session_id,
                    kind,
                    ..
                } => {
                    let directory = if let Some(source) = source_session_id {
                        Some(self.source_directory(&client, target_id.as_str(), &source).await?)
                    } else {
                        anyhow::ensure!(
                            target_id == ExecutionTargetId::local(),
                            "empty Tab requires local target"
                        );
                        None
                    };
                    let prefix = match kind {
                        polar_tmux_protocol::QuickSessionKindV1::Shell => "shell",
                        polar_tmux_protocol::QuickSessionKindV1::Codex => "codex",
                    };
                    anyhow::ensure!(kind != polar_tmux_protocol::QuickSessionKindV1::Codex, "Codex creation requires an app-server endpoint");
                    let launch = SessionLaunchSpec::shell(
                        format!("{prefix}-{}", ManagedSessionId::new()),
                        directory.as_deref(),
                    );
                    let created = self.create(&client, target_id.clone(), launch).await?;
                    return Ok(Some(
                        self.created_fact(
                            &created,
                            &target.descriptor().display_name,
                            ssh.transport_generation(target_id.as_str()).await,
                        )
                        .await,
                    ));
                }
                SessionOperation::Create {
                    name,
                    working_directory,
                    command,
                    auto_restart,
                    ..
                } => {
                    let mut launch = SessionLaunchSpec::shell(name, working_directory.as_deref());
                    if let Some(command) = command {
                        launch.process = ProcessRestoreSpec::Command {
                            command,
                            auto_restart,
                        };
                    }
                    let created = self.create(&client, target_id.clone(), launch).await?;
                    return Ok(Some(
                        self.created_fact(
                            &created,
                            &target.descriptor().display_name,
                            ssh.transport_generation(target_id.as_str()).await,
                        )
                        .await,
                    ));
                }
                SessionOperation::CreateClaude { name: _, working_directory, resume_id, .. } => {
                    let new = resume_id.is_none();
                    let session_id = resume_id.unwrap_or_else(crate::host_identity::random_uuid_v4);
                    let name = SessionKind::ClaudePlane.terminal_name(&session_id).context("Invalid Claude Session ID")?;
                    if let Some(fact) = self.existing_public_session(&client, target_id.as_str(), &name).await? { return Ok(Some(fact)); }
                    // Refuse to create a fake durable session when the CLI is missing.
                    ensure_cli_launcher(client.target(), working_directory.as_deref(), "claude").await?;
                    let mut launch = SessionLaunchSpec::shell(name, working_directory.as_deref());
                    launch.process = ProcessRestoreSpec::Claude { session_id: session_id.clone() };
                    let initial = new.then(||claude_initial_command(&session_id));
                    let created = self.create_with_public_name(&client, target_id.clone(), launch, initial, true).await?;
                    return Ok(Some(self.created_fact(&created, &target.descriptor().display_name,
                        ssh.transport_generation(target_id.as_str()).await).await));
                }
                SessionOperation::UseExisting {
                    session,
                    replacement,
                    retire,
                } => {
                    self.use_existing_session(&client, &session, &replacement, retire)
                        .await?;
                }
                SessionOperation::Restart { session } => {
                    self.restart(
                        &client,
                        target_id.as_str(),
                        session.tmux_session_id.as_str(),
                    )
                    .await?
                }
                SessionOperation::Rename { session, name } => {
                    self.rename_session(
                        &client,
                        target_id.as_str(),
                        session.tmux_session_id.as_str(),
                        name,
                    )
                    .await?
                }
                SessionOperation::Kill { session, binding } => {
                    self.end_session_bound(&client, &target_id, session.tmux_session_id.as_str(), binding.as_ref()).await?
                }
            }
            Ok(None)
        }
        .await;
        // Recheck even after an uncertain side effect; never replay the operation.
        self.request_refresh(target_id);
        result.map_err(|error| {
            if replacing {
                return SessionOperationError { code: "control_session_replacement_changed", message: "The Session replacement could not be verified. Refresh Sessions and confirm the current Session again." };
            }
            if error.to_string() == "Codex launcher or working directory is unavailable on the selected target" {
                return SessionOperationError { code: "codex_launcher_unavailable", message: "Codex is not installed or available on the selected target." };
            }
            if error.to_string() == "Claude launcher or working directory is unavailable on the selected target" {
                return SessionOperationError { code: "claude_launcher_unavailable", message: "Claude is not installed or the directory is unavailable on the selected Host." };
            }
            error
                .downcast_ref::<polar_tmux_core::TmuxError>()
                .map_or_else(
                    SessionOperationError::failed,
                    SessionOperationError::from_tmux,
                )
        })
    }

    pub(crate) fn request_refresh(&self, target: ExecutionTargetId) {
        self.admit_refresh(target, true);
    }

    /// Missing readers share queued or running work without invalidating it.
    pub(crate) fn request_refresh_if_idle(&self, target: ExecutionTargetId) {
        self.admit_refresh(target, false);
    }

    fn admit_refresh(&self, target: ExecutionTargetId, invalidate: bool) {
        let reason = if invalidate {
            "invalidation"
        } else {
            "idle_demand"
        };
        tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_requested", target_id=target.as_str(), reason);
        let mut refreshes = self.refreshes.lock().expect("Session refresh lock");
        match refreshes.get_mut(target.as_str()) {
            Some(RefreshAdmission::Running { invalidated }) => {
                *invalidated |= invalidate;
                tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_coalesced", target_id=target.as_str(), reason);
            }
            Some(RefreshAdmission::Queued) => {
                tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_coalesced", target_id=target.as_str(), reason);
            }
            None => {
                refreshes.insert(target.to_string(), RefreshAdmission::Queued);
                let _ = self
                    .refresh_requests
                    .send(SessionRefreshRequest::Observe(target));
            }
        }
    }

    pub(crate) fn begin_refresh(&self, target: &str) -> bool {
        self.claim_refresh(target, false)
    }

    pub(crate) fn begin_queued_refresh(&self, target: &str) -> bool {
        self.claim_refresh(target, true)
    }

    fn claim_refresh(&self, target: &str, queued_only: bool) -> bool {
        let mut refreshes = self.refreshes.lock().expect("Session refresh lock");
        if queued_only && refreshes.get(target) != Some(&RefreshAdmission::Queued) {
            return false;
        }
        if matches!(
            refreshes.get(target),
            Some(RefreshAdmission::Running { .. })
        ) {
            return false;
        }
        refreshes.insert(
            target.to_owned(),
            RefreshAdmission::Running { invalidated: false },
        );
        tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_worker_started", target_id=target);
        true
    }

    pub(crate) fn finish_refresh(&self, target: &str) -> bool {
        let mut refreshes = self.refreshes.lock().expect("Session refresh lock");
        if refreshes.get(target) == Some(&RefreshAdmission::Running { invalidated: true }) {
            refreshes.insert(
                target.to_owned(),
                RefreshAdmission::Running { invalidated: false },
            );
            tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_followup", target_id=target);
            true
        } else {
            refreshes.remove(target);
            tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_worker_finished", target_id=target);
            false
        }
    }

    /// Share the exact same guarded cleanup between event reconciliation and Resume.
    /// Restrict explicit Resume to its selected terminal; unrelated work is untouched.
    async fn retire_dead_panes(
        &self,
        client: &TmuxClient,
        target: &str,
        observation: &TargetObservation,
        selected_session: Option<&str>,
    ) -> Result<bool> {
        // Both refresh and explicit Resume hold the target write gate.
        // Completion is saved before removing the final pane, preventing replay
        // if that removal also terminates the tmux server.
        let dead: Vec<_> = observation.panes.iter()
            .filter(|(session, _, dead)| *dead && selected_session.is_none_or(|id| id == session))
            .cloned().collect();
        for (session_id, pane_id, _) in &dead {
            let Some(session) = observation.sessions.iter().find(|s| &s.id == session_id) else { continue };
            let Some(created) = session.created_unix_seconds else { continue };
            let Some(generation) = observation.generation.clone() else { continue };
            let marker = observation.markers.get(session_id).cloned().flatten();
            if observation.panes.iter().filter(|(id, _, _)| id == session_id).all(|(_, _, dead)| *dead) {
                for mut record in self.snapshot().await.sessions {
                    if record.execution_target_id.as_str() == target && record.server_generation == generation
                        && record.runtime_tmux_session_id.as_str() == session_id
                        && marker.as_deref() == Some(record.id.to_string().as_str()) {
                        record.process_completed = true;
                        self.save_session(record).await?;
                    }
                }
            }
            let binding = polar_tmux_core::TmuxSessionBinding { session_id: session_id.clone(), server_generation: generation,
                created_unix_seconds: created, managed_marker: marker };
            client.kill_bound_dead_pane(&binding, pane_id).await?;
        }
        Ok(!dead.is_empty())
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "session_observation_reconcile",
        skip_all,
        fields(component = "Sessions")
    )]
    async fn reconcile_observation(
        &self,
        client: &TmuxClient,
        target: &str,
        mut observation: TargetObservation,
    ) -> Result<TargetObservation> {
        if self.retire_dead_panes(client, target, &observation, None).await? {
            observation = TargetObservation::read(client).await?;
        }
        let mut changed = Vec::new();
        let mut records = self.snapshot().await.sessions;
        records.sort_by_key(|r| (r.created_unix_seconds, r.reference()));
        let mut originals = std::collections::BTreeSet::new();
        // Existing live duplicates are preserved. A lost duplicate must never
        // allocate another terminal during automatic recovery.
        for record in &records {
            if record.execution_target_id.as_str() == target && observation.decision(record) == SessionRecoveryDecision::Adopt {
                if let Some(key) = original_key(&record.launch.process) { originals.insert(key); }
            }
        }
        for mut record in records {
            if record.execution_target_id.as_str() != target {
                continue;
            }
            if let Some(key) = original_key(&record.launch.process) {
                if observation.decision(&record) == SessionRecoveryDecision::Restore && !originals.insert(key) {
                    record.status = SessionRecoveryStatus::Manual;
                    changed.push(record);
                    continue;
                }
            }
            self.recover_session(client, &mut record, &mut observation)
                .await?;
            record.created_unix_seconds =
                Self::binding_from_snapshot(&observation, Some(&record), &record.reference())
                    .ok()
                    .flatten()
                    .map(|binding| binding.created_unix_seconds);
            changed.push(record);
        }
        self.save_sessions(changed).await?;
        Ok(observation)
    }

    async fn recover_session(
        &self,
        client: &TmuxClient,
        record: &mut ManagedSessionRecord,
        observation: &mut TargetObservation,
    ) -> Result<()> {
        if record.status == SessionRecoveryStatus::Ended {
            return Ok(());
        }
        let generation = &observation.generation;
        let sessions = &observation.sessions;
        let marker = record.id.to_string();
        let matches = sessions
            .iter()
            .filter(|session| {
                observation
                    .markers
                    .get(&session.id)
                    .and_then(Option::as_deref)
                    == Some(marker.as_str())
            })
            .collect::<Vec<_>>();
        match observation.decision(record) {
            SessionRecoveryDecision::KeepEnded => {}
            SessionRecoveryDecision::Adopt => {
                let session = matches[0];
                record.runtime_tmux_session_id = StableTmuxSessionId::new(&session.id)?;
                record.server_generation =
                    generation.clone().context("observed server disappeared")?;
                record.launch.session_name = session.name.clone();
                record.status = SessionRecoveryStatus::Ready;
            }
            SessionRecoveryDecision::Conflict => record.status = SessionRecoveryStatus::Conflict,
            SessionRecoveryDecision::End => record.status = SessionRecoveryStatus::Ended,
            SessionRecoveryDecision::Manual => record.status = SessionRecoveryStatus::Manual,
            SessionRecoveryDecision::Restore => {
                let command = restore_command(&record.launch.process);
                let session = client
                    .create_managed_session(
                        &record.launch.session_name,
                        record.launch.working_directory.as_deref(),
                        command.as_deref(),
                        &record.id.to_string(),
                    )
                    .await?;
                let created_generation = client
                    .server_generation()
                    .await?
                    .context("recreated server disappeared")?;
                anyhow::ensure!(
                    observation
                        .generation
                        .as_ref()
                        .is_none_or(|generation| generation == &created_generation),
                    "tmux server changed during restore"
                );
                observation.generation = Some(created_generation.clone());
                observation.markers.insert(session.id.clone(), Some(marker));
                record.runtime_tmux_session_id = StableTmuxSessionId::new(&session.id)?;
                record.server_generation = created_generation;
                observation.sessions.push(session);
                record.status = SessionRecoveryStatus::Ready;
            }
        }
        Ok(())
    }

    pub(crate) async fn list_sessions(&self, target: &str) -> (Vec<SessionFact>, bool) {
        self.observations
            .lock()
            .await
            .get(target)
            .map(|observed| (observed.facts.clone(), observed.available))
            .unwrap_or_default()
    }

    pub(crate) async fn observe_attached_clients(
        &self,
        target: &str,
        reference: &str,
        binding: &polar_tmux_core::TmuxSessionBinding,
        transport: Option<&polar_tmux_protocol::SshTransportGenerationV1>,
        count: u32,
    ) {
        let mut observations = self.observations.lock().await;
        let Some(observed) = observations.get_mut(target) else {
            return;
        };
        if observed.transport.as_ref() != transport
            || observed.snapshot.generation.as_deref() != Some(&binding.server_generation)
            || !observed.snapshot.sessions.iter().any(|s| {
                s.id == binding.session_id
                    && s.created_unix_seconds == Some(binding.created_unix_seconds)
            })
        {
            return;
        }
        if let Some(fact) = observed
            .facts
            .iter_mut()
            .find(|f| f.identity.tmux_session_id.as_str() == reference)
        {
            if fact.attached_clients != count {
                fact.attached_clients = count;
                // Reuse the lifecycle event channel, with no remote observation.
                let _ = self.refresh_requests.send(SessionRefreshRequest::Project(
                    fact.identity.execution_target_id.clone(),
                ));
            }
        }
    }

    pub(crate) async fn mark_unavailable(&self, target: &str) {
        let mut observations = self.observations.lock().await;
        let Some(observed) = observations.get_mut(target) else {
            return;
        };
        observed.available = false;
        for fact in &mut observed.facts {
            fact.transport_status = TransportStatus::Offline;
            fact.replacement = None;
        }
    }

    pub(crate) async fn observation_status(&self, target: &str) -> Option<(bool, Option<polar_tmux_core::TmuxErrorKind>)> {
        self.observations.lock().await.get(target).map(|o| (o.available, o.failure_kind))
    }

    pub(crate) async fn mark_observation_failed(&self, target: &str, error: &anyhow::Error) {
        let mut observations = self.observations.lock().await;
        let observed = observations.entry(target.into()).or_insert_with(|| ObservedTarget {
            facts: Vec::new(), available: false, failure_kind: None, transport: None,
            snapshot: TargetObservation { generation: None, sessions: Vec::new(), markers: BTreeMap::new(), clients: Vec::new(), panes: Vec::new() },
        });
        observed.available = false;
        observed.failure_kind = error.downcast_ref::<polar_tmux_core::TmuxError>().map(|e| e.kind());
        for fact in &mut observed.facts { fact.replacement = None; }
    }

    /// Normalize one verified target observation for both the list and terminal feed.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(target = "polar_station_operation", name = "session_observation", skip_all,
        fields(component="Sessions", target_id=target.as_str(), outcome="cancelled"))]
    pub(crate) async fn refresh_target(
        &self,
        client: &TmuxClient,
        target: &ExecutionTarget,
        label: &str,
        registry: &SessionRegistry,
        transport: Option<polar_tmux_protocol::SshTransportGenerationV1>,
        ssh: &crate::ssh_connection_manager::SshConnectionManager,
        gate: &tokio::sync::RwLock<()>,
    ) -> Result<Vec<String>> {
        let mut read = Some(gate.read().instrument(tracing::info_span!(
            target: "polar_station_operation", "session_observation_gate_wait", component="Sessions"
        )).await);
        // An invalidation received while awaiting lease/gate is covered by the
        // query about to start. Only changes during or after its I/O need another.
        {
            let mut refreshes = self.refreshes.lock().expect("Session refresh lock");
            if let Some(RefreshAdmission::Running { invalidated }) =
                refreshes.get_mut(target.as_str())
            {
                if std::mem::take(invalidated) {
                    tracing::info!(target: "polar_station_operation", component="Sessions", event="refresh_invalidation_covered", target_id=target.as_str());
                }
            }
        }
        let mut observation = TargetObservation::read(client).await.inspect_err(|_| {
            tracing::Span::current().record("outcome", "failed");
        })?;
        let needs_restore = observation.panes.iter().any(|(_, _, dead)| *dead) || self.snapshot().await.sessions.iter().any(|record| {
            record.execution_target_id.as_str() == target.as_str()
                && (observation.decision(record) == SessionRecoveryDecision::Restore
                    || (record.process_completed && record.status == SessionRecoveryStatus::Ready))
        });
        let _write = if needs_restore {
            drop(read.take());
            let write = gate.write().await;
            // A mutation may have won the upgrade; re-observe before deciding work recovery.
            observation = TargetObservation::read(client).await?;
            Some(write)
        } else {
            None
        };
        let observation = self
            .reconcile_observation(client, target.as_str(), observation)
            .await?;
        let observed_server = &observation.generation;
        if self
            .observations
            .lock()
            .await
            .get(target.as_str())
            .map(|o| &o.snapshot.generation)
            != Some(observed_server)
        {
            registry
                .mark_target_lifecycle(target, SessionLifecycleV1::Recovering, transport.clone())
                .await?;
        }
        let snapshot = observation.clone();
        let generation = observation.generation;
        let sessions = observation.sessions;
        let observed_sessions = sessions.clone();
        let clients = observation.clients;
        let runtime = self.snapshot().await;
        let section = if target.is_local() {
            SessionSection::Local
        } else {
            SessionSection::Remote
        };
        let target_id = ExecutionTargetId::new(target.to_string())?;
        let mut facts = Vec::new();
        let mut discovered = Vec::new();
        let mut monitors = Vec::new();
        for session in sessions {
            let marker = observation
                .markers
                .get(&session.id)
                .and_then(Option::as_deref);
            let record = runtime.sessions.iter().find(|record| {
                record.execution_target_id == target_id
                    && marker == Some(record.id.to_string().as_str())
            });
            // A conflicting or ended managed identity is never exposed as an external Ready.
            if record.is_some_and(|record| {
                record.status != SessionRecoveryStatus::Ready
                    || generation.as_deref() != Some(record.server_generation.as_str())
            }) {
                continue;
            }
            let identity =
                record.map_or_else(|| session.id.clone(), ManagedSessionRecord::reference);
            facts.push(SessionFact {
                identity: SessionKey::new(target_id.clone(), StableTmuxSessionId::new(&identity)?),
                session_kind: SessionKind::from_terminal_name(&session.name).map_or(SessionKind::Terminal, |(kind, _)| kind),
                original_agent_id: SessionKind::from_terminal_name(&session.name).map(|(_, id)| id.to_owned()),
                display_name: session.name.clone(),
                target_label: label.to_owned(),
                section,
                attached_clients: clients
                    .iter()
                    .filter(|c| c.session_id == session.id && !c.control_mode)
                    .count() as u32,
                transport_status: TransportStatus::Online,
                recovery_status: record.map(|record| record.status),
                replacement: None,
            });
            discovered.push(DiscoveredSession::new(&identity, session.name)?);
            monitors.push(identity);
        }
        for record in runtime
            .sessions
            .iter()
            .filter(|r| r.execution_target_id == target_id)
        {
            if facts
                .iter()
                .any(|f| f.identity.tmux_session_id.as_str() == record.reference())
            {
                continue;
            }
            let lifecycle = match record.status {
                SessionRecoveryStatus::Ended => SessionLifecycleV1::Ended,
                SessionRecoveryStatus::Recovering => SessionLifecycleV1::Recovering,
                SessionRecoveryStatus::Ending => SessionLifecycleV1::Ending,
                _ => SessionLifecycleV1::Unavailable,
            };
            discovered.push(
                DiscoveredSession::new(record.reference(), &record.launch.session_name)?
                    .with_lifecycle(lifecycle),
            );
            if record.status != SessionRecoveryStatus::Ended {
                facts.push(SessionFact {
                    identity: SessionKey::new(
                        target_id.clone(),
                        StableTmuxSessionId::new(record.reference())?,
                    ),
                    display_name: record.launch.session_name.clone(),
                    session_kind: SessionKind::from_restore_spec(&record.launch.process),
                    original_agent_id: record.launch.process.original_agent_id().map(str::to_owned),
                    target_label: label.to_owned(),
                    section,
                    attached_clients: 0,
                    transport_status: if record.status == SessionRecoveryStatus::Ending && observed_sessions.iter().any(|s|
                        s.id == record.runtime_tmux_session_id.as_str() && generation.as_deref() == Some(record.server_generation.as_str())) {
                        TransportStatus::Online
                    } else { TransportStatus::Offline },
                    recovery_status: Some(record.status),
                    replacement: if record.status == SessionRecoveryStatus::Conflict
                        && generation.as_deref() != Some(record.server_generation.as_str())
                    {
                        let named = observed_sessions
                            .iter()
                            .filter(|session| session.name == record.launch.session_name)
                            .collect::<Vec<_>>();
                        if named.len() == 1
                            && observation
                                .markers
                                .get(&named[0].id)
                                .is_some_and(Option::is_none)
                        {
                            named[0].created_unix_seconds.zip(generation.as_ref()).map(
                                |(created_unix_seconds, server_generation)| {
                                    polar_tmux_protocol::SessionReplacementV1 {
                                        execution_target_id: target.to_string(),
                                        tmux_session_id: named[0].id.clone(),
                                        server_generation: server_generation.clone(),
                                        created_unix_seconds,
                                    }
                                },
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    },
                });
            }
        }
        anyhow::ensure!(
            transport == ssh.transport_generation(target.as_str()).await,
            "Session transport changed during observation"
        );
        self.observations.lock().await.insert(
            target.to_string(),
            ObservedTarget {
                facts,
                available: true,
                failure_kind: None,
                snapshot,
                transport: transport.clone(),
            },
        );
        registry
            .replace_target_sessions_with_generation(target, discovered, transport)
            .instrument(tracing::info_span!(target: "polar_station_operation", "session_observation_publish", component="Sessions"))
            .await?;
        self.observation_published.notify_waiters();
        tracing::Span::current().record("outcome", "ok");
        Ok(monitors)
    }

    /// Explicit replacement never adopts a marker or executes the saved recipe.
    /// The native owner acknowledges its persisted binding before `retire`.
    pub(crate) async fn use_existing_session(
        &self,
        client: &TmuxClient,
        session: &SessionKey,
        replacement: &polar_tmux_protocol::SessionReplacementV1,
        retire: bool,
    ) -> Result<()> {
        let record = self
            .record_for(
                session.execution_target_id.as_str(),
                session.tmux_session_id.as_str(),
            )
            .await
            .context("saved Session missing")?;
        anyhow::ensure!(
            replacement.execution_target_id == session.execution_target_id.as_str(),
            "replacement target changed"
        );
        anyhow::ensure!(
            matches!(
                record.status,
                SessionRecoveryStatus::Conflict | SessionRecoveryStatus::Ended
            ),
            "saved Session is not a lost conflict"
        );
        let observation = TargetObservation::read(client).await?;
        anyhow::ensure!(
            observation.generation.as_deref() == Some(replacement.server_generation.as_str())
                && replacement.server_generation != record.server_generation,
            "replacement server changed"
        );
        anyhow::ensure!(
            !observation
                .markers
                .values()
                .any(|marker| marker.as_deref() == Some(record.id.to_string().as_str())),
            "original Session still exists"
        );
        let named = observation
            .sessions
            .iter()
            .filter(|current| current.name == record.launch.session_name)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            named.len() == 1
                && named[0].id == replacement.tmux_session_id
                && named[0].created_unix_seconds == Some(replacement.created_unix_seconds),
            "replacement Session changed"
        );
        anyhow::ensure!(
            observation
                .markers
                .get(&replacement.tmux_session_id)
                .is_some_and(Option::is_none),
            "replacement ownership changed"
        );
        if retire && record.status != SessionRecoveryStatus::Ended {
            // Existing durable Ended state is the tombstone: no kill or recipe transfer.
            self.end(&session.execution_target_id, &record.reference())
                .await?;
            self.request_refresh(session.execution_target_id.clone());
        }
        Ok(())
    }

    pub(crate) async fn rename_session(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
        name: String,
    ) -> Result<()> {
        let id = self.resolve(client, target, reference).await?;
        let original = client.list_sessions().await?.into_iter().find(|s| s.id == id).context("Session missing")?;
        if SessionKind::from_terminal_name(&original.name).is_some() {
            let window = client.list_windows(&id).await?.into_iter().next().context("Session has no window")?;
            client.rename_window(&window.id, &name).await?;
            self.request_refresh(ExecutionTargetId::new(target)?);
            return Ok(());
        }
        client.rename_session(&id, &name).await?;
        if let Some(mut record) = self.record_for(target, reference).await {
            record.launch.session_name = name;
            self.save_session(record).await?;
        }
        self.request_refresh(ExecutionTargetId::new(target)?);
        Ok(())
    }

    pub(crate) async fn end_session(
        &self,
        client: &TmuxClient,
        target: &ExecutionTargetId,
        reference: &str,
    ) -> Result<()> {
        self.end_session_bound(client, target, reference, None).await
    }

    pub(crate) async fn mark_ending(&self, target: &str, reference: &str) -> Result<()> {
        if let Some(mut record) = self.record_for(target, reference).await {
            record.status = SessionRecoveryStatus::Ending;
            self.save_session(record).await?;
        }
        Ok(())
    }

    async fn end_session_bound(&self, client: &TmuxClient, target: &ExecutionTargetId,
        reference: &str, expected: Option<&polar_tmux_core::TmuxSessionBinding>) -> Result<()>
    {
        let prepared = self.prepare_end(client, target.as_str(), reference, target.as_str()).await?;
        if let Some(expected) = expected {
            anyhow::ensure!(*expected == prepared.binding, "Session identity changed; confirm the original Session again");
        }
        self.mark_ending(target.as_str(), reference).await?;
        let attempted = client.kill_bound_session(&prepared.binding).await;
        let after = client.observe_sessions().await?;
        anyhow::ensure!(!after.sessions.iter().any(|(session, _)|
            after.generation.as_deref() == Some(prepared.binding.server_generation.as_str())
            && session.id == prepared.binding.session_id
            && session.created_unix_seconds == Some(prepared.binding.created_unix_seconds)), "The original Session is still present; refresh before retrying");
        // Fresh absence resolves even a lost final reply from an exiting tmux server.
        let _ = attempted;
        if self.record_for(target.as_str(), reference).await.is_some() { self.end(target, reference).await?; }
        self.request_refresh(target.clone());
        Ok(())
    }

    pub(crate) async fn prepare_end(&self, client: &TmuxClient, target: &str,
        reference: &str, label: &str) -> Result<super::session_fact_provider::PreparedSessionEnd>
    {
        let record = self.record_for(target, reference).await;
        let snapshot = client.observe_sessions().await?;
        let original = record.as_ref().map_or(reference, |r| r.runtime_tmux_session_id.as_str());
        let (session, marker) = snapshot.sessions.iter().find(|(s, _)| s.id == original)
            .context("The original Session is no longer present; refresh Sessions")?;
        let generation = snapshot.generation.context("The original tmux server is unavailable")?;
        let created = session.created_unix_seconds.context("Session creation identity is unavailable")?;
        if let Some(record) = &record {
            anyhow::ensure!(matches!(record.status, SessionRecoveryStatus::Ready | SessionRecoveryStatus::Ending)
                && record.server_generation == generation
                && record.created_unix_seconds == Some(created)
                && marker.as_deref() == Some(record.id.to_string().as_str()),
                "Session identity changed; do not end a replacement Session");
        } else {
            anyhow::ensure!(reference.starts_with('$') && !reference.starts_with("station-session-"), "Session identity is unavailable");
        }
        let binding = polar_tmux_core::TmuxSessionBinding { session_id: session.id.clone(),
            server_generation: generation, created_unix_seconds: created, managed_marker: marker.clone() };
        let count = snapshot.clients.iter().filter(|c| c.session_id == session.id && !c.control_mode).count() as u32;
        Ok(super::session_fact_provider::PreparedSessionEnd {
            binding,
            preview: polar_tmux_protocol::SessionEndPreviewV1 {
                session: Some(polar_tmux_protocol::SessionIdentityV1 { execution_target_id: target.into(), tmux_session_id: reference.into() }),
                name: session.name.clone(), target_label: label.into(), attached_clients: count, codex: None,
                confirmation: polar_tmux_protocol::ControlConfirmationV1 { id: "pending-preview".into(),
                    title: "Kill Session?".into(), message: format!("End “{}” on {}? All {} connected terminals will disconnect. Its Pane views will close; other Sessions are kept.", session.name, label, count),
                    confirm_label: "Kill Session".into(), destructive: true },
            },
        })
    }

    pub(crate) async fn begin_operation(
        &self,
        key: String,
        fingerprint: String,
    ) -> Result<OperationAdmission> {
        let mut state = self.state.lock().await;
        if let Some(receipt) = state.operation_receipts.iter().find(|r| r.key == key) {
            return Ok(if receipt.fingerprint != fingerprint {
                OperationAdmission::Conflict
            } else if let Some(response) = &receipt.response {
                OperationAdmission::Completed(response.clone())
            } else {
                OperationAdmission::Unknown
            });
        }
        let mut next = state.clone();
        next.operation_receipts.push(SessionOperationReceipt {
            key,
            fingerprint,
            response: None,
        });
        self.store.save_runtime(&next)?;
        *state = next;
        Ok(OperationAdmission::Started)
    }

    pub(crate) async fn complete_operation(&self, key: &str, response: String) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut next = state.clone();
        next.operation_receipts
            .iter_mut()
            .find(|r| r.key == key)
            .context("operation receipt missing")?
            .response = Some(response);
        self.store.save_runtime(&next)?;
        *state = next;
        Ok(())
    }

    pub(crate) async fn create(
        &self,
        client: &TmuxClient,
        target: ExecutionTargetId,
        launch: SessionLaunchSpec,
    ) -> Result<ManagedSessionRecord> {
        self.create_with_command(client, target, launch, None).await
    }

    pub(crate) async fn create_with_command(
        &self,
        client: &TmuxClient,
        target: ExecutionTargetId,
        launch: SessionLaunchSpec,
        initial_command: Option<String>,
    ) -> Result<ManagedSessionRecord> {
        self.create_with_public_name(client, target, launch, initial_command, false).await
    }

    pub(crate) async fn create_with_public_name(
        &self, client: &TmuxClient, target: ExecutionTargetId, mut launch: SessionLaunchSpec,
        initial_command: Option<String>, reuse_public_name: bool,
    ) -> Result<ManagedSessionRecord> {
        launch.validate()?;
        anyhow::ensure!(initial_command.is_some() || !matches!(&launch.process, ProcessRestoreSpec::CodexForwardedCli { .. }),
            "A forwarded local CLI requires a live, owned socket at creation time");
        let id = ManagedSessionId::new();
        let command = initial_command.or_else(||match &launch.process {
            ProcessRestoreSpec::CodexAppServer { .. } => restore_command(&launch.process),
            ProcessRestoreSpec::CodexForwardedCli { .. } => None,
            ProcessRestoreSpec::Command { command, .. } => Some(command.clone()),
            ProcessRestoreSpec::Shell => None,
            ProcessRestoreSpec::Claude { .. } => restore_command(&launch.process),
            ProcessRestoreSpec::Codex { .. } => {
                None
            }
        });
        anyhow::ensure!(!matches!(&launch.process, ProcessRestoreSpec::Codex { .. }), "Codex creation uses its explicit resume flow");
        let session = if reuse_public_name {
            client.create_unique_managed_session(&launch.session_name, launch.working_directory.as_deref(),
                command.as_deref(), &id.to_string()).await?
        } else {
            client.create_managed_session(&launch.session_name, launch.working_directory.as_deref(), command.as_deref(), &id.to_string()).await?
        };
        // Another Station may have won allocation. Adopt its durable marker and
        // observed cwd/name; never respawn or rename the existing process.
        let allocated_id = id;
        let marker = client.managed_session_marker(&session.id).await?;
        anyhow::ensure!(!reuse_public_name || marker.is_some(),
            "An external terminal won allocation; refresh and connect to its existing public name");
        let id = marker.map(|value|ManagedSessionId::parse(&value)).transpose()?.unwrap_or(id);
        launch.session_name = session.name.clone();
        if reuse_public_name && id != allocated_id {
            launch.working_directory = client.list_panes(&session.id).await?.first().map(|p|p.current_path.clone());
        }
        if launch.working_directory.is_none() {
            launch.working_directory = client
                .list_panes(&session.id)
                .await?
                .first()
                .map(|pane| pane.current_path.clone())
                .filter(|path| !path.is_empty());
        }
        let record = ManagedSessionRecord {
            id,
            execution_target_id: target,
            launch,
            created_unix_seconds: session.created_unix_seconds,
            runtime_tmux_session_id: StableTmuxSessionId::new(&session.id)?,
            server_generation: client
                .server_generation()
                .await?
                .context("created server disappeared")?,
            status: SessionRecoveryStatus::Ready,
            process_completed: false,
        };
        self.save_session(record.clone()).await?;
        // The target operation write gate excludes an older observation commit.
        // Merge the exact create response into the existing canonical observation.
        let mut observations = self.observations.lock().await;
        let observed = observations
            .entry(record.execution_target_id.to_string())
            .or_insert_with(|| ObservedTarget {
                facts: Vec::new(),
                available: true,
                failure_kind: None,
                snapshot: TargetObservation {
                    generation: Some(record.server_generation.clone()),
                    sessions: Vec::new(),
                    markers: BTreeMap::new(),
                    clients: Vec::new(),
                    panes: Vec::new(),
                },
                transport: None,
            });
        if observed.snapshot.generation.as_deref() != Some(record.server_generation.as_str()) {
            observed.facts.clear();
            observed.snapshot = TargetObservation {
                generation: Some(record.server_generation.clone()),
                sessions: Vec::new(),
                markers: BTreeMap::new(),
                clients: Vec::new(),
                    panes: Vec::new(),
            };
        }
        observed
            .snapshot
            .markers
            .insert(session.id.clone(), Some(record.id.to_string()));
        observed.snapshot.sessions.retain(|s| s.id != session.id);
        observed.snapshot.sessions.push(session);
        drop(observations);
        self.observation_published.notify_waiters();
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_create_committed", target_id=record.execution_target_id.as_str(), session_id=record.reference());
        Ok(record)
    }
    /// Resolve a convention-named terminal from current tmux facts, including
    /// an external terminal that has not reached the local observation cache yet.
    pub(crate) async fn existing_public_session(
        &self, client: &TmuxClient, target: &str, name: &str,
    ) -> Result<Option<SessionFact>> {
        let Some((kind, original)) = SessionKind::from_terminal_name(name) else { return Ok(None); };
        let mut observed = TargetObservation::read(client).await?;
        let Some(selected) = observed.sessions.iter().find(|s| s.name == name) else { return Ok(None); };
        if self.retire_dead_panes(client, target, &observed, Some(&selected.id)).await? {
            observed = TargetObservation::read(client).await?;
            self.request_refresh(ExecutionTargetId::new(target)?);
        }
        let Some(session) = observed.sessions.iter().find(|s| s.name == name) else { return Ok(None); };
        anyhow::ensure!(observed.panes.iter().any(|(id, _, dead)| id == &session.id && !dead),
            "Existing terminal has no verified live pane; refresh before retrying");
        let marker = observed.markers.get(&session.id).and_then(|value| value.as_deref());
        let runtime = self.snapshot().await;
        let record = runtime.sessions.iter().find(|r| r.execution_target_id.as_str() == target
            && marker == Some(r.id.to_string().as_str()));
        if let Some(record) = record {
            anyhow::ensure!(record.status == SessionRecoveryStatus::Ready && observed.generation.as_deref() == Some(&record.server_generation),
                "Existing terminal is awaiting recovery; no duplicate was created");
        }
        let id = record.map_or_else(|| session.id.clone(), ManagedSessionRecord::reference);
        let target_id = ExecutionTargetId::new(target)?;
        self.request_refresh(target_id.clone());
        Ok(Some(SessionFact {
            identity: SessionKey::new(target_id.clone(), StableTmuxSessionId::new(id)?),
            display_name: session.name.clone(), session_kind: kind, original_agent_id: Some(original.into()),
            target_label: target.into(),
            section: if target_id == ExecutionTargetId::local() { SessionSection::Local } else { SessionSection::Remote },
            attached_clients: observed.clients.iter().filter(|c| c.session_id == session.id && !c.control_mode).count() as u32,
            transport_status: TransportStatus::Online, recovery_status: record.map(|r| r.status), replacement: None,
        }))
    }

    pub(super) async fn created_fact(
        &self,
        record: &ManagedSessionRecord,
        label: &str,
        transport: Option<polar_tmux_protocol::SshTransportGenerationV1>,
    ) -> SessionFact {
        let fact = SessionFact {
            identity: SessionKey::new(
                record.execution_target_id.clone(),
                StableTmuxSessionId::new(record.reference()).expect("managed reference"),
            ),
            display_name: record.launch.session_name.clone(),
            session_kind: SessionKind::from_restore_spec(&record.launch.process),
            original_agent_id: record.launch.process.original_agent_id().map(str::to_owned),
            target_label: label.to_owned(),
            section: if record.execution_target_id == ExecutionTargetId::local() {
                SessionSection::Local
            } else {
                SessionSection::Remote
            },
            attached_clients: 0,
            transport_status: TransportStatus::Online,
            recovery_status: Some(record.status),
            replacement: None,
        };
        if let Some(observed) = self
            .observations
            .lock()
            .await
            .get_mut(record.execution_target_id.as_str())
        {
            observed.facts.retain(|f| f.identity != fact.identity);
            observed.facts.push(fact.clone());
            observed.available = true;
            observed.failure_kind = None;
            observed.transport = transport;
        }
        tracing::info!(target: "polar_station_operation", component="Sessions", event="session_create_fact_published", target_id=record.execution_target_id.as_str(), session_id=record.reference());
        fact
    }

    /// Explicit execution, serialized by the caller's target operation gate.
    pub(crate) async fn restart(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
    ) -> Result<()> {
        let mut record = self
            .record_for(target, reference)
            .await
            .context("saved Session is unknown")?;
        anyhow::ensure!(
            record.status == SessionRecoveryStatus::Manual,
            "Session is not awaiting manual recovery"
        );
        anyhow::ensure!(!matches!(&record.launch.process, ProcessRestoreSpec::CodexForwardedCli { .. }),
            "A forwarded CLI plane must be reopened with a fresh SSH socket lease; the original Codex Thread is unchanged");
        if let Some(key) = original_key(&record.launch.process) {
            anyhow::ensure!(!self.snapshot().await.sessions.iter().any(|other| other.id != record.id
                && other.execution_target_id == record.execution_target_id
                && other.status != SessionRecoveryStatus::Ended && original_key(&other.launch.process).as_ref() == Some(&key)),
                "This original conversation already has a terminal on this Host; use that Session instead");
        }
        let sessions = client.list_sessions().await?;
        for session in sessions {
            let marker = client.managed_session_marker(&session.id).await?;
            anyhow::ensure!(
                session.name != record.launch.session_name
                    && marker.as_deref() != Some(record.id.to_string().as_str()),
                "Session already exists; refresh before running the saved command"
            );
        }
        record.launch.validate()?;
        let command = restore_command(&record.launch.process);
        let session = client
            .create_managed_session(
                &record.launch.session_name,
                record.launch.working_directory.as_deref(),
                command.as_deref(),
                &record.id.to_string(),
            )
            .await?;
        record.created_unix_seconds = session.created_unix_seconds;
        record.runtime_tmux_session_id = StableTmuxSessionId::new(session.id)?;
        record.server_generation = client
            .server_generation()
            .await?
            .context("created server disappeared")?;
        record.process_completed = false;
        record.status = SessionRecoveryStatus::Ready;
        self.save_session(record).await
    }

    pub(crate) async fn observe_completion(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
    ) -> Result<()> {
        // The monitor contributes only an invalidation. The shared refresh
        // verifies generation, ownership and pane death under the target gate.
        let _ = (client, reference);
        self.request_refresh(ExecutionTargetId::new(target)?);
        Ok(())
    }

    pub(crate) fn open(store: SessionStore) -> Result<Self> {
        Ok(Self {
            state: Mutex::new(store.load_runtime_or_initialize()?),
            store,
            observations: Mutex::new(BTreeMap::new()),
            observation_published: tokio::sync::Notify::new(),
            refreshes: StdMutex::new(BTreeMap::new()),
            refresh_requests: tokio::sync::broadcast::channel(64).0,
            
        })
    }

    

    pub(crate) fn subscribe_refresh_requests(
        &self,
    ) -> tokio::sync::broadcast::Receiver<SessionRefreshRequest> {
        self.refresh_requests.subscribe()
    }

    pub(crate) async fn snapshot(&self) -> SessionRuntimeState {
        self.state.lock().await.clone()
    }

    pub(crate) async fn save_session(&self, record: ManagedSessionRecord) -> Result<()> {
        self.save_sessions(vec![record]).await
    }

    async fn save_sessions(&self, records: Vec<ManagedSessionRecord>) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut next = state.clone();
        for mut record in records {
            if let Some(current) = next.sessions.iter().find(|current| {
                current.id == record.id && current.server_generation == record.server_generation
            }) {
                record.process_completed |= current.process_completed;
            }
            if next.sessions.iter().any(|current| current == &record) {
                continue;
            }
            if let Some(existing) = next.sessions.iter_mut().find(|s| s.id == record.id) {
                *existing = record;
            } else {
                next.sessions.push(record);
            }
        }
        if next.sessions == state.sessions {
            return Ok(());
        }
        next.revision = next.revision.saturating_add(1);
        next.validate()?;
        self.store.save_runtime(&next)?;
        *state = next;
        Ok(())
    }

    pub(crate) async fn record_for(
        &self,
        target: &str,
        reference: &str,
    ) -> Option<ManagedSessionRecord> {
        self.state
            .lock()
            .await
            .sessions
            .iter()
            .find(|s| s.execution_target_id.as_str() == target && s.reference() == reference)
            .cloned()
    }

    pub(crate) async fn managed_reference(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
    ) -> Result<String> {
        if reference.starts_with("station-session-") {
            return Ok(reference.to_owned());
        }
        let session = client
            .list_sessions()
            .await?
            .into_iter()
            .find(|s| s.id == reference || s.name == reference)
            .context("Session missing")?;
        let marker = client.managed_session_marker(&session.id).await?;
        Ok(self
            .snapshot()
            .await
            .sessions
            .iter()
            .find(|r| {
                r.execution_target_id.as_str() == target
                    && marker.as_deref() == Some(r.id.to_string().as_str())
            })
            .map_or(session.id, ManagedSessionRecord::reference))
    }

    /// Read the shared observation; actual tmux attachment checks this identity
    /// in the same server command queue as attach-session. No per-Pane queries.
    /// Wait on facts, including an empty observation that emits no Session feed
    /// change. No lease, operation gate or retry timer is held while waiting.
    pub(crate) async fn attachment_observation_current(
        &self,
        target: &str,
        generation: Option<&polar_tmux_protocol::SshTransportGenerationV1>,
    ) -> bool {
        self.observations
            .lock()
            .await
            .get(target)
            .is_some_and(|observed| observed.available && observed.transport.as_ref() == generation)
    }

    pub(crate) async fn wait_for_attachment_observation(
        &self,
        target: &str,
        ssh: &crate::ssh_connection_manager::SshConnectionManager,
    ) {
        let mut requested = false;
        loop {
            let published = self.observation_published.notified();
            tokio::pin!(published);
            published.as_mut().enable();
            let generation = ssh.transport_generation(target).await;
            let observations = self.observations.lock().await;
            if observations
                .get(target)
                .is_some_and(|observed| observed.available && observed.transport == generation)
            {
                return;
            }
            if !requested {
                if let Ok(target) = ExecutionTargetId::new(target) {
                    self.request_refresh_if_idle(target);
                }
                requested = true;
            }
            drop(observations);
            published.await;
        }
    }

    #[tracing::instrument(
        target = "polar_station_operation",
        name = "terminal_identity_resolve",
        skip_all,
        fields(component = "Sessions")
    )]
    pub(crate) async fn attachment_binding(
        &self,
        target: &str,
        reference: &str,
        transport: Option<&polar_tmux_protocol::SshTransportGenerationV1>,
    ) -> Result<Option<polar_tmux_core::TmuxSessionBinding>> {
        let record = self.record_for(target, reference).await;
        if record
            .as_ref()
            .is_some_and(|r| r.status == SessionRecoveryStatus::Ended)
        {
            return Ok(None);
        }
        let observations = self.observations.lock().await;
        let observed = observations
            .get(target)
            .context("Session observation is pending")?;
        anyhow::ensure!(
            observed.transport.as_ref() == transport,
            "Session observation transport is stale"
        );
        anyhow::ensure!(observed.available, "Session observation is unavailable");
        Self::binding_from_snapshot(&observed.snapshot, record.as_ref(), reference)
    }

    /// A previous identity is only a hint for the existing remote guarded
    /// attach. Only its remote success receipt (or fresh observation) can
    /// authorize publication; the cached hint alone never does.
    pub(crate) async fn previous_attachment_binding(
        &self,
        target: &str,
        reference: &str,
    ) -> Option<polar_tmux_core::TmuxSessionBinding> {
        let record = self.record_for(target, reference).await;
        let observations = self.observations.lock().await;
        if let Some(observed) = observations.get(target) {
            // An observed conflict/end must never fall back to older stored facts.
            return Self::binding_from_snapshot(&observed.snapshot, record.as_ref(), reference)
                .ok()
                .flatten();
        }
        let record = record?;
        if record.status != SessionRecoveryStatus::Ready {
            return None;
        }
        let created_unix_seconds = record.created_unix_seconds.filter(|value| *value >= 0)?;
        tracing::info!(target: "polar_station_operation", component="Sessions", event="terminal_persisted_binding_hint");
        Some(polar_tmux_core::TmuxSessionBinding {
            session_id: record.runtime_tmux_session_id.to_string(),
            server_generation: record.server_generation,
            created_unix_seconds,
            managed_marker: Some(record.id.to_string()),
        })
    }

    fn binding_from_snapshot(
        snapshot: &TargetObservation,
        record: Option<&ManagedSessionRecord>,
        reference: &str,
    ) -> Result<Option<polar_tmux_core::TmuxSessionBinding>> {
        let id = if let Some(record) = record {
            anyhow::ensure!(
                record.status == SessionRecoveryStatus::Ready
                    && snapshot.generation.as_deref() == Some(record.server_generation.as_str()),
                "Session recovery is pending"
            );
            record.runtime_tmux_session_id.as_str()
        } else {
            anyhow::ensure!(
                !reference.starts_with("station-session-"),
                "managed Session is unknown"
            );
            reference
        };
        let Some(session) = snapshot
            .sessions
            .iter()
            .find(|s| s.id == id || (record.is_none() && s.name == id))
        else {
            return Ok(None);
        };
        let marker = snapshot.markers.get(&session.id).cloned().flatten();
        if let Some(record) = record {
            anyhow::ensure!(
                marker.as_deref() == Some(record.id.to_string().as_str()),
                "managed Session ownership changed"
            );
        }
        Ok(Some(polar_tmux_core::TmuxSessionBinding {
            session_id: session.id.clone(),
            server_generation: snapshot
                .generation
                .clone()
                .context("observed server is absent")?,
            created_unix_seconds: session
                .created_unix_seconds
                .context("observed Session creation is absent")?,
            managed_marker: marker,
        }))
    }

    

    pub(crate) async fn resolve(
        &self,
        client: &TmuxClient,
        target: &str,
        reference: &str,
    ) -> Result<String> {
        let Some(record) = self.record_for(target, reference).await else {
            anyhow::ensure!(
                !reference.starts_with("station-session-"),
                "managed Session is unknown"
            );
            // Exact physical IDs already discovered by the target owner do
            // not trigger another full target scan for each terminal consumer.
            let known = {
                let observations = self.observations.lock().await;
                observations.get(target).is_some_and(|observed| {
                    observed.available
                        && observed
                            .facts
                            .iter()
                            .any(|fact| fact.identity.tmux_session_id.as_str() == reference)
                })
            };
            if known && reference.starts_with('$') {
                let observed = self
                    .observations
                    .lock()
                    .await
                    .get(target)
                    .and_then(|observed| observed.snapshot.generation.clone());
                anyhow::ensure!(
                    observed.is_some() && client.server_generation().await? == observed,
                    "tmux server changed; Session observation is pending"
                );
                return Ok(reference.to_owned());
            }
            let session = client
                .list_sessions()
                .await?
                .into_iter()
                .find(|session| session.id == reference || session.name == reference)
                .context("Session is missing; select an existing Session to rebind this Pane")?;
            return Ok(session.id);
        };
        anyhow::ensure!(
            record.status == SessionRecoveryStatus::Ready,
            "managed Session is not ready"
        );
        let generation = client.server_generation().await?;
        anyhow::ensure!(
            generation.as_deref() == Some(record.server_generation.as_str()),
            "tmux server changed; Session recovery is pending"
        );
        let marker = client
            .managed_session_marker(record.runtime_tmux_session_id.as_str())
            .await?;
        anyhow::ensure!(
            marker.as_deref() == Some(&record.id.to_string()),
            "managed Session ownership changed"
        );
        Ok(record.runtime_tmux_session_id.to_string())
    }

    pub(crate) async fn end(&self, target: &ExecutionTargetId, reference: &str) -> Result<()> {
        let mut record = self
            .record_for(target.as_str(), reference)
            .await
            .context("managed Session not found")?;
        record.status = SessionRecoveryStatus::Ended;
        self.save_session(record).await
    }
}

fn original_key(process: &ProcessRestoreSpec) -> Option<(String, String, String)> {
    match process {
        ProcessRestoreSpec::CodexAppServer { owner_host_id, endpoint_id, thread_id, .. }
        | ProcessRestoreSpec::CodexForwardedCli { owner_host_id, endpoint_id, thread_id, .. } =>
            Some((owner_host_id.clone(), endpoint_id.clone(), thread_id.clone())),
        _ => None,
    }
}

pub(crate) fn restore_command(process: &ProcessRestoreSpec) -> Option<String> {
    match process {
        ProcessRestoreSpec::CodexAppServer { thread_id, socket_path, .. } => Some(codex_remote_cli_command(socket_path, thread_id)),
        ProcessRestoreSpec::CodexForwardedCli { .. } => None,

        ProcessRestoreSpec::Shell => None,
        ProcessRestoreSpec::Command { command, .. } => Some(command.clone()),
        ProcessRestoreSpec::Claude { session_id } => Some(format!("exec claude --resume {}", shell_quote(session_id))),
        ProcessRestoreSpec::Codex { session_id, yolo } => {
            let mut command = format!("codex resume {}", shell_quote(session_id));
            if *yolo {
                command.push_str(" --yolo");
            }
            Some(command)
        }
    }
}

/// The CLI only renders/controls the original app-server Thread. All user
/// arguments are quoted; no shell command is constructed from the Host label.
pub(crate) fn codex_remote_cli_command(socket_path: &str, thread_id: &str) -> String {
    codex_cli_command(socket_path, thread_id, None)
}

pub(super) fn codex_cli_command(socket_path: &str, thread_id: &str, cwd: Option<&str>) -> String {
    let directory = cwd.map(|path| format!("--cd {}", shell_quote(path)))
        .unwrap_or_else(|| format!("-c {}", shell_quote("tui.resume_cwd=\"session\"")));
    format!("{}; exec \"$station_codex\" --remote {} {directory} resume {}",super::target_tools::CODEX_RESOLVE, shell_quote(&format!("unix://{socket_path}")), shell_quote(thread_id))
}

fn claude_initial_command(session_id: &str) -> String {
    format!("exec claude --session-id {}", shell_quote(session_id))
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-._:/".contains(character))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Probe the already selected transport. This never starts or installs Codex.
pub(super) async fn ensure_codex_launcher(target: &polar_tmux_core::TmuxTarget, directory: Option<&str>) -> Result<()> {
    ensure_cli_launcher(target, directory, "codex").await
}

async fn ensure_cli_launcher(target: &polar_tmux_core::TmuxTarget, directory: Option<&str>, tool: &'static str) -> Result<()> {
    let probe=directory.map(|path|format!("cd {} && command -v {tool} >/dev/null",shell_quote(path))).unwrap_or_else(||format!("command -v {tool} >/dev/null"));
    let probe=if tool=="codex" {let check=format!("{}; command -v \"$station_codex\" >/dev/null",super::target_tools::CODEX_RESOLVE);directory.map(|path|format!("cd {} && {check}",shell_quote(path))).unwrap_or(check)}else{probe};
    let remote=format!("sh -lc {}",shell_quote(&probe));
    use polar_tmux_core::TmuxTarget;
    let mut command = match target {
        TmuxTarget::Local => {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-lc", &probe]);
            c
        }
        TmuxTarget::Ssh { destination } => {
            let mut c = tokio::process::Command::new("ssh");
            c.args([
                "-T",
                "-o",
                "BatchMode=yes",
                destination,
                &remote,
            ]);
            c
        }
        TmuxTarget::MultiplexedSsh {
            destination,
            control_socket,
        } => {
            let mut c = tokio::process::Command::new("ssh");
            c.args([
                "-T",
                "-S",
                control_socket,
                "-o",
                "BatchMode=yes",
                "-o",
                "ProxyCommand=/usr/bin/false",
                destination,
                &remote,
            ]);
            c
        }
        TmuxTarget::Wsl { distribution } => {
            let mut c = tokio::process::Command::new("wsl.exe");
            if let Some(d) = distribution {
                c.args(["-d", d]);
            }
            c.args(["--", "sh", "-lc", &probe]);
            c
        }
    };
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), command.status()).await;
    anyhow::ensure!(
        matches!(status, Ok(Ok(s)) if s.success()),
        format!("{} launcher or working directory is unavailable on the selected target", if tool == "codex" { "Codex" } else { "Claude" })
    );
    Ok(())
}




