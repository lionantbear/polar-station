mod lifetime;
pub(crate) use lifetime::TerminalLifetimes;
mod control;
pub(crate) use control::TerminalControl;
mod registry;

pub(crate) use registry::TerminalConsumerRegistry;
#[cfg(unix)]
mod bridge;
#[cfg(unix)]
pub(crate) use bridge::{serve_bridge, TerminalServices};

// Host-owned terminal Consumer state and backend PTY process.

use crate::terminal_io::AsyncTerminalFd;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use tracing::Instrument;

use anyhow::{Context, Result};
use polar_tmux_core::{TmuxClient, TmuxTerminalAttachPlan, TmuxTerminalClient};
use polar_tmux_protocol::{
    PaneContentBindingV1, ProtocolError, ProtocolValidationError, SshTransportGenerationV1,
    TerminalBridgeHelloV1, TerminalConsumerLifecycleV1, TerminalConsumerStatusV1, TerminalSizeV1,
    TmuxAttachPlan, TMUX_EXECUTION_PATH,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand_core::RngCore;
use tokio::sync::{mpsc, watch};

#[derive(Debug)]
pub(crate) struct TerminalSessionEnded;
impl std::fmt::Display for TerminalSessionEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The bound tmux Session no longer exists")
    }
}
impl std::error::Error for TerminalSessionEnded {}

/// Per-attachment retry budget, serviced by the existing bridge select loop.
/// No task, queue, transport, or work-recovery ownership lives here.
#[derive(Default)]
pub(crate) struct TerminalRetry {
    failures: usize,
    pub(crate) deadline: Option<tokio::time::Instant>,
}
impl TerminalRetry {
    pub(crate) fn failed(&mut self) {
        self.deadline = [1, 2, 5, 10, 30]
            .get(self.failures)
            .map(|seconds| tokio::time::Instant::now() + std::time::Duration::from_secs(*seconds));
        self.failures += 1;
    }
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn pause(&mut self) {
        self.deadline = None;
    }
}

pub(crate) struct TerminalAttachment {
    pub(crate) backend: BackendPty,
    pub(crate) transport_generation: Option<SshTransportGenerationV1>,
}

/// Dropping this future cancels preparation; the bridge owns one attempt per
/// Consumer and drops it on Bind, Detach, retirement or transport replacement.
#[tracing::instrument(target = "polar_station_operation", name = "pane_backend_prepare", skip_all,
    fields(component="Host", consumer_id=consumer.status().consumer_id.as_str(), content_generation=consumer.status().content_generation, outcome="cancelled"))]
