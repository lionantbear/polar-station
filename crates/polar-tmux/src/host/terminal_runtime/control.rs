//! Policy owned and polled by the existing attachment select loop. No task,
//! timer, transport or input queue is created by this reconciler.
use super::*;
use polar_tmux_protocol::{
    TerminalControlIntentV1, TerminalControlPhaseV1 as Phase, TerminalControlStatusV1,
    TerminalGeometryV1,
};
use std::{future::Future, pin::Pin};
use tokio::sync::broadcast;

type Events = broadcast::Receiver<polar_tmux_core::TmuxControlEvent>;
pub(crate) enum Applied {
    Foreground {
        geometry: TerminalGeometryV1,
        events: Option<Events>,
    },
    Passive,
}
type Attempt = Pin<Box<dyn Future<Output = Result<Applied>> + Send>>;

#[derive(Default)]
pub(crate) struct TerminalControl {
    pub(crate) trace_consumer_id: String,
    pub(crate) intent: Option<TerminalControlIntentV1>,
    pub(crate) status: TerminalControlStatusV1,
    pub(crate) pending: Option<Attempt>,
    pub(crate) events: Option<Events>,
    pub(crate) epoch: Option<polar_tmux_core::TmuxControlEpoch>,
    pub(crate) deadline: Option<tokio::time::Instant>,
    eligible: bool,
    claim_needed: bool,
    eligibility_uncertain: bool,
    passive_epoch: Arc<AtomicU64>,
}

