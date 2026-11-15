//! Host-owned tmux Session registry and ordered frontend event feed.
//!
//! The registry owns runtime facts only. It cannot name or persist native
//! Workspace, Tab, Pane, layout, focus, selection, or Ghostty Surface state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use super::application_state::ApplicationStatePublisher;
use super::session_monitor::SessionMonitorReducer;
use super::session_summary_provider::{normalize_summary_sections, SessionSummaryProvider};
use super::terminal_text::sanitize_terminal_output;
use polar_tmux_core::TmuxClient;
use polar_tmux_protocol::{
    ApplicationStateApiV1, ExecutionTarget, ProtocolValidationError, SessionAttentionProjectionV1,
    SessionAttentionSourceV1, SessionAttentionStateV1, SessionFeedChangeV1, SessionFeedEventV1,
    SessionFeedSnapshotV1, SessionKeyV1, SessionLifecycleV1, SessionProjectionV1,
    SshTransportGenerationV1,
};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;

const SESSION_EVENT_CAPACITY: usize = 256;
const DEFAULT_SUMMARY_QUIET_PERIOD: Duration = Duration::from_secs(5);
const MAX_ANALYSIS_SNAPSHOT_BYTES: usize = 64 * 1024;
type MonitorJobKey = (SessionKeyV1, String);
type MonitorJobs = Arc<Mutex<BTreeMap<MonitorJobKey, JoinHandle<()>>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredSession {
    tmux_session_id: String,
    display_name: String,
    lifecycle: SessionLifecycleV1,
}

impl DiscoveredSession {
    pub(crate) fn with_lifecycle(mut self, lifecycle: SessionLifecycleV1) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    pub(crate) fn new(
        tmux_session_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Result<Self, ProtocolValidationError> {
        let candidate = Self {
            tmux_session_id: tmux_session_id.into(),
            display_name: display_name.into(),
            lifecycle: SessionLifecycleV1::Ready,
        };
        SessionProjectionV1 {
            key: SessionKeyV1 {
                execution_target: ExecutionTarget::local(),
                tmux_session_id: candidate.tmux_session_id.clone(),
            },
            display_name: candidate.display_name.clone(),
            lifecycle: SessionLifecycleV1::Ready,
            transport_generation: None,
            revision: 1,
            attention: None,
        }
        .validate()?;
        Ok(candidate)
    }
}

#[derive(Debug)]
struct SessionRegistryState {
    revision: u64,
    sessions: BTreeMap<SessionKeyV1, SessionProjectionV1>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionRegistry {
    epoch: Arc<str>,
    state: Arc<Mutex<SessionRegistryState>>,
    monitor_reducer: Arc<Mutex<SessionMonitorReducer>>,
    summary_provider: Arc<RwLock<Option<Arc<dyn SessionSummaryProvider>>>>,
    summary_quiet_period: Arc<RwLock<Duration>>,
    monitor_jobs: MonitorJobs,
    // Derived from active bindings, never a second control/notification owner.
    protocol_sessions: Arc<RwLock<BTreeSet<SessionKeyV1>>>,
    events: broadcast::Sender<SessionFeedEventV1>,
    application_events: Option<ApplicationStatePublisher>,
}

impl SessionRegistry {
    

    pub(crate) fn with_epoch_summary_provider_and_events(
        epoch: impl Into<String>,
        summary_provider: Option<Arc<dyn SessionSummaryProvider>>,
        application_events: Option<ApplicationStatePublisher>,
    ) -> Result<Self, ProtocolValidationError> {
        let epoch = epoch.into();
        SessionFeedSnapshotV1 {
            epoch: epoch.clone(),
            revision: 1,
            sessions: Vec::new(),
        }
        .validate()?;
        let (events, _) = broadcast::channel(SESSION_EVENT_CAPACITY);
        Ok(Self {
            epoch: Arc::from(epoch),
            state: Arc::new(Mutex::new(SessionRegistryState {
                revision: 1,
                sessions: BTreeMap::new(),
            })),
            monitor_reducer: Arc::new(Mutex::new(SessionMonitorReducer::default())),
            summary_provider: Arc::new(RwLock::new(summary_provider)),
            summary_quiet_period: Arc::new(RwLock::new(DEFAULT_SUMMARY_QUIET_PERIOD)),
            monitor_jobs: Arc::new(Mutex::new(BTreeMap::new())),
            protocol_sessions: Arc::new(RwLock::new(BTreeSet::new())),
            events,
            application_events,
        })
    }