pub(crate) async fn attach_terminal(
    consumer: TerminalConsumerState,
    targets: crate::target_registry::TargetRegistry,
    ssh: crate::ssh_connection_manager::SshConnectionManager,
    lifecycle: std::sync::Arc<super::session_lifecycle::SessionLifecycle>,
    lifetimes: TerminalLifetimes,
) -> Result<TerminalAttachment> {
    ssh.record_consumer_recovery_stage("pane_attach_started");
    let binding = consumer
        .binding()
        .context("terminal Consumer has no binding")?;
    let target = targets
        .get(binding.execution_target.as_str())
        .context("terminal target missing")?;
    tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_attach_requested", consumer_id=consumer.status().consumer_id.as_str());
    let attach = tracing::Instrument::instrument(target.managed_client_for_attach(&ssh),
        tracing::info_span!(target: "polar_station_operation", "terminal_lease_wait", component="Terminal")).await?;
    ssh.record_consumer_recovery_stage("pane_lease_ready");
    tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_attach_execute", consumer_id=consumer.status().consumer_id.as_str());
    let current = lifecycle
        .attachment_observation_current(
            binding.execution_target.as_str(),
            attach.transport_generation.as_ref(),
        )
        .await;
    let previous = if attach.transport_generation.is_some() && !current {
        lifecycle
            .previous_attachment_binding(
                binding.execution_target.as_str(),
                &binding.tmux_session_id,
            )
            .await
    } else {
        None
    };
    // The same remote identity guard runs before attach. Its existing receipt
    // can confirm this binding without waiting for catalog observation.
    let speculative_trace = previous.as_ref().map(|_| {
        tracing::info_span!(
        target: "polar_station_operation", "terminal_speculative_attempt",
        component="Terminal", outcome="discarded")
    });
    let speculative = if let Some(identity) = previous {
        let plan = attach
            .client
            .prepare_attach_known_session(&identity.session_id)?;
        tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_speculative_started");
        Some((
            spawn_backend(&consumer, plan, Some(identity.clone()), false, &lifetimes).await?,
            identity,
        ))
    } else {
        None
    };
    let mut prepared = if attach.transport_generation.is_some() && !current && speculative.is_none()
    {
        // The placeholder only builds the existing SSH argv. No command may
        // attach it: the remote shell waits for the exact observed identity.
        let plan = attach.client.prepare_attach_known_session("$0")?;
        Some(spawn_backend(&consumer, plan, None, true, &lifetimes).await?)
    } else {
        None
    };
    let remote_confirmed = attachment_confirmed_remotely(
        speculative.as_ref().map(|(backend, _)| backend.client_identity.clone()),
        lifecycle.wait_for_attachment_observation(binding.execution_target.as_str(), &ssh)
            .instrument(tracing::info_span!(target: "polar_station_operation", "terminal_observation_wait", component="Terminal")),
    ).await;
    if let Some(backend) = prepared.as_mut() {
        backend.wait_prepared().await?;
    }
    let gate = target.operation_gate();
    let _operation = tracing::Instrument::instrument(gate.read(),
        tracing::info_span!(target: "polar_station_operation", "terminal_gate_wait", component="Terminal")).await;
    let identity = if remote_confirmed {
        tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_remote_attach_confirmed");
        speculative
            .as_ref()
            .expect("receipt requires a guarded backend")
            .1
            .clone()
    } else {
        match lifecycle
            .attachment_binding(
                binding.execution_target.as_str(),
                &binding.tmux_session_id,
                attach.transport_generation.as_ref(),
            )
            .await
        {
            Ok(Some(identity)) => identity,
            Ok(None) => return Err(TerminalSessionEnded.into()),
            Err(error) => {
                if let Ok(target) = polar_tmux_workspace::ExecutionTargetId::new(
                    binding.execution_target.to_string(),
                ) {
                    lifecycle.request_refresh_if_idle(target);
                }
                return Err(error);
            }
        }
    };
    let plan = attach
        .client
        .prepare_attach_known_session(&identity.session_id)?;
    anyhow::ensure!(
        attach.transport_generation
            == ssh
                .transport_generation(binding.execution_target.as_str())
                .await,
        "transport changed during attach"
    );
    ssh.record_consumer_recovery_stage("pane_binding_ready");
    let speculative = match speculative {
        Some((backend, previous)) if previous == identity => {
            if let Some(trace) = &speculative_trace {
                trace.record("outcome", "reused");
            }
            tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_speculative_reused");
            Some(backend)
        }
        Some((backend, _)) => {
            drop(backend);
            tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_speculative_discarded");
            None
        }
        None => None,
    };
    let mut backend = if let Some(backend) = speculative {
        backend
    } else if let Some(mut backend) = prepared {
        backend.complete_preparation(&plan, &identity).await?;
        backend
    } else {
        spawn_backend(&consumer, plan, Some(identity), false, &lifetimes).await?
    };
    backend.trace_consumer_id = consumer.status().consumer_id.clone();
    backend.size_client_provider = Some((target, ssh, attach.transport_generation.clone()));
    tracing::info!(target: "polar_station_operation", component="Terminal", event="terminal_backend_released",
        reason=if remote_confirmed { "remote_receipt" } else { "observation" });
    tracing::Span::current().record("outcome", "ok");
    Ok(TerminalAttachment {
        backend,
        transport_generation: attach.transport_generation,
    })
}

/// The private post-attach receipt proves the remote guard succeeded. Reuse the
/// existing observation as fallback if it completes first or receipt is absent.
/// In particular, a large initial redraw may backpressure the existing bounded
/// output queue before the post-attach receipt arrives: never wait for that
/// receipt after fresh observation already authorizes normal streaming.
async fn attachment_confirmed_remotely(
    identity: Option<watch::Receiver<Option<TmuxTerminalClient>>>,
    observation: impl std::future::Future<Output = ()>,
) -> bool {
    let Some(mut identity) = identity else {
        observation.await;
        return false;
    };
    tokio::pin!(observation);
    let receipt = async {
        loop {
            if identity.borrow_and_update().is_some() {
                return true;
            }
            if identity.changed().await.is_err() {
                return false;
            }
        }
    };
    tokio::select! {
        biased;
        confirmed = receipt => {
            if !confirmed { observation.await; }
            confirmed
        }
        _ = &mut observation => false,
    }
}

/// Reuse Tokio's blocking pool for portable-pty's synchronous fork/exec work.
/// If the caller is cancelled, the returned BackendPty is dropped and kills
/// only its owned child, including when spawn finishes after cancellation.
async fn spawn_backend(
    consumer: &TerminalConsumerState,
    plan: TmuxAttachPlan,
    binding: Option<polar_tmux_core::TmuxSessionBinding>,
    deferred: bool,
    lifetimes: &TerminalLifetimes,
) -> Result<BackendPty> {
    let ticket = lifetimes.acquire()?;
    let consumer = consumer.clone();
    let span = tracing::info_span!(target: "polar_station_operation", "terminal_spawn_dispatch", component="PTY");
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_spawn_worker_started");
        BackendPty::spawn_inner(&plan, binding.as_ref(), consumer.terminal_size(),
            consumer.terminal_type(), consumer.locale(), consumer.status().consumer_id.as_str(), deferred, ticket)
    }).await.context("terminal spawn worker failed")?
}

pub(crate) fn detach_terminal(
    consumer: &mut TerminalConsumerState,
    backend: &mut Option<BackendPty>,
    generation: u64,
) -> Result<()> {
    consumer.detach(generation)?;
    if let Some(backend) = backend.take() {
        backend.stop();
    }
    Ok(())
}