impl TerminalControl {
    pub(crate) fn accept(&mut self, intent: TerminalControlIntentV1) -> bool {
        if self.intent.as_ref().is_some_and(|old| {
            old.native_instance_id == intent.native_instance_id
                && (old.sequence > intent.sequence
                    || (old.sequence == intent.sequence
                        && old.viewport_revision >= intent.viewport_revision))
        }) {
            return false;
        }
        let selection = self.intent.as_ref().is_none_or(|old| {
            old.native_instance_id != intent.native_instance_id || old.sequence != intent.sequence
        });
        if selection {
            self.passive_epoch.fetch_add(1, Ordering::AcqRel);
        }
        if selection || !intent.focused {
            if self.pending.take().is_some() {
                tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_superseded", sequence=self.status.sequence);
            }
            self.events = None;
            self.claim_needed = intent.focused;
            if !intent.focused {
                self.events = None;
                self.deadline = None;
            }
        }
        self.status = TerminalControlStatusV1 {
            sequence: intent.sequence,
            binding_generation: intent.binding_generation,
            viewport_revision: intent.viewport_revision,
            phase: if !intent.focused {
                Phase::Superseded
            } else if !selection && self.pending.is_some() {
                Phase::Requested
            } else if !selection
                && self.eligible
                && self
                    .intent
                    .as_ref()
                    .is_some_and(|old| old.size == intent.size)
            {
                self.status.phase
            } else if !selection && self.eligible {
                Phase::Applied
            } else {
                Phase::Deferred
            },
            geometry: self.status.geometry.take(),
        };
        tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_desired",
            native_instance_id=intent.native_instance_id.as_str(), sequence=intent.sequence,
            viewport_revision=intent.viewport_revision, focused=intent.focused,
            content_generation=intent.binding_generation, columns=intent.size.columns, rows=intent.size.rows);
        self.intent = Some(intent);
        true
    }

    pub(crate) fn invalidate(&mut self) {
        self.passive_epoch.fetch_add(1, Ordering::AcqRel);
        self.eligibility_uncertain = false;
        if let Some(epoch) = self.epoch.take() {
            epoch.current.fetch_add(1, Ordering::AcqRel);
        }
        self.pending = None;
        self.deadline = None;
        self.events = None;
        self.eligible = false;
        self.claim_needed = self.intent.as_ref().is_some_and(|intent| intent.focused);
        self.status.phase = Phase::Deferred;
        self.status.geometry = None;
    }

    pub(crate) fn blur(&mut self) {
        self.passive_epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(epoch) = self.epoch.take() {
            epoch.current.fetch_add(1, Ordering::AcqRel);
        }
        self.pending = None;
        self.deadline = None;
        self.events = None;
        self.claim_needed = false;
        if let Some(intent) = &mut self.intent {
            intent.focused = false;
        }
        self.status.phase = Phase::Superseded;
    }

    pub(crate) fn reconcile(
        &mut self,
        backend: &mut BackendPty,
        size: TerminalSizeV1,
    ) -> Result<()> {
        if self.epoch.as_ref().is_some_and(|epoch| !epoch.is_current()) {
            self.blur();
        }
        let changed = backend.terminal_size != size;
        if !self.intent.as_ref().is_some_and(|intent| intent.focused) {
            if changed && (self.eligible || self.eligibility_uncertain) {
                if self.pending.is_none() {
                    let epoch = polar_tmux_core::TmuxControlEpoch {
                        current: self.passive_epoch.clone(),
                        expected: self.passive_epoch.load(Ordering::Acquire),
                    };
                    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                    self.deadline = Some(deadline);
                    self.pending = Some(release(backend, epoch, deadline));
                }
            } else {
                backend.synchronize_size(size)?;
            }
            return Ok(());
        }
        backend.synchronize_size(size)?;
        backend
            .control_sequence
            .store(self.status.sequence, Ordering::Release);
        if changed && self.eligible && self.pending.is_none() {
            self.status.phase = Phase::Applied;
            self.deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
        }
        if self.claim_needed && self.pending.is_none() {
            self.claim_needed = false;
            self.events = None;
            self.status.phase = Phase::Requested;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            self.deadline = Some(deadline);
            self.eligibility_uncertain = true;
            self.pending = Some(claim(
                backend,
                true,
                self.status.sequence,
                self.epoch.clone(),
                deadline,
            ));
        }
        Ok(())
    }

    pub(crate) fn completed(&mut self, result: Result<Applied>, desired: TerminalSizeV1) {
        self.pending = None;
        if self.epoch.as_ref().is_some_and(|epoch| !epoch.is_current()) {
            self.blur();
            return;
        }
        match result {
            Ok(Applied::Passive) => {
                self.eligible = false;
                self.eligibility_uncertain = false;
                self.deadline = None;
            }
            Ok(Applied::Foreground { geometry, events }) => {
                self.eligible = true;
                self.events = events;
                self.status.phase = Phase::Applied;
                tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_applied",
                    sequence=self.status.sequence, window_id=geometry.window_id.as_str(),
                    window_columns=geometry.window_columns, window_rows=geometry.window_rows);
                if geometry.window_policy != "latest" {
                    self.deadline = None;
                }
                let matches = geometry.matches(desired);
                self.status.geometry = Some(geometry);
                if matches {
                    self.observed();
                }
            }
            Err(_) => {
                self.status.phase = Phase::Failed;
                self.deadline = None;
                tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_failed", sequence=self.status.sequence);
            }
        }
    }

    pub(crate) fn geometry(
        &mut self,
        window: &str,
        columns: u16,
        rows: u16,
        desired: TerminalSizeV1,
    ) -> bool {
        if self.epoch.as_ref().is_some_and(|epoch| !epoch.is_current()) {
            self.blur();
            return true;
        }
        let before = self.status.clone();
        let Some(geometry) = &mut self.status.geometry else {
            return false;
        };
        if geometry.window_id != window {
            return false;
        }
        geometry.window_columns = columns;
        geometry.window_rows = rows;
        let status = if desired.rows <= geometry.status_lines {
            0
        } else {
            geometry.status_lines
        };
        if self.pending.is_none()
            && geometry.window_policy == "latest"
            && columns == desired.columns
            && rows == desired.rows.saturating_sub(status).max(1)
            && self.intent.as_ref().is_some_and(|intent| intent.focused)
        {
            if self.status.phase != Phase::Observed {
                self.observed();
            }
        } else if self.status.phase == Phase::Observed {
            self.status.phase = Phase::Applied;
            self.deadline = None;
            tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_geometry_changed", sequence=self.status.sequence, window_columns=columns, window_rows=rows);
        }
        self.status != before
    }

    pub(crate) fn timed_out(&mut self) {
        self.pending = None;
        self.deadline = None;
        self.status.phase = Phase::Failed;
        tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_observation_timeout", sequence=self.status.sequence);
    }
    pub(crate) fn retry_on_activity(&mut self) {
        if self.status.phase == Phase::Failed
            && self.intent.as_ref().is_some_and(|intent| intent.focused)
        {
            self.claim_needed = true;
        }
    }
    fn observed(&mut self) {
        self.deadline = None;
        self.status.phase = Phase::Observed;
        tracing::info!(target: "polar_station_operation", component="Host", consumer_id=self.trace_consumer_id.as_str(), operation_id=format!("{}-{}", self.trace_consumer_id, self.status.sequence), event="terminal_control_observed", sequence=self.status.sequence);
    }
}

