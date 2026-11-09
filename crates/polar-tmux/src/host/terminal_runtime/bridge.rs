//! One owner for each Consumer attachment, retry budget and cancellable I/O.
use super::*;
use crate::host::{session_lifecycle::SessionLifecycle, session_registry::SessionRegistry};
use crate::{
    local_ipc,
    ssh_connection_manager::{SshConnectionManager, SshConnectionStatus},
    target_registry::TargetRegistry,
};
use polar_tmux_protocol::{SessionKeyV1, SessionLifecycleV1, TerminalBridgeFrameV1};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

pub(crate) struct TerminalServices {
    pub targets: TargetRegistry,
    pub ssh_connections: SshConnectionManager,
    pub session_lifecycle: Arc<SessionLifecycle>,
    pub sessions: SessionRegistry,
    pub terminal_consumers: TerminalConsumerRegistry,
}

type AttachAttempt = Pin<Box<dyn Future<Output = Result<TerminalAttachment>> + Send>>;
struct AttachmentRuntime {
    control: TerminalControl,
    input_activity: TerminalInputActivity,
    retry: TerminalRetry,
    pending_attach: Option<AttachAttempt>,
    backend: Option<BackendPty>,
    attaching_transport: Option<SshTransportGenerationV1>,
    recovered_until: Option<tokio::time::Instant>,
    redraw_deadline: Option<tokio::time::Instant>,
    last_probe_unix_ms: Option<u64>,
    presentation_tick: tokio::time::Instant,
}
impl Default for AttachmentRuntime {
    fn default() -> Self {
        Self {
            control: TerminalControl::default(),
            input_activity: TerminalInputActivity::default(),
            retry: TerminalRetry::default(),
            pending_attach: None,
            backend: None,
            attaching_transport: None,
            recovered_until: None,
            redraw_deadline: None,
            last_probe_unix_ms: None,
            presentation_tick: tokio::time::Instant::now() + Duration::from_secs(1),
        }
    }
}
impl AttachmentRuntime {
    fn start(&mut self, services: &TerminalServices, consumer: &TerminalConsumerState) {
        if consumer.binding().is_none() || self.pending_attach.is_some() {
            return;
        }
        self.retry.pause();
        self.redraw_deadline = None;
        self.last_probe_unix_ms = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
        self.pending_attach = Some(Box::pin(start_terminal_attach(services, consumer)));
    }
    fn session_unavailable(
        &mut self,
        consumer: &mut TerminalConsumerState,
        status: Option<SessionLifecycleV1>,
    ) -> Result<bool> {
        // Recovery projections are derived from the same transport transition.
        // Pending and installed backends validate the remote guard or fresh
        // observation plus transport generation. Do not restart that work.
        if (self.pending_attach.is_some() || self.backend.is_some())
            && matches!(
                status,
                Some(SessionLifecycleV1::Connecting | SessionLifecycleV1::Recovering)
            )
        {
            tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_attach_retained_during_session_recovery",
                reason=if self.backend.is_some() { "active_backend" } else { "pending_attach" });
            return Ok(false);
        }
        tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_session_invalidated",
        count=usize::from(self.pending_attach.is_some()), reason=match status {
            Some(SessionLifecycleV1::Connecting) => "connecting",
            Some(SessionLifecycleV1::Recovering) => "recovering",
            Some(SessionLifecycleV1::Unavailable) => "unavailable",
            Some(SessionLifecycleV1::Ending) => "ending",
            Some(SessionLifecycleV1::Ended) => "ended",
            Some(SessionLifecycleV1::Ready) => "ready",
            None => "removed",
        });
        if status == Some(SessionLifecycleV1::Ending) {
            self.cancel(); self.retry.pause(); consumer.transport_lost(None)?;
        } else if status == Some(SessionLifecycleV1::Ended) {
            self.ended(consumer)?;
        } else {
            self.failed(consumer)?;
        }
        Ok(true)
    }
    fn failed(&mut self, consumer: &mut TerminalConsumerState) -> Result<()> {
        self.cancel();
        self.retry.failed();
        consumer.transport_lost(None)?;
        Ok(())
    }
    fn reset(&mut self) {
        self.cancel();
        self.retry.reset();
        self.control = TerminalControl::default();
        self.recovered_until = None;
    }
    fn ended(&mut self, consumer: &mut TerminalConsumerState) -> Result<()> {
        self.cancel();
        self.retry.pause();
        consumer.mark_ended()?;
        Ok(())
    }
    fn first_redraw(&mut self, consumer: &mut TerminalConsumerState) -> Result<()> {
        consumer.mark_ready(self.attaching_transport.clone())?;
        self.redraw_deadline = None;
        self.recovered_until = Some(tokio::time::Instant::now() + Duration::from_secs(2));
        self.retry.reset();
        Ok(())
    }
    fn retire_backend(&mut self) {
        self.control.invalidate();
        if let Some(backend) = self.backend.take() {
            backend.stop();
        }
    }
    fn cancel(&mut self) {
        self.pending_attach = None;
        self.retire_backend();
        self.redraw_deadline = None;
        self.attaching_transport = None;
    }
}
impl Drop for AttachmentRuntime {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(crate) async fn serve_bridge(
    stream: tokio::net::UnixStream,
    state: TerminalServices,
    hello: TerminalBridgeHelloV1,
    control_v2: bool,
) -> Result<()> {
    let mut consumer =
        TerminalConsumerState::new(hello).context("invalid terminal Consumer registration")?;
    let consumer_id = consumer.status().consumer_id.clone();
    let consumer_generation = consumer.status().consumer_generation;
    let mut registration = state
        .terminal_consumers
        .register(
            &consumer_id,
            consumer_generation,
            consumer.binding().cloned(),
        )
        .await
        .context("terminal Consumer registration is stale")?;
    tracing::info!(target: "polar_station_operation", component="Host", event="bridge_registered",
        consumer_id=consumer_id.as_str(), generation=consumer_generation, content_generation=consumer.status().content_generation);
    let mut runtime = AttachmentRuntime::default();
    runtime.control.trace_consumer_id = consumer_id.clone();
    let mut selection = registration.selected.clone();
    let result: Result<()> = async {
        let (mut reader, mut writer) = stream.into_split();
        runtime.start(&state, &consumer);
        consumer.project_recovery(&runtime.retry, runtime.pending_attach.is_some(), runtime.last_probe_unix_ms, false);
        local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;

        if control_v2 {
            runtime.control.status.binding_generation = consumer.status().content_generation;
            local_ipc::write_terminal_bridge_frame(&mut writer, &TerminalBridgeFrameV1::ControlStatus(runtime.control.status.clone())).await?;
        }
        let mut session_events = state.sessions.subscribe();
        let mut observed_session_state = state.sessions.snapshot().await.sessions.into_iter()
            .find(|s| consumer.binding().is_some_and(|binding| s.key == SessionKeyV1::from(binding.clone())))
            .map(|s| (s.key, s.lifecycle, s.transport_generation));
        let mut transport_events = state.ssh_connections.subscribe_status_changes();
        // Keep partial headers/payloads across unrelated status and timer events.
        let mut focus_ordinal: u64 = 0;
        let mut reading = Box::pin(local_ipc::read_terminal_bridge_frame(&mut reader));
        loop {
        if consumer.project_recovery(&runtime.retry, runtime.pending_attach.is_some() || (runtime.backend.is_some() && consumer.status().lifecycle != TerminalConsumerLifecycleV1::Ready), runtime.last_probe_unix_ms, runtime.recovered_until.is_some_and(|until| tokio::time::Instant::now() < until)) {
            local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
        }
        tokio::select! {
            _ = async { if let Some(deadline) = runtime.control.deadline { tokio::time::sleep_until(deadline).await } else { std::future::pending::<()>().await } } => {
                runtime.control.timed_out();
                if control_v2 { local_ipc::write_terminal_bridge_frame(&mut writer, &TerminalBridgeFrameV1::ControlStatus(runtime.control.status.clone())).await?; }
            }
            result = async { match runtime.control.pending.as_mut() { Some(attempt) => attempt.await, None => std::future::pending().await } } => {
                let succeeded = result.is_ok();
                runtime.control.completed(result, consumer.terminal_size());
                if succeeded {
                    if let Some(backend) = runtime.backend.as_mut() { runtime.control.reconcile(backend, consumer.terminal_size())?; }
                }
                if control_v2 { local_ipc::write_terminal_bridge_frame(&mut writer, &TerminalBridgeFrameV1::ControlStatus(runtime.control.status.clone())).await?; }
            }
            event = async { match runtime.control.events.as_mut() { Some(events) => events.recv().await, None => std::future::pending().await } } => {
                match event {
                    Ok(polar_tmux_core::TmuxControlEvent::WindowGeometry { window_id, columns, rows }) => {
                        let changed = runtime.control.geometry(&window_id, columns, rows, consumer.terminal_size());
                        if changed && control_v2 { local_ipc::write_terminal_bridge_frame(&mut writer, &TerminalBridgeFrameV1::ControlStatus(runtime.control.status.clone())).await?; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => runtime.control.events = None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => { tracing::info!(target:"polar_station_operation", component="Host", event="terminal_control_observation_lagged", count); }
                    _ => {}
                }
            }
            changed = selection.changed(), if control_v2 => {
                if changed.is_err() { break Ok(()); }
                if !*selection.borrow_and_update() {
                    runtime.control.blur();
                    if let Some(backend) = runtime.backend.as_mut() { backend.focus(false).await?; }
                }
            }
            _ = tokio::time::sleep_until(runtime.presentation_tick), if runtime.retry.deadline.is_some() || runtime.recovered_until.is_some() || runtime.redraw_deadline.is_some() => {
                runtime.presentation_tick = tokio::time::Instant::now() + Duration::from_secs(1);
                if runtime.recovered_until.is_some_and(|until| tokio::time::Instant::now() >= until) { runtime.recovered_until = None; }
                if runtime.redraw_deadline.is_some_and(|until| tokio::time::Instant::now() >= until) {
                    runtime.failed(&mut consumer)?;
                }
            }
            _ = registration.revoked() => {
                break Ok(());
            }
            _ = async {
                match runtime.retry.deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            }, if runtime.backend.is_none() && runtime.pending_attach.is_none() => {
                runtime.retry.pause();
                if consumer.binding().is_some() && consumer.status().lifecycle != TerminalConsumerLifecycleV1::Ended {
                    consumer.transport_lost(None)?;
                    runtime.start(&state, &consumer);
                    local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                }
            }
            completed = async {
                match runtime.pending_attach.as_mut() {
                    Some(attempt) => attempt.await,
                    None => std::future::pending().await,
                }
            } => {
                runtime.pending_attach = None;
                match completed {
                    Ok(installed) => {
                        // A background attach retains its initial passive PTY
                        // grid; current foreground state is confirmed on Ready.
                        runtime.attaching_transport = installed.transport_generation;
                        runtime.redraw_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(15));
                        runtime.backend = Some(installed.backend);
                        // First output proves readiness; client counts arrive through Control Mode.
                    },
                    Err(error) => {
                        if error.is::<crate::host::terminal_runtime::TerminalSessionEnded>() {
                            runtime.ended(&mut consumer)?;
                        } else {
                            runtime.retry.failed();
                            consumer.mark_unavailable(ProtocolError::new("terminal_backend_unavailable", "The terminal connection could not be verified", true))?;
                        }
                    },
                }
                local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
            }
            event = session_events.recv() => {
                if matches!(event, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break Ok(()); }
                if let Some(binding) = consumer.binding() {
                    let relevant = match &event {
                        Ok(event) => event.changes.iter().any(|change| match change {
                            polar_tmux_protocol::SessionFeedChangeV1::Upsert { session } => session.key == SessionKeyV1::from(binding.clone()),
                            polar_tmux_protocol::SessionFeedChangeV1::Remove { key } => *key == SessionKeyV1::from(binding.clone()),
                        }),
                        Err(_) => true,
                    };
                    if !relevant { continue; }
                    let key = SessionKeyV1::from(binding.clone());
                    let current = match event {
                        Ok(event) => event.changes.into_iter().find_map(|change| match change {
                            polar_tmux_protocol::SessionFeedChangeV1::Upsert { session } if session.key == key => Some((session.key, session.lifecycle, session.transport_generation)),
                            _ => None,
                        }),
                        Err(_) => state.sessions.snapshot().await.sessions.into_iter().find(|s| s.key == key)
                            .map(|s| (s.key, s.lifecycle, s.transport_generation)),
                    };
                    let changed = current != observed_session_state;
                    observed_session_state = current.clone();
                    if changed && current.as_ref().is_none_or(|(_, status, _)| *status != SessionLifecycleV1::Ready)
                        && runtime.session_unavailable(&mut consumer, current.as_ref().map(|(_, status, _)| *status))?
                    {
                        local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                    }
                    if changed && runtime.backend.is_none() && runtime.pending_attach.is_none() && current.is_some_and(|(_, status, _)| status == SessionLifecycleV1::Ready) {
                        runtime.start(&state, &consumer);
                    }
                }
            }
            frame = &mut reading => {
                drop(reading);
                reading = Box::pin(local_ipc::read_terminal_bridge_frame(&mut reader));
                let Some(frame) = frame? else {
                    break Ok(());
                };
                match frame {
                    TerminalBridgeFrameV1::RetainBinding { binding_generation } => {
                        consumer.retain_binding(binding_generation)?;
                        runtime.control.pending = None;
                        runtime.control.status.binding_generation = binding_generation;
                        if let Some(intent) = runtime.control.intent.as_mut() { intent.binding_generation = binding_generation; }
                        local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                    }
                    TerminalBridgeFrameV1::Control(intent) => {
                        if !control_v2 || intent.binding_generation != consumer.status().content_generation { continue; }
                        let Some(epoch) = state.terminal_consumers.select(&intent.native_instance_id, intent.sequence, &consumer_id, consumer_generation, intent.focused).await else { continue; };
                        runtime.control.epoch = Some(epoch);
                        let focused = intent.focused;
                        let cause = intent.cause;
                        let size = intent.size;
                        if runtime.control.accept(intent) {
                            // A rejected viewport cannot change the cached size
                            // that the next genuine input will reconcile.
                            consumer.resize(size)?;
                            if consumer.status().lifecycle == TerminalConsumerLifecycleV1::Ready {
                                if let Some(backend) = runtime.backend.as_mut() {
                                    if cause != polar_tmux_protocol::TerminalControlCauseV1::Viewport {
                                        backend.focus_epoch(focused, runtime.control.epoch.clone()).await?;
                                    }
                                    runtime.control.reconcile(backend, consumer.terminal_size())?;
                                }
                            }
                        }
                        local_ipc::write_terminal_bridge_frame(&mut writer, &TerminalBridgeFrameV1::ControlStatus(runtime.control.status.clone())).await?;
                    }
                    TerminalBridgeFrameV1::Focus { focused, binding_generation, size } => {
                        focus_ordinal += 1;
                        tracing::info!(target: "polar_station_operation", component="Host", event="bridge_focus_received", consumer_id=consumer_id.as_str(), generation=consumer_generation, content_generation=binding_generation, ordinal=focus_ordinal, focused, columns=size.columns, rows=size.rows);
                        if binding_generation == consumer.status().content_generation && consumer.status().lifecycle == TerminalConsumerLifecycleV1::Ready {
                            consumer.resize(size)?;
                            runtime.control.accept(polar_tmux_protocol::TerminalControlIntentV1 {
                                native_instance_id: format!("legacy-{consumer_id}"), sequence: focus_ordinal,
                                binding_generation, viewport_revision: focus_ordinal, focused,
                                cause: polar_tmux_protocol::TerminalControlCauseV1::Selection, size,
                            });
                            if let Some(active) = runtime.backend.as_mut() {
                                active.focus(focused).await?;
                                runtime.control.reconcile(active, size)?;
                            }
                        }
                    }
                    TerminalBridgeFrameV1::DismissRecovery => {} // Legacy compatibility; no warning remains.
                    TerminalBridgeFrameV1::Retry => {
                        if consumer.binding().is_some() && !matches!(consumer.status().lifecycle, TerminalConsumerLifecycleV1::Ready | TerminalConsumerLifecycleV1::Ended) && runtime.pending_attach.is_none() && runtime.backend.is_none() {
                            runtime.retry.reset();
                            consumer.transport_lost(None)?;
                            runtime.start(&state, &consumer);
                        }
                    }
                    TerminalBridgeFrameV1::Input(bytes) => {
                        if consumer.status().lifecycle == TerminalConsumerLifecycleV1::Ready {
                            let activity = runtime.input_activity.observe(&bytes);
                            if activity { runtime.control.retry_on_activity(); }
                            if !control_v2 && activity && !runtime.control.intent.as_ref().is_some_and(|intent| intent.focused) {
                                focus_ordinal += 1;
                                runtime.control.accept(polar_tmux_protocol::TerminalControlIntentV1 {
                                    native_instance_id: format!("legacy-{consumer_id}"), sequence: focus_ordinal,
                                    binding_generation: consumer.status().content_generation, viewport_revision: focus_ordinal,
                                    focused: true, cause: polar_tmux_protocol::TerminalControlCauseV1::Selection, size: consumer.terminal_size(),
                                });
                            }
                            if activity && control_v2 && runtime.control.epoch.is_none() {
                                if *selection.borrow() {
                                    if let Some(intent) = runtime.control.intent.as_ref() {
                                        runtime.control.epoch = state.terminal_consumers.select(&intent.native_instance_id, intent.sequence, &consumer_id, consumer_generation, true).await;
                                    }
                                } else { runtime.control.blur(); }
                            }
                            if let Some(active) = runtime.backend.as_mut() {
                                if activity { runtime.control.reconcile(active, consumer.terminal_size())?; }
                                active.write(bytes).await?;
                            }
                        } else if consumer.binding().is_some() && consumer.status().lifecycle != TerminalConsumerLifecycleV1::Ended {
                            tracing::debug!(target: "polar_station_operation", component="Terminal", event="terminal_input_discarded", discarded_bytes=bytes.len());
                            // Human input is a wakeup, never a replayable command.
                            if runtime.pending_attach.is_none() && runtime.backend.is_none() {
                                runtime.retry.reset();
                                consumer.transport_lost(None)?;
                                runtime.start(&state, &consumer);
                            }
                            local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                        }
                    }
                    TerminalBridgeFrameV1::Resize(size) => {
                        consumer.resize(size)?;
                        if let Some(active) = runtime.backend.as_mut() { runtime.control.reconcile(active, size)?; }
                    }
                    TerminalBridgeFrameV1::Bind {
                        binding,
                        binding_generation,
                    } => {
                        runtime.reset();
                        observed_session_state = None;
                        consumer
                            .begin_binding(binding, binding_generation)
                            .context("invalid terminal binding transition")?;
                        state
                            .terminal_consumers
                            .update_binding(
                                &consumer_id,
                                consumer_generation,
                                consumer.binding().cloned(),
                            )
                            .await;
                        local_ipc::write_terminal_bridge_frame(
                            &mut writer,
                            &consumer.status_frame(),
                        )
                        .await?;
                        runtime.start(&state, &consumer);
                        local_ipc::write_terminal_bridge_frame(
                            &mut writer,
                            &consumer.status_frame(),
                        )
                        .await?;
                    }
                    TerminalBridgeFrameV1::Detach { binding_generation } => {
                        runtime.reset();
                        detach_terminal(&mut consumer, &mut runtime.backend, binding_generation)?;
                        state
                            .terminal_consumers
                            .update_binding(&consumer_id, consumer_generation, None)
                            .await;
                        local_ipc::write_terminal_bridge_frame(
                            &mut writer,
                            &consumer.status_frame(),
                        )
                        .await?;
                    }
                    TerminalBridgeFrameV1::Retire => break Ok(()),
                    TerminalBridgeFrameV1::Output(_) | TerminalBridgeFrameV1::Status(_) | TerminalBridgeFrameV1::ControlStatus(_) => {
                        break Err(anyhow::anyhow!(
                            "terminal Consumer sent a Host-only terminal frame"
                        ));
                    }
                }
            }
            event = async {
                match runtime.backend.as_mut() {
                    Some(active) => active.next_event().await,
                    None => std::future::pending().await,
                }
            } => {
                match event {
                    Some(BackendPtyEvent::Output(bytes)) => {
                        if runtime.backend
                            .as_mut()
                            .is_some_and(BackendPty::take_first_output)
                        {
                            tracing::info!(target: "polar_station_operation", component="Host", event="pane_first_byte",
                                consumer_id=consumer_id.as_str(), generation=consumer_generation, content_generation=consumer.status().content_generation);
                            runtime.first_redraw(&mut consumer)?;
                            local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                            state.ssh_connections.record_consumer_recovery_stage("pane_first_byte");
                        }
                        let span = tracing::info_span!(target: "polar_station_operation", "terminal_bridge_output_write", component="Host",
                            consumer_id=consumer_id.as_str(), sequence=runtime.control.status.sequence,
                            operation_id=format!("{}-{}", consumer_id, runtime.control.status.sequence), bytes=bytes.len(), outcome="cancelled");
                        tracing::info!(target: "polar_station_operation", component="Host", event="terminal_bridge_output_write_execute",
                            consumer_id=consumer_id.as_str(), bytes=bytes.len());
                        let frame = TerminalBridgeFrameV1::Output(bytes);
                        let written = local_ipc::write_terminal_bridge_frame(
                            &mut writer,
                            &frame,
                        )
                        .instrument(span.clone()).await;
                        span.record("outcome", if written.is_ok() { "ok" } else { "failed" });
                        written?;
                    }
                    Some(BackendPtyEvent::Exited { code, signal, error }) => {
                        if let Some(active) = runtime.backend.take() { active.stop(); }
                        state.ssh_connections.record_terminal_exit(code, signal.clone());
                        warn!(consumer_id = consumer.status().consumer_id, ?code, ?signal, observation_failed = error.is_some(), "terminal attachment exited; Session existence requires observation");
                        runtime.failed(&mut consumer)?;
                        if let Some(binding) = consumer.binding() {
                            if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(binding.execution_target.to_string()) {
                                state.session_lifecycle.request_refresh(target);
                            }
                        }
                        local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                    }
                    None => {
                        if let Some(active) = runtime.backend.take() { active.stop(); }
                        runtime.failed(&mut consumer)?;
                        local_ipc::write_terminal_bridge_frame(&mut writer, &consumer.status_frame()).await?;
                    }
                }
            }
            event = transport_events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let Some(binding) = consumer.binding() else {
                            continue;
                        };
                        let Some(change) = state
                            .ssh_connections
                            .current_status_change(binding.execution_target.as_str())
                        else {
                            continue;
                        };
                        change
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                };
                let Some(binding) = consumer.binding() else {
                    continue;
                };
                if binding.execution_target.as_str() != event.target_id {
                    continue;
                }
                let Some(event) = state
                    .ssh_connections
                    .current_status_change(binding.execution_target.as_str())
                else {
                    continue;
                };
                match event.status {
                    SshConnectionStatus::Connecting
                    | SshConnectionStatus::Recovering
                    | SshConnectionStatus::Offline => {
                        tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_transport_invalidated", count=usize::from(runtime.pending_attach.is_some()), reason=match event.status {
                            SshConnectionStatus::Connecting => "connecting",
                            SshConnectionStatus::Recovering => "recovering",
                            SshConnectionStatus::Offline => "offline",
                            SshConnectionStatus::Connected => "connected",
                        });
                        runtime.pending_attach = None;
                        if matches!(
                            consumer.status().lifecycle,
                            TerminalConsumerLifecycleV1::Ready
                                | TerminalConsumerLifecycleV1::Connecting
                                | TerminalConsumerLifecycleV1::Unavailable
                        ) {
                            if let Some(active) = runtime.backend.take() {
                                active.stop();
                            }
                            if consumer.transport_lost(None)? == RecoveryDisposition::Recover {
                                runtime.cancel();
                                runtime.retry.failed();
                                local_ipc::write_terminal_bridge_frame(
                                    &mut writer,
                                    &consumer.status_frame(),
                                )
                                .await?;
                            }
                        }
                    }
                    SshConnectionStatus::Connected => {
                        let latest = state
                            .ssh_connections
                            .transport_generation(binding.execution_target.as_str())
                            .await;
                        if !matches!(consumer.status().lifecycle, TerminalConsumerLifecycleV1::Ended | TerminalConsumerLifecycleV1::Detached)
                            && runtime.pending_attach.is_none()
                            && (runtime.backend.is_none() || consumer.requires_transport_reconcile(latest.as_ref()))
                        {
                            runtime.retire_backend();
                            let _ = consumer.transport_lost(None)?;
                            runtime.start(&state, &consumer);
                            local_ipc::write_terminal_bridge_frame(
                                &mut writer,
                                &consumer.status_frame(),
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        }
    }
    .await;
    runtime.cancel();
    state
        .terminal_consumers
        .unregister(&consumer_id, consumer_generation)
        .await;
    result
}

#[cfg(unix)]
fn start_terminal_attach(
    state: &TerminalServices,
    consumer: &TerminalConsumerState,
) -> impl std::future::Future<Output = Result<TerminalAttachment>> + Send + 'static {
    let consumer = consumer.clone();
    let targets = state.targets.clone();
    let ssh = state.ssh_connections.clone();
    let lifecycle = state.session_lifecycle.clone();
    let attempt = attach_terminal(
        consumer,
        targets,
        ssh,
        lifecycle,
        state.terminal_consumers.lifetimes.clone(),
    );
    async move {
        tokio::time::timeout(Duration::from_secs(15), attempt)
            .await
            .context("Terminal connection probe timed out")?
    }
}