const TERMINAL_CHUNK_BYTES: usize = 16 * 1024;
const TERMINAL_QUEUE_CHUNKS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryDisposition {
    Recover,
    Stale,
    Detached,
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalConsumerState {
    status: TerminalConsumerStatusV1,
    recovery_feedback: bool,
    terminal_size: TerminalSizeV1,
    terminal_type: String,
    locale: Option<String>,
}

impl TerminalConsumerState {
    pub(crate) fn new(hello: TerminalBridgeHelloV1) -> Result<Self, ProtocolValidationError> {
        hello.validate()?;
        let lifecycle = if hello.initial_binding.is_some() {
            TerminalConsumerLifecycleV1::Connecting
        } else {
            TerminalConsumerLifecycleV1::Detached
        };
        let state = Self {
            recovery_feedback: hello.recovery_feedback,
            status: TerminalConsumerStatusV1 {
                recovery: None,
                consumer_id: hello.consumer_id,
                consumer_generation: hello.consumer_generation,
                binding: hello.initial_binding,
                lifecycle,
                transport_generation: None,
                content_generation: hello.initial_content_generation,
                error: None,
            },
            terminal_size: hello.terminal_size,
            terminal_type: hello.terminal_type,
            locale: hello.locale,
        };
        state.status.validate()?;
        Ok(state)
    }

    pub(crate) fn status(&self) -> &TerminalConsumerStatusV1 {
        &self.status
    }

    pub(crate) fn project_recovery(
        &mut self,
        retry: &TerminalRetry,
        busy: bool,
        last_probe_unix_ms: Option<u64>,
        recovered: bool,
    ) -> bool {
        if !self.recovery_feedback {
            return false;
        }
        use polar_tmux_protocol::TerminalRecoveryPhaseV1 as P;
        use TerminalConsumerLifecycleV1 as L;
        let phase = match self.status.lifecycle {
            L::Detached => P::Hidden,
            L::Ready if recovered => P::Recovered,
            L::Ready => P::Hidden,
            L::Ended => P::Ended,
            L::Connecting if busy => P::Probing,
            _ if busy => P::Attaching,
            _ if retry.deadline.is_some() => P::Waiting,
            _ => P::Paused,
        };
        let mut view = polar_tmux_core::terminal_feedback(
            phase,
            retry.deadline.map(|deadline| {
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_secs()
                    .max(1)
            }),
        );
        view.content_generation = self.status.content_generation;
        view.detail = self
            .status
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        view.last_probe_unix_ms = last_probe_unix_ms;
        if self.status.recovery.as_deref() == Some(&view) {
            return false;
        }
        self.status.recovery = Some(Box::new(view));
        true
    }

    pub(crate) fn terminal_size(&self) -> TerminalSizeV1 {
        self.terminal_size
    }

    pub(crate) fn terminal_type(&self) -> &str {
        &self.terminal_type
    }

    pub(crate) fn locale(&self) -> Option<&str> {
        self.locale.as_deref()
    }

    pub(crate) fn binding(&self) -> Option<&PaneContentBindingV1> {
        self.status.binding.as_ref()
    }

    pub(crate) fn begin_binding(
        &mut self,
        binding: PaneContentBindingV1,
        content_generation: u64,
    ) -> Result<(), ProtocolValidationError> {
        binding.validate()?;
        require_new_generation(content_generation, self.status.content_generation)?;
        self.status.binding = Some(binding);
        self.status.lifecycle = TerminalConsumerLifecycleV1::Connecting;
        self.status.transport_generation = None;
        self.status.content_generation = content_generation;
        self.status.error = None;
        self.status.validate()
    }

    pub(crate) fn retain_binding(
        &mut self,
        content_generation: u64,
    ) -> Result<(), ProtocolValidationError> {
        require_new_generation(content_generation, self.status.content_generation)?;
        self.status.content_generation = content_generation;
        if let Some(view) = self.status.recovery.as_mut() {
            view.content_generation = content_generation;
        }
        self.status.validate()
    }

    pub(crate) fn resize(&mut self, size: TerminalSizeV1) -> Result<(), ProtocolValidationError> {
        size.validate()?;
        self.terminal_size = size;
        Ok(())
    }

    pub(crate) fn mark_ready(
        &mut self,
        transport_generation: Option<SshTransportGenerationV1>,
    ) -> Result<(), ProtocolValidationError> {
        if self.status.binding.is_none() {
            return Err(ProtocolValidationError::InvalidIdentifier(
                "ready terminal binding",
            ));
        }
        if let Some(generation) = &transport_generation {
            generation.validate()?;
        }
        self.status.lifecycle = TerminalConsumerLifecycleV1::Ready;
        self.status.transport_generation = transport_generation;
        self.status.error = None;
        self.status.validate()
    }

    pub(crate) fn transport_lost(
        &mut self,
        observed_generation: Option<&SshTransportGenerationV1>,
    ) -> Result<RecoveryDisposition, ProtocolValidationError> {
        if self.status.binding.is_none() {
            return Ok(RecoveryDisposition::Detached);
        }
        if let (Some(current), Some(observed)) = (
            self.status.transport_generation.as_ref(),
            observed_generation,
        ) {
            if current != observed {
                return Ok(RecoveryDisposition::Stale);
            }
        }
        self.status.lifecycle = TerminalConsumerLifecycleV1::Recovering;
        self.status.error.get_or_insert_with(|| {
            ProtocolError::new(
                "transport_recovering",
                "Terminal attachment lost; checking the original Session",
                true,
            )
        });
        self.status.validate()?;
        Ok(RecoveryDisposition::Recover)
    }

    pub(crate) fn requires_transport_reconcile(
        &self,
        latest: Option<&SshTransportGenerationV1>,
    ) -> bool {
        self.status.binding.is_some()
            && !matches!(
                self.status.lifecycle,
                TerminalConsumerLifecycleV1::Detached | TerminalConsumerLifecycleV1::Ended
            )
            && self.status.transport_generation.as_ref() != latest
    }

    pub(crate) fn mark_unavailable(
        &mut self,
        error: ProtocolError,
    ) -> Result<(), ProtocolValidationError> {
        error.validate_sanitized()?;
        self.status.lifecycle = TerminalConsumerLifecycleV1::Unavailable;
        self.status.error = Some(error);
        self.status.validate()
    }

    pub(crate) fn mark_ended(&mut self) -> Result<(), ProtocolValidationError> {
        self.status.lifecycle = TerminalConsumerLifecycleV1::Ended;
        self.status.error = None;
        self.status.validate()
    }

    pub(crate) fn detach(
        &mut self,
        content_generation: u64,
    ) -> Result<(), ProtocolValidationError> {
        require_new_generation(content_generation, self.status.content_generation)?;
        self.status.binding = None;
        self.status.lifecycle = TerminalConsumerLifecycleV1::Detached;
        self.status.transport_generation = None;
        self.status.content_generation = content_generation;
        self.status.error = None;
        self.status.validate()
    }

    pub(crate) fn status_frame(&self) -> polar_tmux_protocol::TerminalBridgeFrameV1 {
        polar_tmux_protocol::TerminalBridgeFrameV1::Status(self.status.clone())
    }
}

fn require_new_generation(candidate: u64, current: u64) -> Result<(), ProtocolValidationError> {
    if candidate <= current {
        return Err(ProtocolValidationError::InvalidIdentifier(
            "terminal content generation",
        ));
    }
    Ok(())
}

pub(crate) struct BackendPty {
    master: std::sync::Weak<std::sync::Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>>,
    input: Option<mpsc::Sender<TerminalInput>>,
    events: mpsc::Receiver<BackendPtyEvent>,
    stop_request: Option<lifetime::Stop>,
    exited: watch::Receiver<Option<BackendPtyEvent>>,
    completion: Option<tokio::sync::oneshot::Receiver<Result<lifetime::Cleanup>>>,
    first_output_pending: bool,
    terminal_size: TerminalSizeV1,
    tmux_client: TmuxClient,
    size_client_provider: Option<(
        crate::target_registry::RegisteredTarget,
        crate::ssh_connection_manager::SshConnectionManager,
        Option<SshTransportGenerationV1>,
    )>,
    tmux_session: String,
    tmux_binding: Option<polar_tmux_core::TmuxSessionBinding>,
    client_identity: watch::Receiver<Option<TmuxTerminalClient>>,
    focused: Arc<AtomicBool>,
    trace_consumer_id: String,
    input_sequence: u64,
    control_sequence: Arc<AtomicU64>,
    preparation: Option<watch::Receiver<bool>>,
    identity_nonce: u64,
}

struct TerminalInput {
    bytes: Vec<u8>,
    span: tracing::Span,
    focus_guard: Option<(Arc<AtomicBool>, bool)>,
    epoch_guard: Option<polar_tmux_core::TmuxControlEpoch>,
}

/// Bounded private preparation prefix. Returns the number of bytes consumed
/// from this read once ready; ordinary terminal output after it is preserved.
fn consume_preparation_prefix(
    expected: &[u8],
    pending: &mut Vec<u8>,
    bytes: &[u8],
) -> Result<Option<usize>> {
    let needed = expected
        .len()
        .checked_sub(pending.len())
        .context("invalid preparation prefix")?;
    let taken = needed.min(bytes.len());
    pending.extend_from_slice(&bytes[..taken]);
    anyhow::ensure!(
        expected.starts_with(pending),
        "invalid preparation response"
    );
    Ok((pending.len() == expected.len()).then_some(taken))
}

/// Relay a real native focus event, never a key or a terminal query response.
async fn forward_terminal_focus(
    input: &mpsc::Sender<TerminalInput>,
    focused: bool,
    consumer_id: &str,
    state: &Arc<AtomicBool>,
    epoch: Option<polar_tmux_core::TmuxControlEpoch>,
) -> Result<()> {
    let span = tracing::info_span!(target: "polar_station_operation", "terminal_focus_forwarded", component="Host", consumer_id, reason=if focused { "enter" } else { "leave" }, outcome="cancelled");
    input
        .send(TerminalInput {
            bytes: if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            },
            span,
            focus_guard: Some((state.clone(), focused)),
            epoch_guard: epoch,
        })
        .await
        .context("Host PTY focus input closed")
}