    pub(crate) async fn snapshot(&self) -> SessionFeedSnapshotV1 {
        let state = self.state.lock().await;
        SessionFeedSnapshotV1 {
            epoch: self.epoch.to_string(),
            revision: state.revision,
            sessions: state.sessions.values().cloned().collect(),
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SessionFeedEventV1> {
        self.events.subscribe()
    }

    

    pub(crate) async fn replace_target_sessions_with_generation(
        &self,
        target: &ExecutionTarget,
        discovered: Vec<DiscoveredSession>,
        transport_generation: Option<SshTransportGenerationV1>,
    ) -> Result<bool, ProtocolValidationError> {
        target.validate()?;
        if let Some(generation) = &transport_generation {
            generation.validate()?;
        }
        let mut incoming = BTreeMap::new();
        for session in discovered {
            let key = SessionKeyV1 {
                execution_target: target.clone(),
                tmux_session_id: session.tmux_session_id,
            };
            key.validate()?;
            if incoming
                .insert(key, (session.display_name, session.lifecycle))
                .is_some()
            {
                return Err(ProtocolValidationError::InvalidIdentifier(
                    "discovered tmux Session",
                ));
            }
        }

        let mut state = self.state.lock().await;
        let existing_keys = state
            .sessions
            .keys()
            .filter(|key| &key.execution_target == target)
            .cloned()
            .collect::<BTreeSet<_>>();
        let incoming_keys = incoming.keys().cloned().collect::<BTreeSet<_>>();

        let mut changes = Vec::new();
        let mut absent = existing_keys
            .difference(&incoming_keys)
            .cloned()
            .collect::<Vec<_>>();
        absent.sort_by_key(|key| {
            let session = &state.sessions[key];
            if session.lifecycle == SessionLifecycleV1::Ended {
                session.revision
            } else {
                state.revision.saturating_add(1)
            }
        });
        let evict = absent.len().saturating_sub(SESSION_EVENT_CAPACITY);
        for (index, key) in absent.iter().enumerate() {
            if index < evict {
                state.sessions.remove(key);
                changes.push(SessionFeedChangeV1::Remove { key: key.clone() });
            } else if let Some(session) = state.sessions.get(key) {
                if session.lifecycle != SessionLifecycleV1::Ended {
                    let mut ended = session.clone();
                    ended.lifecycle = SessionLifecycleV1::Ended;
                    ended.attention = None;
                    changes.push(SessionFeedChangeV1::Upsert { session: ended });
                }
            }
        }
        for (key, (display_name, lifecycle)) in incoming {
            let attention = state
                .sessions
                .get(&key)
                .and_then(|existing| existing.attention.clone());
            let changed = state.sessions.get(&key).is_none_or(|existing| {
                existing.display_name != display_name
                    || existing.lifecycle != lifecycle
                    || existing.transport_generation != transport_generation
            });
            if changed {
                changes.push(SessionFeedChangeV1::Upsert {
                    session: SessionProjectionV1 {
                        key,
                        display_name,
                        lifecycle,
                        transport_generation: transport_generation.clone(),
                        revision: 0,
                        attention,
                    },
                });
            }
        }
        let removed = existing_keys
            .difference(&incoming_keys)
            .cloned()
            .collect::<Vec<_>>();
        let changed = self.commit_locked(&mut state, changes)?;
        drop(state);
        for key in removed {
            self.abort_monitor_jobs_for_session(&key).await;
            self.monitor_reducer.lock().await.remove(&key);
        }
        Ok(changed)
    }

    

    

    pub(crate) async fn mark_target_lifecycle(
        &self,
        target: &ExecutionTarget,
        lifecycle: SessionLifecycleV1,
        transport_generation: Option<SshTransportGenerationV1>,
    ) -> Result<bool, ProtocolValidationError> {
        target.validate()?;
        if let Some(generation) = &transport_generation {
            generation.validate()?;
        }
        let mut state = self.state.lock().await;
        let candidates = state
            .sessions
            .values()
            .filter(|session| {
                &session.key.execution_target == target
                    && session.lifecycle != SessionLifecycleV1::Ended
            })
            .filter(|session| {
                session.lifecycle != lifecycle
                    || session.transport_generation != transport_generation
            })
            .cloned()
            .map(|mut session| {
                session.lifecycle = lifecycle;
                session.transport_generation = transport_generation.clone();
                SessionFeedChangeV1::Upsert { session }
            })
            .collect();
        self.commit_locked(&mut state, candidates)
    }

    async fn publish_monitor_attention(
        &self,
        key: &SessionKeyV1,
        mut attention: SessionAttentionProjectionV1,
    ) -> Result<bool, ProtocolValidationError> {
        key.validate()?;
        let mut state = self.state.lock().await;
        let Some(existing) = state.sessions.get(key) else {
            return Ok(false);
        };
        let current_revision = existing
            .attention
            .as_ref()
            .map_or(0, |current| current.summary_revision);
        attention.summary_revision = current_revision.max(1);
        if existing.attention.as_ref() == Some(&attention) {
            return Ok(false);
        }
        attention.summary_revision = current_revision.saturating_add(1).max(1);
        attention.validate()?;
        let mut session = existing.clone();
        session.attention = Some(attention);
        self.commit_locked(&mut state, vec![SessionFeedChangeV1::Upsert { session }])
    }

    pub(crate) async fn observe_monitor_activity(
        &self,
        key: &SessionKeyV1,
        window_id: &str,
        client: TmuxClient,
        now_ms: u64,
    ) -> Result<bool, ProtocolValidationError> {
        let owned = self.protocol_sessions.read().await;
        if owned.contains(key) {
            return Ok(false);
        }
        key.validate()?;
        let update = self
            .monitor_reducer
            .lock()
            .await
            .observe_activity(key, now_ms);
        let changed = self
            .publish_monitor_attention(key, update.projection)
            .await?;
        self.schedule_monitor_analysis(
            key.clone(),
            window_id.to_owned(),
            client,
            update.activity_revision,
        )
        .await;
        Ok(changed)
    }

    pub(crate) async fn observe_monitor_bell(
        &self,
        key: &SessionKeyV1,
        now_ms: u64,
    ) -> Result<bool, ProtocolValidationError> {
        let owned = self.protocol_sessions.read().await;
        if owned.contains(key) {
            return Ok(false);
        }
        let candidate = self.monitor_reducer.lock().await.observe_bell(key, now_ms);
        match candidate {
            Some(attention) => self.publish_monitor_attention(key, attention).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn observe_monitor_direct(
        &self,
        key: &SessionKeyV1,
        state: SessionAttentionStateV1,
        source: SessionAttentionSourceV1,
        sections: Vec<String>,
        source_event_id: Option<String>,
        now_ms: u64,
    ) -> Result<bool, ProtocolValidationError> {
        let owned = self.protocol_sessions.read().await;
        if owned.contains(key) {
            return Ok(false);
        }
        let sections = normalize_summary_sections(sections);
        let candidate = self.monitor_reducer.lock().await.observe_direct(
            key,
            state,
            source,
            sections,
            source_event_id,
            now_ms,
        );
        if candidate.is_some() {
            self.abort_monitor_jobs_for_session(key).await;
        }
        match candidate {
            Some(attention) => self.publish_monitor_attention(key, attention).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn mark_monitor_disconnected(
        &self,
        key: &SessionKeyV1,
        detail: &str,
    ) -> Result<bool, ProtocolValidationError> {
        let owned = self.protocol_sessions.read().await;
        if owned.contains(key) {
            return Ok(false);
        }
        let candidate = self
            .monitor_reducer
            .lock()
            .await
            .mark_disconnected(key, detail);
        match candidate {
            Some(attention) => self.publish_monitor_attention(key, attention).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn mark_monitor_connected(
        &self,
        key: &SessionKeyV1,
        now_ms: u64,
    ) -> Result<bool, ProtocolValidationError> {
        let owned = self.protocol_sessions.read().await;
        if owned.contains(key) {
            return Ok(false);
        }
        let candidate = self
            .monitor_reducer
            .lock()
            .await
            .mark_connected(key, now_ms);
        match candidate {
            Some(attention) => self.publish_monitor_attention(key, attention).await,
            None => Ok(false),
        }
    }

    async fn schedule_monitor_analysis(
        &self,
        key: SessionKeyV1,
        window_id: String,
        client: TmuxClient,
        activity_revision: u64,
    ) {
        let quiet_period = *self.summary_quiet_period.read().await;
        let registry = self.clone();
        let job_key = (key.clone(), window_id.clone());
        let task = tokio::spawn(async move {
            tokio::time::sleep(quiet_period).await;
            let owned = registry.protocol_sessions.read().await;
            if owned.contains(&key) {
                return;
            }
            if registry
                .monitor_reducer
                .lock()
                .await
                .activity_revision(&key)
                != Some(activity_revision)
            {
                return;
            }
            let Some(provider) = registry.summary_provider.read().await.clone() else {
                let candidate = registry.monitor_reducer.lock().await.complete_inference(
                    &key,
                    activity_revision,
                    SessionAttentionStateV1::Unknown,
                    Vec::new(),
                    unix_milliseconds(),
                );
                if let Some(attention) = candidate {
                    let _ = registry.publish_monitor_attention(&key, attention).await;
                }
                return;
            };
            // A takeover cancels this registered job, including in-flight capture.
            drop(owned);
            let input = match capture_window_for_analysis(&client, &window_id).await {
                Ok(input) if !input.is_empty() => input,
                Ok(_) | Err(_) => {
                    let owned = registry.protocol_sessions.read().await;
                    if owned.contains(&key) {
                        return;
                    }
                    let candidate = registry.monitor_reducer.lock().await.complete_inference(
                        &key,
                        activity_revision,
                        SessionAttentionStateV1::Unknown,
                        Vec::new(),
                        unix_milliseconds(),
                    );
                    if let Some(attention) = candidate {
                        let _ = registry.publish_monitor_attention(&key, attention).await;
                    }
                    return;
                }
            };
            let owned = registry.protocol_sessions.read().await;
            if owned.contains(&key) {
                return;
            }
            let analyzing = registry
                .monitor_reducer
                .lock()
                .await
                .mark_analyzing(&key, activity_revision);
            let Some(analyzing) = analyzing else {
                return;
            };
            let _ = registry.publish_monitor_attention(&key, analyzing).await;
            drop(owned);
            let result = provider.analyze(&input).await;
            let owned = registry.protocol_sessions.read().await;
            if owned.contains(&key) {
                return;
            }
            let (state, sections) = match result {
                Ok(analysis) => (analysis.state, analysis.sections),
                Err(_) => (SessionAttentionStateV1::Unknown, Vec::new()),
            };
            let candidate = registry.monitor_reducer.lock().await.complete_inference(
                &key,
                activity_revision,
                state,
                normalize_summary_sections(sections),
                unix_milliseconds(),
            );
            if let Some(attention) = candidate {
                let _ = registry.publish_monitor_attention(&key, attention).await;
            }
        });
        if let Some(previous) = self.monitor_jobs.lock().await.insert(job_key, task) {
            previous.abort();
        }
    }

    async fn abort_monitor_jobs_for_session(&self, key: &SessionKeyV1) {
        let mut jobs = self.monitor_jobs.lock().await;
        let job_keys = jobs
            .keys()
            .filter(|(candidate, _)| candidate == key)
            .cloned()
            .collect::<Vec<_>>();
        for job_key in job_keys {
            if let Some(task) = jobs.remove(&job_key) {
                task.abort();
            }
        }
    }

    pub(crate) async fn protocol_owns_attention(&self, key: &SessionKeyV1) -> bool {
        self.protocol_sessions.read().await.contains(key)
    }

    #[cfg(feature = "codex-control")]
    pub(crate) async fn update_protocol_attention(
        &self, projections: Vec<(SessionKeyV1, SessionAttentionProjectionV1)>,
        scope: Option<&BTreeSet<SessionKeyV1>>,
    ) -> Result<(), ProtocolValidationError> {
        let mut owned = self.protocol_sessions.write().await;
        let mut next: BTreeSet<_> = projections.iter().map(|(key, _)| key.clone()).collect();
        if let Some(scope) = scope { next.extend(owned.iter().filter(|key| !scope.contains(key)).cloned()); }
        for key in owned.symmetric_difference(&next) {
            self.abort_monitor_jobs_for_session(key).await;
            self.monitor_reducer.lock().await.remove(key);
        }
        for key in owned.difference(&next) {
            let mut state = self.state.lock().await;
            if let Some(session) = state.sessions.get(key) {
                let mut session = session.clone();
                session.attention = None;
                self.commit_locked(&mut state, vec![SessionFeedChangeV1::Upsert { session }])?;
            }
        }
        *owned = next;
        for (key, mut attention) in projections {
            // Preserve an unchanged edge's timestamp and acknowledgements.
            if let Some(previous) = self
                .state
                .lock()
                .await
                .sessions
                .get(&key)
                .and_then(|session| session.attention.as_ref())
                .filter(|previous| {
                    previous.source_event_id == attention.source_event_id
                        && previous.state == attention.state
                })
            {
                attention.attention_since_ms = previous.attention_since_ms;
                attention.unread = previous.unread;
                attention.notification_pending = previous.notification_pending;
            }
            self.publish_monitor_attention(&key, attention).await?;
        }
        Ok(())
    }

    pub(crate) async fn set_summary_provider(
        &self,
        provider: Option<Arc<dyn SessionSummaryProvider>>,
        quiet_period: Duration,
    ) {
        for (_, task) in std::mem::take(&mut *self.monitor_jobs.lock().await) {
            task.abort();
        }
        *self.summary_provider.write().await = provider;
        *self.summary_quiet_period.write().await = quiet_period;
    }

    pub(crate) async fn generate_reply_draft(
        &self,
        summary: &str,
        reply: &str,
    ) -> Result<String, String> {
        let provider = self.summary_provider.read().await.clone().ok_or_else(|| {
            "Enable the Codex app-server provider in Settings before generating a draft".to_owned()
        })?;
        provider.draft(summary, reply).await
    }

    pub(crate) async fn attention_snapshot(
        &self,
    ) -> Vec<(SessionKeyV1, SessionAttentionProjectionV1)> {
        self.state
            .lock()
            .await
            .sessions
            .values()
            .filter_map(|session| {
                session
                    .attention
                    .clone()
                    .map(|attention| (session.key.clone(), attention))
            })
            .collect()
    }

    /// Returns `None` for a missing/stale projection and `Some(changed)` for
    /// an accepted acknowledgement.
    pub(crate) async fn acknowledge_attention(
        &self,
        key: &SessionKeyV1,
        summary_revision: u64,
    ) -> Result<Option<bool>, ProtocolValidationError> {
        key.validate()?;
        let mut state = self.state.lock().await;
        let Some(existing) = state.sessions.get(key) else {
            return Ok(None);
        };
        let Some(mut attention) = existing.attention.clone() else {
            return Ok(None);
        };
        if attention.summary_revision != summary_revision {
            return Ok(None);
        }
        let previous = attention.clone();
        attention.unread = false;
        if !attention.state.is_red() {
            attention.notification_pending = false;
        }
        if attention == previous {
            return Ok(Some(false));
        }
        let mut session = existing.clone();
        session.attention = Some(attention);
        self.commit_locked(&mut state, vec![SessionFeedChangeV1::Upsert { session }])?;
        Ok(Some(true))
    }

    /// Returns `None` for a missing/stale projection and `Some(changed)` for
    /// an accepted notification receipt.
    pub(crate) async fn mark_attention_notification_handled(
        &self,
        key: &SessionKeyV1,
        summary_revision: u64,
    ) -> Result<Option<bool>, ProtocolValidationError> {
        key.validate()?;
        let mut state = self.state.lock().await;
        let Some(existing) = state.sessions.get(key) else {
            return Ok(None);
        };
        let Some(mut attention) = existing.attention.clone() else {
            return Ok(None);
        };
        if attention.summary_revision != summary_revision {
            return Ok(None);
        }
        if !attention.notification_pending {
            return Ok(Some(false));
        }
        attention.notification_pending = false;
        let mut session = existing.clone();
        session.attention = Some(attention);
        self.commit_locked(&mut state, vec![SessionFeedChangeV1::Upsert { session }])?;
        Ok(Some(true))
    }

    fn commit_locked(
        &self,
        state: &mut SessionRegistryState,
        mut changes: Vec<SessionFeedChangeV1>,
    ) -> Result<bool, ProtocolValidationError> {
        if changes.is_empty() {
            return Ok(false);
        }
        state.revision = state.revision.saturating_add(1).max(1);
        let revision = state.revision;
        changes.sort_by(|left, right| change_key(left).cmp(change_key(right)));
        for change in &mut changes {
            if let SessionFeedChangeV1::Upsert { session } = change {
                session.revision = revision;
                session.validate()?;
                state.sessions.insert(session.key.clone(), session.clone());
            }
        }
        let event = SessionFeedEventV1 {
            epoch: self.epoch.to_string(),
            revision,
            changes,
        };
        event.validate()?;
        let _ = self.events.send(event);
        if let Some(events) = &self.application_events {
            events.publish(ApplicationStateApiV1::ControlCenterV1);
        }
        Ok(true)
    }
}

async fn capture_window_for_analysis(
    client: &TmuxClient,
    window_id: &str,
) -> Result<Vec<String>, String> {
    let mut panes = client
        .list_topology_panes(window_id)
        .await
        .map_err(|_| "tmux pane inventory was unavailable".to_owned())?;
    panes.sort_by_key(|pane| (!pane.active, pane.index));
    let mut remaining = MAX_ANALYSIS_SNAPSHOT_BYTES;
    let mut sections = Vec::new();
    for pane in panes.into_iter().filter(|pane| !pane.dead) {
        if remaining == 0 {
            break;
        }
        let capture = client
            .capture_pane_for_attention(&pane.id, 100)
            .await
            .map_err(|_| "tmux pane capture was unavailable".to_owned())?;
        let sanitized = sanitize_terminal_output(capture.as_bytes());
        let sanitized = sanitized.trim();
        if sanitized.is_empty() {
            continue;
        }
        let bounded = utf8_prefix(sanitized, remaining);
        remaining = remaining.saturating_sub(bounded.len());
        sections.push(format!("tmux pane {}\n{}", pane.id, bounded));
    }
    Ok(sections)
}

fn utf8_prefix(value: &str, maximum_bytes: usize) -> &str {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut end = maximum_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn unix_milliseconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}



fn change_key(change: &SessionFeedChangeV1) -> &SessionKeyV1 {
    match change {
        SessionFeedChangeV1::Upsert { session } => &session.key,
        SessionFeedChangeV1::Remove { key } => key,
    }
}
