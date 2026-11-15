//! Source-aware Agent attention reduction over typed tmux and adapter signals.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use polar_tmux_core::{TmuxClient, TmuxControlConnection, TmuxControlEvent, TmuxSessionBinding};
use polar_tmux_protocol::{
    SessionAttentionConfidenceV1, SessionAttentionProjectionV1, SessionAttentionSourceV1,
    SessionAttentionStateV1, SessionKeyV1, SessionMonitorHealthV1, SessionSummaryFreshnessV1,
    SshTransportGenerationV1,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::codex_monitor::{decode_codex_title, parse_codex_event_queue, CodexTitleState};
use super::session_lifecycle::SessionLifecycle;
use super::session_registry::SessionRegistry;

#[derive(Clone)]
pub(crate) struct SessionMonitorSupervisor {
    lifecycle: Option<Arc<SessionLifecycle>>,
    tasks: Arc<Mutex<BTreeMap<SessionKeyV1, RunningSessionMonitor>>>,
    connections:
        Arc<Mutex<BTreeMap<polar_tmux_protocol::ExecutionTarget, TargetMonitorConnection>>>,
    enabled: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    codex_title_detection: Arc<AtomicBool>,
}

struct RunningSessionMonitor {
    transport_generation: Option<SshTransportGenerationV1>,
    client: TmuxClient,
    connection: TmuxControlConnection,
    registry: SessionRegistry,
    task: Option<JoinHandle<()>>,
}

struct TargetMonitorConnection {
    generation: Option<SshTransportGenerationV1>,
    connection: TmuxControlConnection,
    observer: JoinHandle<()>,
}

impl Default for SessionMonitorSupervisor {
    fn default() -> Self {
        Self {
            lifecycle: None,
            tasks: Default::default(),
            connections: Default::default(),
            enabled: Arc::new(AtomicBool::new(true)),
            stopping: Arc::new(AtomicBool::new(false)),
            codex_title_detection: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl SessionMonitorSupervisor {
    pub(crate) fn with_lifecycle(mut self, lifecycle: Arc<SessionLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub(crate) async fn reconcile_target(
        &self,
        target: &polar_tmux_protocol::ExecutionTarget,
        client: TmuxClient,
        session_ids: impl IntoIterator<Item = String>,
        transport_generation: Option<SshTransportGenerationV1>,
        registry: SessionRegistry,
        registered: Option<crate::target_registry::RegisteredTarget>,
    ) -> anyhow::Result<()> {
        let incoming = session_ids
            .into_iter()
            .map(|tmux_session_id| SessionKeyV1 {
                execution_target: target.clone(),
                tmux_session_id,
            })
            .collect::<BTreeSet<_>>();
        if incoming.is_empty() {
            self.stop_target(target).await;
            return Ok(());
        }
        let (connection, newly_started) = {
            let mut connections = self.connections.lock().await;
            if connections
                .get(target)
                .is_some_and(|c| c.generation != transport_generation || c.connection.is_closed())
            {
                if let Some(old) = connections.remove(target) {
                    old.observer.abort();
                    old.connection.close();
                }
            }
            if let Some(existing) = connections.get(target) {
                (existing.connection.clone(), false)
            } else {
                let first = incoming.first().unwrap();
                let binding = if let Some(lifecycle) = &self.lifecycle {
                    match lifecycle
                        .attachment_binding(
                            target.as_str(),
                            &first.tmux_session_id,
                            transport_generation.as_ref(),
                        )
                        .await
                    {
                        Ok(Some(binding)) => Some(binding),
                        _ => {
                            if let Ok(id) =
                                polar_tmux_workspace::ExecutionTargetId::new(target.to_string())
                            {
                                lifecycle.request_refresh_if_idle(id);
                            }
                            anyhow::bail!("Session binding unavailable for target monitor");
                        }
                    }
                } else {
                    None
                };
                let runtime_id = binding
                    .as_ref()
                    .map_or(first.tmux_session_id.as_str(), |b| b.session_id.as_str());
                let connection = start_control_connection(&client, runtime_id, binding.as_ref())
                    .map_err(anyhow::Error::msg)?;
                let mut events = connection.subscribe();
                let lifecycle = self.lifecycle.clone();
                let target_id = target.clone();
                let observer = tokio::spawn(async move {
                    loop {
                        let event = events.recv().await;
                        let ended = matches!(
                            event,
                            Ok(TmuxControlEvent::Exit)
                                | Err(tokio::sync::broadcast::error::RecvError::Closed)
                        );
                        let changed = ended || matches!(event,
                            Ok(TmuxControlEvent::Lifecycle(polar_tmux_core::TmuxControlLifecycleEvent::SessionsChanged
                                | polar_tmux_core::TmuxControlLifecycleEvent::SessionRenamed { .. }
                                | polar_tmux_core::TmuxControlLifecycleEvent::WindowAdded { .. }
                                | polar_tmux_core::TmuxControlLifecycleEvent::WindowClosed { .. }))
                            | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)));
                        if changed {
                            if let Some(lifecycle) = &lifecycle {
                                if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(
                                    target_id.to_string(),
                                ) {
                                    lifecycle.request_refresh(target);
                                }
                            }
                        }
                        if ended {
                            break;
                        }
                    }
                });
                connections.insert(
                    target.clone(),
                    TargetMonitorConnection {
                        generation: transport_generation.clone(),
                        connection: connection.clone(),
                        observer,
                    },
                );
                (connection, true)
            }
        };
        if let Some(registered) = registered {
            registered.install_control_connection(transport_generation.clone(), connection.clone());
        }
        let client = client.with_control_connection(connection.clone());
        let mut added = false;
        let mut tasks = self.tasks.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            connection.close();
            return Ok(());
        }
        let stale = tasks
            .iter()
            .filter(|(key, running)| {
                &key.execution_target == target
                    && (!incoming.contains(*key)
                        || !running.connection.same_connection(&connection)
                        || running.transport_generation != transport_generation
                        || running.task.as_ref().is_some_and(JoinHandle::is_finished))
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(running) = tasks.remove(&key) {
                if let Some(task) = running.task {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        for key in incoming {
            if tasks.contains_key(&key) {
                continue;
            }
            added = true;
            let events = connection.subscribe();
            let monitor_client = client.clone();
            let monitor_registry = registry.clone();
            let monitor_key = key.clone();
            let task =
                (self.enabled.load(Ordering::Relaxed) || self.lifecycle.is_some()).then(|| {
                    let codex_titles = self.codex_title_detection.clone();
                    let lifecycle = self.lifecycle.clone();
                    let attention_enabled = self.enabled.clone();
                    let transport = transport_generation.clone();
                    tokio::spawn(async move {
                        run_session_monitor(
                            monitor_client,
                            monitor_key,
                            monitor_registry,
                            codex_titles,
                            lifecycle,
                            attention_enabled,
                            transport,
                            events,
                        )
                        .await;
                    })
                });
            tasks.insert(
                key,
                RunningSessionMonitor {
                    transport_generation: transport_generation.clone(),
                    client: client.clone(),
                    connection: connection.clone(),
                    registry: registry.clone(),
                    task,
                },
            );
        }
        drop(tasks);
        if newly_started {
            connection.wait_ready().await.map_err(anyhow::Error::msg)?;
        } else if added {
            let result = connection
                .command_text(
                    polar_tmux_core::target_subscription_commands().join(" ; "),
                    Duration::from_secs(10),
                )
                .await;
            if result.is_err() || result.is_ok_and(|reply| reply.failed) {
                connection.close();
                anyhow::bail!("target Control Mode subscriptions unavailable");
            }
        }
        Ok(())
    }

    pub(crate) async fn stop_target(&self, target: &polar_tmux_protocol::ExecutionTarget) {
        let old = self.connections.lock().await.remove(target);
        if let Some(old) = old {
            old.observer.abort();
            old.connection.shutdown().await;
        }
        let mut tasks = self.tasks.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        let keys = tasks
            .keys()
            .filter(|key| &key.execution_target == target)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(running) = tasks.remove(&key) {
                if let Some(task) = running.task {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let connections = std::mem::take(&mut *self.connections.lock().await);
        for (_, connection) in connections {
            connection.observer.abort();
            connection.connection.shutdown().await;
        }
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for (_, running) in tasks {
            if let Some(task) = running.task {
                task.abort();
                // Await cancellation so the owned tmux child is killed before Host exit.
                let _ = task.await;
            }
        }
    }

    pub(crate) async fn configure(&self, enabled: bool, codex_title_detection: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.codex_title_detection
            .store(codex_title_detection, Ordering::Relaxed);
        let mut tasks = self.tasks.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        let mut restarted = BTreeMap::new();
        for (key, running) in tasks.iter_mut() {
            if !enabled && self.lifecycle.is_none() {
                if let Some(task) = running.task.take() {
                    task.abort();
                    let _ = task.await;
                }
                let _ = running
                    .registry
                    .mark_monitor_disconnected(key, "tmux monitoring disabled in Settings")
                    .await;
            } else if running.task.as_ref().is_none_or(JoinHandle::is_finished) {
                let client = running.client.clone();
                let registry = running.registry.clone();
                let monitor_key = key.clone();
                let codex_titles = self.codex_title_detection.clone();
                let lifecycle = self.lifecycle.clone();
                let attention_enabled = self.enabled.clone();
                let transport = running.transport_generation.clone();
                restarted.insert(key.execution_target.clone(), running.connection.clone());
                let events = running.connection.subscribe();
                running.task = Some(tokio::spawn(async move {
                    run_session_monitor(
                        client,
                        monitor_key,
                        registry,
                        codex_titles,
                        lifecycle,
                        attention_enabled,
                        transport,
                        events,
                    )
                    .await;
                }));
            }
        }
        drop(tasks);
        for connection in restarted.into_values() {
            let result = connection
                .command_text(
                    polar_tmux_core::target_subscription_commands().join(" ; "),
                    Duration::from_secs(10),
                )
                .await;
            if result.is_err() || result.is_ok_and(|reply| reply.failed) {
                connection.close();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)] // Existing reduction inputs plus its target-owned event feed.
async fn run_session_monitor(
    client: TmuxClient,
    key: SessionKeyV1,
    registry: SessionRegistry,
    codex_title_detection: Arc<AtomicBool>,
    lifecycle: Option<Arc<SessionLifecycle>>,
    attention_enabled: Arc<AtomicBool>,
    transport: Option<SshTransportGenerationV1>,
    events: tokio::sync::broadcast::Receiver<TmuxControlEvent>,
) {
    let binding = match &lifecycle {
        Some(lifecycle) => match lifecycle
            .attachment_binding(
                key.execution_target.as_str(),
                &key.tmux_session_id,
                transport.as_ref(),
            )
            .await
        {
            Ok(Some(binding)) => Some(binding),
            _ => {
                report_existence_change(&Some(lifecycle.clone()), &key, &registry).await;
                return;
            }
        },
        None => None,
    };
    let runtime_id = binding
        .as_ref()
        .map_or(key.tmux_session_id.as_str(), |b| b.session_id.as_str());
    let _ = observe_control_mode(
        events,
        &client,
        &key,
        runtime_id,
        &registry,
        &codex_title_detection,
        lifecycle.as_deref(),
        &attention_enabled,
        binding.as_ref(),
        transport.as_ref(),
    )
    .await;
    let _ = registry
        .mark_monitor_disconnected(&key, "tmux monitor reconnecting")
        .await;
}

async fn report_existence_change(
    lifecycle: &Option<Arc<SessionLifecycle>>,
    key: &SessionKeyV1,
    registry: &SessionRegistry,
) {
    let _ = registry
        .mark_monitor_disconnected(key, "Session existence needs confirmation")
        .await;
    if let Some(lifecycle) = lifecycle {
        if let Ok(target) =
            polar_tmux_workspace::ExecutionTargetId::new(key.execution_target.to_string())
        {
            lifecycle.request_refresh(target);
        }
    }
}

fn start_control_connection(
    client: &TmuxClient,
    runtime_id: &str,
    binding: Option<&TmuxSessionBinding>,
) -> Result<TmuxControlConnection, String> {
    let plan = client
        .target_control_mode_plan(runtime_id, binding)
        .map_err(|error| error.to_string())?;
    let mut command = tokio::process::Command::new(&plan.program);
    command
        .args(&plan.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (name, value) in &plan.environment {
        command.env(name, value);
    }
    super::child_process::configure_parent_lease(&mut command)
        .map_err(|_| "tmux Control Mode parent lease could not be configured".to_owned())?;
    let child = command
        .spawn()
        .map_err(|_| "tmux Control Mode process could not start".to_owned())?;
    TmuxControlConnection::from_bootstrapped_child(child).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn observe_control_mode(
    mut events: tokio::sync::broadcast::Receiver<TmuxControlEvent>,
    client: &TmuxClient,
    key: &SessionKeyV1,
    runtime_id: &str,
    registry: &SessionRegistry,
    codex_title_detection: &AtomicBool,
    lifecycle: Option<&SessionLifecycle>,
    attention_enabled: &AtomicBool,
    binding: Option<&TmuxSessionBinding>,
    transport: Option<&SshTransportGenerationV1>,
) -> Result<(), String> {
    let mut observations = ControlObservations::default();
    let expected_subscriptions = polar_tmux_core::target_subscription_commands().len();
    let mut initialized_subscriptions = BTreeSet::new();
    let mut connected = false;
    loop {
        let event = events
            .recv()
            .await
            .map_err(|_| "tmux target subscription lost".to_owned())?;
        if matches!(event, TmuxControlEvent::Exit) {
            return Ok(());
        }
        if matches!(event, TmuxControlEvent::CommandFailed) {
            return Err("tmux Control Mode rejected a subscription command".to_owned());
        }
        if let TmuxControlEvent::SubscriptionChanged {
            name,
            session_id,
            window_id,
            pane_id,
            value,
            ..
        } = event
        {
            if session_id != runtime_id {
                continue;
            }
            initialized_subscriptions.insert(name.clone());
            if name == "attached-clients" {
                if let (Some(lifecycle), Some(binding)) = (lifecycle, binding) {
                    if let Ok(count) = value.parse::<u32>() {
                        lifecycle
                            .observe_attached_clients(
                                key.execution_target.as_str(),
                                &key.tmux_session_id,
                                binding,
                                transport,
                                count,
                            )
                            .await;
                    }
                }
            }
            if name == "pane-dead" && value == "1" {
                if let Some(lifecycle) = lifecycle {
                    let _ = lifecycle
                        .observe_completion(
                            client,
                            key.execution_target.as_str(),
                            &key.tmux_session_id,
                        )
                        .await;
                }
            }
            if !attention_enabled.load(Ordering::Relaxed)
                || registry.protocol_owns_attention(key).await
            {
                continue;
            }
            observations
                .observe(
                    &name,
                    &window_id,
                    &pane_id,
                    &value,
                    client,
                    key,
                    registry,
                    codex_title_detection.load(Ordering::Relaxed),
                )
                .await;
            if !connected && initialized_subscriptions.len() >= expected_subscriptions {
                let _ = registry.mark_monitor_connected(key, now_ms()).await;
                connected = true;
            }
        }
    }
}

#[derive(Default)]
struct ControlObservations {
    values: BTreeMap<(String, String, String), String>,
    pane_commands: BTreeMap<String, String>,
    pane_titles: BTreeMap<String, String>,
    codex_titles: BTreeMap<String, CodexTitleState>,
}

impl ControlObservations {
    #[allow(clippy::too_many_arguments)]
    async fn observe(
        &mut self,
        name: &str,
        window_id: &str,
        pane_id: &str,
        value: &str,
        client: &TmuxClient,
        key: &SessionKeyV1,
        registry: &SessionRegistry,
        codex_title_detection: bool,
    ) {
        let observation_key = (name.to_owned(), window_id.to_owned(), pane_id.to_owned());
        let previous = self.values.insert(observation_key, value.to_owned());
        if previous.as_deref() == Some(value) {
            return;
        }
        match name {
            "window-activity" if previous.is_some() => {
                let _ = registry
                    .observe_monitor_activity(key, window_id, client.clone(), now_ms())
                    .await;
            }
            "window-bell" if previous.as_deref() == Some("0") && value == "1" => {
                let _ = registry.observe_monitor_bell(key, now_ms()).await;
            }
            "pane-command" if codex_title_detection => {
                self.pane_commands
                    .insert(pane_id.to_owned(), value.to_owned());
                self.observe_codex_title(pane_id, key, registry).await;
            }
            "pane-title" if codex_title_detection => {
                self.pane_titles
                    .insert(pane_id.to_owned(), value.to_owned());
                self.observe_codex_title(pane_id, key, registry).await;
            }
            "agent-events" if !value.is_empty() => {
                if let Ok(events) = parse_codex_event_queue(value) {
                    for event in events {
                        let _ = registry
                            .observe_monitor_direct(
                                key,
                                SessionAttentionStateV1::ReviewReady,
                                SessionAttentionSourceV1::AgentHook,
                                vec!["Codex turn completed".to_owned()],
                                Some(event.event_id),
                                now_ms(),
                            )
                            .await;
                    }
                }
            }
            _ => {}
        }
    }

    async fn observe_codex_title(
        &mut self,
        pane_id: &str,
        key: &SessionKeyV1,
        registry: &SessionRegistry,
    ) {
        let (Some(command), Some(title)) = (
            self.pane_commands.get(pane_id),
            self.pane_titles.get(pane_id),
        ) else {
            return;
        };
        let Some(next) = decode_codex_title(command, title) else {
            return;
        };
        let previous = self.codex_titles.insert(pane_id.to_owned(), next);
        if previous == Some(next) {
            return;
        }
        let (state, summary) = match next {
            CodexTitleState::Working => (SessionAttentionStateV1::Working, "Codex is working"),
            CodexTitleState::ActionRequired => (
                SessionAttentionStateV1::NeedsAttention,
                "Codex requires attention",
            ),
            CodexTitleState::Ready if previous == Some(CodexTitleState::Working) => (
                SessionAttentionStateV1::ReviewReady,
                "Codex is ready for review",
            ),
            CodexTitleState::Ready => return,
        };
        let _ = registry
            .observe_monitor_direct(
                key,
                state,
                SessionAttentionSourceV1::PaneTitle,
                vec![summary.to_owned()],
                None,
                now_ms(),
            )
            .await;
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitorActivityUpdate {
    pub(crate) activity_revision: u64,
    pub(crate) projection: SessionAttentionProjectionV1,
}

#[derive(Debug)]
struct SessionMonitorEntry {
    activity_revision: u64,
    epoch_started_ms: u64,
    projection: SessionAttentionProjectionV1,
    seen_direct_events: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub(crate) struct SessionMonitorReducer {
    sessions: BTreeMap<SessionKeyV1, SessionMonitorEntry>,
}

impl SessionMonitorReducer {
    pub(crate) fn observe_activity(
        &mut self,
        key: &SessionKeyV1,
        now_ms: u64,
    ) -> MonitorActivityUpdate {
        let entry = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| new_entry(key, now_ms));
        let starts_new_epoch = !matches!(entry.projection.state, SessionAttentionStateV1::Working);
        if starts_new_epoch {
            entry.epoch_started_ms = now_ms;
            entry.seen_direct_events.clear();
        }
        entry.activity_revision = entry.activity_revision.saturating_add(1).max(1);
        if !starts_new_epoch && entry.projection.confidence == SessionAttentionConfidenceV1::Direct
        {
            return MonitorActivityUpdate {
                activity_revision: entry.activity_revision,
                projection: entry.projection.clone(),
            };
        }
        entry.projection = projection(
            key,
            SessionAttentionStateV1::Working,
            SessionAttentionSourceV1::TmuxActivity,
            SessionAttentionConfidenceV1::Inferred,
            SessionMonitorHealthV1::Connected,
            None,
            Vec::new(),
            None,
            entry.epoch_started_ms,
        );
        MonitorActivityUpdate {
            activity_revision: entry.activity_revision,
            projection: entry.projection.clone(),
        }
    }

    pub(crate) fn observe_bell(
        &mut self,
        key: &SessionKeyV1,
        now_ms: u64,
    ) -> Option<SessionAttentionProjectionV1> {
        let entry = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| new_entry(key, now_ms));
        if entry.projection.confidence == SessionAttentionConfidenceV1::Direct {
            return None;
        }
        let candidate = projection(
            key,
            SessionAttentionStateV1::NeedsAttention,
            SessionAttentionSourceV1::TmuxBell,
            SessionAttentionConfidenceV1::Inferred,
            SessionMonitorHealthV1::Connected,
            None,
            vec!["Terminal notification requested attention".to_owned()],
            None,
            entry.epoch_started_ms,
        );
        replace_if_changed(entry, candidate)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn observe_direct(
        &mut self,
        key: &SessionKeyV1,
        state: SessionAttentionStateV1,
        source: SessionAttentionSourceV1,
        summary_sections: Vec<String>,
        source_event_id: Option<String>,
        now_ms: u64,
    ) -> Option<SessionAttentionProjectionV1> {
        let entry = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| new_entry(key, now_ms));
        if source_event_id
            .as_ref()
            .is_some_and(|id| entry.seen_direct_events.contains(id))
        {
            return None;
        }
        if let Some(id) = &source_event_id {
            entry.seen_direct_events.insert(id.clone());
        }
        let candidate = projection(
            key,
            state,
            source,
            SessionAttentionConfidenceV1::Direct,
            SessionMonitorHealthV1::Connected,
            None,
            summary_sections,
            source_event_id,
            entry.epoch_started_ms,
        );
        replace_if_changed(entry, candidate)
    }

    pub(crate) fn complete_inference(
        &mut self,
        key: &SessionKeyV1,
        expected_activity_revision: u64,
        state: SessionAttentionStateV1,
        summary_sections: Vec<String>,
        now_ms: u64,
    ) -> Option<SessionAttentionProjectionV1> {
        let entry = self.sessions.get_mut(key)?;
        if entry.activity_revision != expected_activity_revision
            || entry.projection.confidence == SessionAttentionConfidenceV1::Direct
        {
            return None;
        }
        let (health, detail) = if state == SessionAttentionStateV1::Unknown {
            (
                SessionMonitorHealthV1::Degraded,
                Some("Agent state could not be classified".to_owned()),
            )
        } else {
            (SessionMonitorHealthV1::Connected, None)
        };
        let candidate = projection(
            key,
            state,
            SessionAttentionSourceV1::AiClassification,
            SessionAttentionConfidenceV1::Inferred,
            health,
            detail,
            summary_sections,
            None,
            entry.epoch_started_ms.min(now_ms),
        );
        replace_if_changed(entry, candidate)
    }

    pub(crate) fn mark_analyzing(
        &mut self,
        key: &SessionKeyV1,
        expected_activity_revision: u64,
    ) -> Option<SessionAttentionProjectionV1> {
        let entry = self.sessions.get_mut(key)?;
        if entry.activity_revision != expected_activity_revision
            || entry.projection.confidence == SessionAttentionConfidenceV1::Direct
        {
            return None;
        }
        let mut candidate = entry.projection.clone();
        candidate.freshness = SessionSummaryFreshnessV1::Analyzing;
        candidate.primary_source = SessionAttentionSourceV1::AiClassification;
        candidate.monitor_health = SessionMonitorHealthV1::Connected;
        candidate.monitor_detail = None;
        replace_if_changed(entry, candidate)
    }

    pub(crate) fn activity_revision(&self, key: &SessionKeyV1) -> Option<u64> {
        self.sessions.get(key).map(|entry| entry.activity_revision)
    }

    pub(crate) fn mark_disconnected(
        &mut self,
        key: &SessionKeyV1,
        detail: impl Into<String>,
    ) -> Option<SessionAttentionProjectionV1> {
        let entry = self.sessions.get_mut(key)?;
        let mut candidate = entry.projection.clone();
        candidate.monitor_health = SessionMonitorHealthV1::Disconnected;
        candidate.monitor_detail = Some(detail.into());
        if candidate.freshness == SessionSummaryFreshnessV1::Analyzing {
            candidate.freshness = SessionSummaryFreshnessV1::Stale;
        }
        replace_if_changed(entry, candidate)
    }

    pub(crate) fn mark_connected(
        &mut self,
        key: &SessionKeyV1,
        now_ms: u64,
    ) -> Option<SessionAttentionProjectionV1> {
        if !self.sessions.contains_key(key) {
            let entry = new_entry(key, now_ms);
            let projection = entry.projection.clone();
            self.sessions.insert(key.clone(), entry);
            return Some(projection);
        }
        let entry = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| new_entry(key, now_ms));
        let mut candidate = entry.projection.clone();
        candidate.monitor_health = SessionMonitorHealthV1::Connected;
        candidate.monitor_detail = None;
        replace_if_changed(entry, candidate)
    }

    pub(crate) fn remove(&mut self, key: &SessionKeyV1) {
        self.sessions.remove(key);
    }
}

fn new_entry(key: &SessionKeyV1, now_ms: u64) -> SessionMonitorEntry {
    SessionMonitorEntry {
        activity_revision: 0,
        epoch_started_ms: now_ms,
        projection: projection(
            key,
            SessionAttentionStateV1::Unknown,
            SessionAttentionSourceV1::Unspecified,
            SessionAttentionConfidenceV1::Inferred,
            SessionMonitorHealthV1::Connected,
            None,
            Vec::new(),
            None,
            now_ms,
        ),
        seen_direct_events: BTreeSet::new(),
    }
}

#[allow(clippy::too_many_arguments)] // One constructor keeps every provenance field explicit.
fn projection(
    key: &SessionKeyV1,
    state: SessionAttentionStateV1,
    source: SessionAttentionSourceV1,
    confidence: SessionAttentionConfidenceV1,
    monitor_health: SessionMonitorHealthV1,
    monitor_detail: Option<String>,
    summary_sections: Vec<String>,
    source_event_id: Option<String>,
    epoch_started_ms: u64,
) -> SessionAttentionProjectionV1 {
    let actionable = state.is_red() || state.is_orange();
    SessionAttentionProjectionV1 {
        execution_target_id: key.execution_target.to_string(),
        tmux_session_id: key.tmux_session_id.clone(),
        summary_revision: 0,
        summary_sections,
        state,
        freshness: if state == SessionAttentionStateV1::Unknown {
            SessionSummaryFreshnessV1::Unavailable
        } else {
            SessionSummaryFreshnessV1::Fresh
        },
        primary_source: source,
        confidence,
        monitor_health,
        monitor_detail,
        source_event_id,
        attention_since_ms: actionable.then_some(epoch_started_ms),
        unread: actionable,
        notification_pending: state.is_red(),
    }
}

fn replace_if_changed(
    entry: &mut SessionMonitorEntry,
    candidate: SessionAttentionProjectionV1,
) -> Option<SessionAttentionProjectionV1> {
    if entry.projection == candidate {
        return None;
    }
    entry.projection = candidate.clone();
    Some(candidate)
}