/// Only recognizes automatic terminal replies, never transforms input bytes.
/// Partial escape sequences are retained solely to decide size participation.
#[derive(Default)]
struct TerminalInputActivity {
    pending: Vec<u8>,
}

impl TerminalInputActivity {
    fn observe(&mut self, input: &[u8]) -> bool {
        if input.is_empty() {
            return false;
        }
        if self.pending.len() + input.len() > 4096 {
            self.pending.clear();
            return true;
        }
        self.pending.extend_from_slice(input);
        let mut offset = 0;
        while offset < self.pending.len() {
            let tail = &self.pending[offset..];
            if tail[0] != 0x1b {
                self.pending.clear();
                return true;
            }
            if tail.len() == 1 {
                break;
            }
            let end = match tail[1] {
                b'[' => {
                    let Some(end) = tail[2..]
                        .iter()
                        .position(|byte| (0x40..=0x7e).contains(byte))
                        .map(|n| n + 2)
                    else {
                        break;
                    };
                    let reply = match tail[end] {
                        b'I' | b'O' => end == 2, // focus reports
                        b'c' | b'R' | b'n' | b't' | b'y' => {
                            end > 2
                                && tail[2..end]
                                    .iter()
                                    .all(|b| b.is_ascii_digit() || b";?>=$".contains(b))
                        }
                        _ => false,
                    };
                    if !reply {
                        self.pending.clear();
                        return true;
                    }
                    end + 1
                }
                b']' | b'P' => {
                    let Some(end) = tail.iter().enumerate().skip(2).find_map(|(i, b)| {
                        if *b == 7 || (*b == b'\\' && tail[i - 1] == 0x1b) {
                            Some(i + 1)
                        } else {
                            None
                        }
                    }) else {
                        break;
                    };
                    end
                }
                _ => {
                    self.pending.clear();
                    return true;
                }
            };
            offset += end;
        }
        self.pending.drain(..offset);
        false
    }
}