fn claim(
    backend: &BackendPty,
    enable: bool,
    sequence: u64,
    epoch: Option<polar_tmux_core::TmuxControlEpoch>,
    deadline: tokio::time::Instant,
) -> Attempt {
    let mut identity = backend.client_identity.clone();
    let fallback = backend.tmux_client.clone();
    let provider = backend.size_client_provider.clone();
    let session = backend.tmux_session.clone();
    let binding = backend.tmux_binding.clone();
    let span = tracing::info_span!(target: "polar_station_operation", "terminal_control_claim", component="Host",
        consumer_id=backend.trace_consumer_id.as_str(), sequence, operation_id=format!("{}-{sequence}", backend.trace_consumer_id), outcome="cancelled");
    Box::pin(async move {
        let result = tokio::time::timeout_at(deadline, async move {
            let receipt = async {
                loop {
                    if let Some(receipt) = identity.borrow_and_update().clone() { break Ok::<_, anyhow::Error>(receipt); }
                    identity.changed().await.context("terminal identity closed")?;
                }
            }.instrument(tracing::info_span!(target: "polar_station_operation", "terminal_control_identity_wait", component="Host")).await?;
            let client = if let Some((target, ssh, generation)) = provider {
                let client = target.wait_for_control_client(generation.as_ref())
                    .instrument(tracing::info_span!(target: "polar_station_operation", "terminal_control_channel_wait", component="Host")).await?;
                anyhow::ensure!(generation == ssh.transport_generation(target.id()).await, "terminal transport changed");
                client
            } else { fallback };
            let events = client.terminal_control_events();
            let geometry = client.claim_terminal_size_guarded(&receipt, &session, enable, epoch, binding.as_ref()).await?;
            Ok::<_, anyhow::Error>(Applied::Foreground { geometry, events })
        }).await.context("terminal control deadline")?;
        tracing::Span::current().record("outcome", if result.is_ok() { "ok" } else { "failed" });
        result
    }.instrument(span))
}

fn release(
    backend: &BackendPty,
    epoch: polar_tmux_core::TmuxControlEpoch,
    deadline: tokio::time::Instant,
) -> Attempt {
    let mut identity = backend.client_identity.clone();
    let fallback = backend.tmux_client.clone();
    let provider = backend.size_client_provider.clone();
    let session = backend.tmux_session.clone();
    let binding = backend.tmux_binding.clone();
    let span = tracing::info_span!(target:"polar_station_operation", "terminal_viewport_release", component="Host", consumer_id=backend.trace_consumer_id.as_str(), outcome="cancelled");
    Box::pin(
        async move {
            let result = tokio::time::timeout_at(deadline, async {
                let receipt = loop {
                    if let Some(receipt) = identity.borrow_and_update().clone() {
                        break receipt;
                    }
                    identity
                        .changed()
                        .await
                        .context("terminal identity closed")?;
                };
                let client = if let Some((target, ssh, generation)) = provider {
                    let client = target.wait_for_control_client(generation.as_ref()).await?;
                    anyhow::ensure!(
                        generation == ssh.transport_generation(target.id()).await,
                        "terminal transport changed"
                    );
                    client
                } else {
                    fallback
                };
                client
                    .release_terminal_size_guarded(&receipt, &session, epoch, binding.as_ref())
                    .await?;
                Ok(Applied::Passive)
            })
            .await
            .context("passive viewport deadline")?;
            tracing::Span::current()
                .record("outcome", if result.is_ok() { "ok" } else { "failed" });
            result
        }
        .instrument(span),
    )
}