struct TerminalIdentityPreamble {
    prefix: Vec<u8>,
    pending: Vec<u8>,
    pid: Option<u32>,
    name: Option<String>,
}

impl TerminalIdentityPreamble {
    fn new(prefix: String) -> Self {
        Self {
            prefix: prefix.into_bytes(),
            pending: Vec::new(),
            pid: None,
            name: None,
        }
    }

    fn consume(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.pid.is_some() {
            return Ok(bytes.to_vec());
        }
        self.pending.extend_from_slice(bytes);
        if let Some(start) = self
            .pending
            .windows(self.prefix.len())
            .position(|w| w == self.prefix)
        {
            let digits_start = start + self.prefix.len();
            if let Some(length) = self.pending[digits_start..]
                .iter()
                .position(|b| *b == b'\n')
            {
                let end = digits_start + length;
                let digits = self.pending[digits_start..end]
                    .strip_suffix(b"\r")
                    .unwrap_or(&self.pending[digits_start..end]);
                let receipt = std::str::from_utf8(digits)?;
                let (digits, name) = receipt
                    .split_once('\t')
                    .context("terminal client name missing")?;
                let pid = digits.parse::<u32>()?;
                TmuxTerminalClient {
                    pid,
                    name: name.to_owned(),
                }
                .validate()?;
                self.name = Some(name.to_owned());
                anyhow::ensure!(pid != 0, "terminal child PID is zero");
                self.pid = Some(pid);
                self.pending.drain(start..=end);
                return Ok(std::mem::take(&mut self.pending));
            }
        }
        // Only retain a possible fragmented identity, never an entire redraw.
        // Content may legitimately arrive before tmux's post-attach job runs.
        let retained = if let Some(start) = self
            .pending
            .windows(self.prefix.len())
            .position(|w| w == self.prefix)
        {
            anyhow::ensure!(
                self.pending.len() - start <= self.prefix.len() + 268,
                "terminal child identity is malformed"
            );
            self.pending.len() - start
        } else {
            (1..self.prefix.len())
                .rev()
                .find(|n| self.pending.ends_with(&self.prefix[..*n]))
                .unwrap_or(0)
        };
        let emit = self.pending.len() - retained;
        Ok(self.pending.drain(..emit).collect())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum BackendPtyEvent {
    Output(Vec<u8>),
    Exited {
        code: Option<u32>,
        signal: Option<String>,
        error: Option<String>,
    },
}

impl BackendPty {
    

    

    #[allow(clippy::too_many_arguments)]
    fn spawn_inner(
        plan: &TmuxAttachPlan,
        binding: Option<&polar_tmux_core::TmuxSessionBinding>,
        size: TerminalSizeV1,
        terminal_type: &str,
        locale: Option<&str>,
        trace_consumer_id: &str,
        deferred: bool,
        mut ticket: lifetime::Ticket,
    ) -> Result<Self> {
        let mut nonce = rand_core::OsRng.next_u64();
        while nonce == 0 {
            nonce = rand_core::OsRng.next_u64();
        }
        let mut guarded = match binding {
            Some(binding) => TmuxTerminalAttachPlan::new_bound(plan, nonce, binding),
            None => TmuxTerminalAttachPlan::new(plan, nonce),
        }
        .context("Host produced an invalid attach plan")?;
        let expected_preparation = if deferred {
            Some(guarded.defer_until_binding()?)
        } else {
            None
        };
        let (preparation_sender, preparation) = watch::channel(false);
        let preparation = deferred.then_some(preparation);
        size.validate().context("terminal dimensions are invalid")?;
        let trace_operation_id = format!("terminal-{nonce:016x}");
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_pty_open_begin",
            consumer_id=trace_consumer_id, operation_id=trace_operation_id.as_str());
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("failed to open Host terminal PTY")?;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_pty_open_end",
            consumer_id=trace_consumer_id, operation_id=trace_operation_id.as_str());
        let mut command = CommandBuilder::new(&guarded.program);
        command.args(&guarded.args);
        command.env("TERM", terminal_type);
        if let Some(locale) = locale {
            command.env("LANG", locale);
        }
        command.env("PATH", TMUX_EXECUTION_PATH);
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_child_spawn_begin",
            consumer_id=trace_consumer_id, operation_id=trace_operation_id.as_str());
        // All fallible descriptor setup precedes child creation.
        let reader = Arc::new(AsyncTerminalFd::duplicate(
            pair.master
                .as_raw_fd()
                .context("Host PTY descriptor is missing")?,
        )?);
        let mut child = pair
            .slave
            .spawn_command(command)
            .context("failed to start Host terminal child")?;
        ticket.child_started();
        let child_pid = child.process_id().unwrap_or(0);
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_child_spawned",
            child_pid, consumer_id=trace_consumer_id, operation_id=trace_operation_id.as_str());
        drop(pair.slave);
        let killer = child.clone_killer();
        let writer = reader.clone();
        let retirement_reader = reader.clone();

        let (input_sender, mut input_receiver) =
            mpsc::channel::<TerminalInput>(TERMINAL_QUEUE_CHUNKS);
        let input_task = tokio::spawn(async move {
            while let Some(input) = input_receiver.recv().await {
                if input
                    .epoch_guard
                    .as_ref()
                    .is_some_and(|epoch| !epoch.is_current())
                {
                    input.span.record("outcome", "superseded");
                    continue;
                }
                if input
                    .focus_guard
                    .as_ref()
                    .is_some_and(|(state, expected)| state.load(Ordering::Acquire) != *expected)
                {
                    input.span.record("outcome", "superseded");
                    continue;
                }
                let written = {
                    let span = tracing::info_span!(target: "polar_station_operation", parent: &input.span, "pty_write", component="PTY", bytes=input.bytes.len(), outcome="cancelled");
                    let result = writer
                        .write_all(&input.bytes)
                        .instrument(span.clone())
                        .await;
                    span.record("outcome", if result.is_ok() { "ok" } else { "failed" });
                    result
                };
                input
                    .span
                    .record("outcome", if written.is_ok() { "ok" } else { "failed" });
                if written.is_err() {
                    break;
                }
            }
        });
        let (event_sender, event_receiver) = mpsc::channel(TERMINAL_QUEUE_CHUNKS);
        let output_sender = event_sender.clone();
        let (identity_sender, client_identity) = watch::channel(None);
        let control_sequence = Arc::new(AtomicU64::new(0));
        let reader_control_sequence = control_sequence.clone();
        let reader_consumer_id = trace_consumer_id.to_owned();
        let reader_operation_id = trace_operation_id.clone();
        let reader_task = tokio::spawn(async move {
            let trace_operation_id = reader_operation_id;
            let mut preamble = TerminalIdentityPreamble::new(guarded.identity_prefix);
            let mut first_content = true;
            let mut first_read = true;
            let mut buffer = vec![0_u8; TERMINAL_CHUNK_BYTES];
            let mut preparation_pending = expected_preparation;
            let mut preparation_bytes = Vec::new();
            loop {
                let read_result = {
                    let span = tracing::info_span!(target: "polar_station_operation", "terminal_pty_read_wait", component="PTY",
                        consumer_id=reader_consumer_id.as_str(), operation_id=trace_operation_id.as_str(), bytes=tracing::field::Empty, outcome="cancelled");
                    tracing::info!(target: "polar_station_operation", parent: &span, component="PTY", event="terminal_pty_read_execute",
                        consumer_id=reader_consumer_id.as_str(), operation_id=trace_operation_id.as_str());
                    let result = reader.read(&mut buffer).instrument(span.clone()).await;
                    span.record("bytes", result.as_ref().copied().unwrap_or(0));
                    span.record(
                        "outcome",
                        if result.is_ok() {
                            "ok"
                        } else {
                            "closed_or_failed"
                        },
                    );
                    result
                };
                match read_result {
                    Ok(0) | Err(_) => {
                        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_reader_closed",
                            consumer_id=reader_consumer_id.as_str(), operation_id=trace_operation_id.as_str(),
                            bytes=preamble.pending.len(), outcome=if preamble.pid.is_some() { "identity_received" } else { "identity_missing" });
                        return;
                    }
                    Ok(read) => {
                        let sequence = reader_control_sequence.load(Ordering::Acquire);
                        let output_operation_id: std::borrow::Cow<'_, str> = if sequence > 0
                            && tracing::enabled!(target: "polar_station_operation", tracing::Level::INFO)
                        {
                            std::borrow::Cow::Owned(format!("{reader_consumer_id}-{sequence}"))
                        } else {
                            std::borrow::Cow::Borrowed(&trace_operation_id)
                        };
                        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_pty_read_completed",
                            consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=read);
                        if first_read {
                            tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_pty_first_read",
                                child_pid, consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=read);
                            first_read = false;
                        }
                        let mut content = &buffer[..read];
                        if let Some(expected) = preparation_pending.as_ref() {
                            let taken = match consume_preparation_prefix(
                                expected,
                                &mut preparation_bytes,
                                content,
                            ) {
                                Ok(Some(taken)) => taken,
                                Ok(None) => continue,
                                Err(_) => return,
                            };
                            preparation_pending = None;
                            preparation_sender.send_replace(true);
                            tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_channel_prepared",
                                consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence);
                            content = &content[taken..];
                            if content.is_empty() {
                                continue;
                            }
                        }
                        let had_identity = preamble.pid.is_some();
                        let output = match preamble.consume(content) {
                            Ok(output) => output,
                            Err(_) => {
                                tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_identity_invalid",
                                    consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=preamble.pending.len());
                                let _ = output_sender
                                    .send(BackendPtyEvent::Exited {
                                        code: None,
                                        signal: None,
                                        error: Some("Invalid terminal child identity".to_owned()),
                                    })
                                    .await;
                                return;
                            }
                        };
                        if !had_identity {
                            if let (Some(pid), Some(name)) = (preamble.pid, preamble.name.as_ref())
                            {
                                tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_exec_preamble_received",
                                    child_pid=pid, consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence);
                                identity_sender.send_replace(Some(TmuxTerminalClient {
                                    pid,
                                    name: name.clone(),
                                }));
                            }
                        }
                        if output.is_empty() {
                            continue;
                        }
                        if first_content {
                            tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_content_first_read",
                                child_pid, consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=output.len());
                            first_content = false;
                        }
                        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_output_enqueue_execute",
                            consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=output.len());
                        let span = tracing::info_span!(target: "polar_station_operation", "terminal_output_queue_wait", component="PTY",
                            consumer_id=reader_consumer_id.as_str(), operation_id=output_operation_id.as_ref(), sequence, bytes=output.len(), outcome="cancelled");
                        let sent = output_sender
                            .send(BackendPtyEvent::Output(output))
                            .instrument(span.clone())
                            .await;
                        span.record("outcome", if sent.is_ok() { "ok" } else { "failed" });
                        if sent.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let wait_task = tokio::task::spawn_blocking(move || match child.wait() {
            Ok(status) => BackendPtyEvent::Exited {
                code: Some(status.exit_code()),
                signal: status.signal().map(str::to_owned),
                error: None,
            },
            Err(error) => BackendPtyEvent::Exited {
                code: None,
                signal: None,
                error: Some(error.to_string()),
            },
        });
        drop(event_sender);
        let master = Arc::new(std::sync::Mutex::new(Some(pair.master)));
        let master_handle = Arc::downgrade(&master);
        let (stop, exited, completion) = lifetime::Owner {
            master,
            killer,
            waiter: wait_task,
            input: input_task,
            reader: reader_task,
            drain: Some(retirement_reader),
            ticket,
            child_pid,
            consumer_id: trace_consumer_id.to_owned(),
            operation_id: trace_operation_id,
        }
        .start();
        Ok(Self {
            master: master_handle,
            input: Some(input_sender),
            events: event_receiver,
            stop_request: Some(stop),
            exited,
            completion: Some(completion),
            first_output_pending: true,
            terminal_size: size,
            tmux_client: guarded
                .client
                .with_command_timeout(std::time::Duration::from_secs(2)),
            tmux_session: guarded.session_id,
            tmux_binding: binding.cloned(),
            size_client_provider: None,
            client_identity,
            focused: Arc::new(AtomicBool::new(false)),
            trace_consumer_id: String::new(),
            input_sequence: 0,
            control_sequence,
            preparation,
            identity_nonce: nonce,
        })
    }

    async fn wait_prepared(&mut self) -> Result<()> {
        let ready = self
            .preparation
            .as_mut()
            .context("terminal was not prepared")?;
        while !*ready.borrow_and_update() {
            ready
                .changed()
                .await
                .context("terminal preparation ended before readiness")?;
        }
        Ok(())
    }

    async fn complete_preparation(
        &mut self,
        plan: &TmuxAttachPlan,
        binding: &polar_tmux_core::TmuxSessionBinding,
    ) -> Result<()> {
        let guarded = TmuxTerminalAttachPlan::new_bound(plan, self.identity_nonce, binding)?;
        let bytes = guarded.deferred_binding_command()?;
        let span = tracing::info_span!(target: "polar_station_operation", "terminal_binding_write", component="PTY", outcome="cancelled");
        self.input
            .as_ref()
            .context("prepared terminal input closed")?
            .send(TerminalInput {
                bytes,
                span,
                focus_guard: None,
                epoch_guard: None,
            })
            .await
            .context("prepared terminal input closed")?;
        self.tmux_client = guarded
            .client
            .with_command_timeout(std::time::Duration::from_secs(2));
        self.tmux_session = guarded.session_id;
        self.tmux_binding = Some(binding.clone());
        self.preparation.take();
        Ok(())
    }

    pub(crate) async fn write(&mut self, bytes: Vec<u8>) -> Result<()> {
        let sender = self.input.as_ref().context("Host PTY input is closed")?;
        self.input_sequence = self.input_sequence.saturating_add(1);
        let span = tracing::info_span!(target: "polar_station_operation", "input_to_pty", component="Host",
            consumer_id=self.trace_consumer_id.as_str(), sequence=self.input_sequence, bytes=bytes.len(),
            queue_depth=sender.max_capacity()-sender.capacity(), outcome="cancelled");
        sender
            .send(TerminalInput {
                bytes,
                span,
                focus_guard: None,
                epoch_guard: None,
            })
            .await
            .context("Host PTY input queue closed")
    }

    /// Native focus is explicit; automatic terminal query replies remain passive.
    pub(crate) async fn focus(&mut self, focused: bool) -> Result<()> {
        self.focus_epoch(focused, None).await
    }

    pub(crate) async fn focus_epoch(
        &mut self,
        focused: bool,
        epoch: Option<polar_tmux_core::TmuxControlEpoch>,
    ) -> Result<()> {
        let was_focused = self.focused.swap(focused, Ordering::AcqRel);
        tracing::info!(target: "polar_station_operation", component="Host", event="terminal_focus_received", consumer_id=self.trace_consumer_id.as_str(), reason=if focused { "enter" } else { "leave" });
        if focused || was_focused {
            forward_terminal_focus(
                self.input.as_ref().context("Host PTY input is closed")?,
                focused,
                &self.trace_consumer_id,
                &self.focused,
                epoch,
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn next_event(&mut self) -> Option<BackendPtyEvent> {
        // Output keeps its ordering, while reaping never waits for its capacity.
        if let Ok(event) = self.events.try_recv() {
            return Some(event);
        }
        if let Some(event) = self.exited.borrow().clone() {
            return Some(event);
        }
        let event = tokio::select! {
            biased;
            event = self.events.recv() => event,
            exit = async { self.exited.wait_for(|event| event.is_some()).await.ok().and_then(|event| event.clone()) } => exit,
        };
        if event.is_some() {
            return event;
        }
        self.exited
            .wait_for(|event| event.is_some())
            .await
            .ok()
            .and_then(|event| event.clone())
    }

    pub(crate) fn take_first_output(&mut self) -> bool {
        std::mem::replace(&mut self.first_output_pending, false)
    }

    /// Recovery snapshots synchronize the child without granting size control.
    pub(crate) fn synchronize_size(&mut self, size: TerminalSizeV1) -> Result<()> {
        if self.terminal_size == size {
            return Ok(());
        }
        self.master
            .upgrade()
            .context("Host PTY is closed")?
            .lock()
            .map_err(|_| anyhow::anyhow!("Host PTY lock failed"))?
            .as_ref()
            .context("Host PTY is closed")?
            .resize(pty_size(size))
            .context("failed to resize Host terminal PTY")?;
        self.terminal_size = size;
        tracing::info!(target: "polar_station_operation", component="PTY", event="terminal_pty_size_applied", consumer_id=self.trace_consumer_id.as_str(), columns=size.columns, rows=size.rows);
        Ok(())
    }

    pub(crate) fn stop(mut self) -> tokio::sync::oneshot::Receiver<Result<lifetime::Cleanup>> {
        self.request_stop();
        self.completion.take().expect("terminal completion exists")
    }

    fn request_stop(&mut self) {
        if let Some(stop) = self.stop_request.take() {
            let _ = stop.send(());
            self.input.take();
        }
    }
}

impl Drop for BackendPty {
    fn drop(&mut self) {
        // Fallback only. The independent owner joins/reaps; Host awaits its ticket.
        self.request_stop();
    }
}

fn pty_size(size: TerminalSizeV1) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: size.pixel_width,
        pixel_height: size.pixel_height,
    }
}


