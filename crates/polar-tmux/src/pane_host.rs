//! Stable terminal byte bridge for one native Ghostty Surface.
//!
//! Native code owns the Surface and sends only content binding changes over
//! the private control socket. This process never starts tmux or SSH. It
//! forwards terminal bytes, input, and size to the central Rust Host, which
//! owns the backend PTY, child process, and transport recovery.

use std::{
    env,
    os::fd::RawFd,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::Instrument;

use anyhow::{bail, Context, Result};
use polar_tmux_core::{ContentChannel, ContentChannelAction, PendingContentTransition};
use polar_tmux_protocol::{
    ExecutionTarget, LocalClientFrame, PaneContentApiV1, PaneContentBindingV1,
    PaneContentCommandV1, PaneContentRequestV1, PaneContentResponseV1, PaneContentResultV1,
    ProtocolError, TerminalBridgeFrameV1, TerminalBridgeHelloV1, TerminalConsumerLifecycleV1,
    TerminalConsumerStatusV1, TerminalSizeV1, PROTOCOL_VERSION, SESSION_RUNTIME_API_VERSION,
};
use tokio::{
    net::UnixStream,
    signal::unix::{signal, SignalKind},
    sync::mpsc,
};

use crate::local_ipc;

const TERMINAL_CHUNK_BYTES: usize = 16 * 1024;
const BRIDGE_QUEUE_CAPACITY: usize = 64;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(50);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const STARTUP_REPLY_FILTER_WINDOW: Duration = Duration::from_secs(5);
const MAX_STARTUP_REPLY_BYTES: usize = 128;

use crate::terminal_io::AsyncTerminalFd;

struct TerminalModeGuard {
    fd: RawFd,
    original: libc::termios,
}

impl TerminalModeGuard {
    fn enter_stdin() -> Result<Self> {
        Self::enter_fd(libc::STDIN_FILENO)
    }

    fn enter_fd(fd: RawFd) -> Result<Self> {
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes `original` on success and `fd` is not
        // retained beyond the lifetime of this scoped guard.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to read Pane Host terminal mode");
        }
        // SAFETY: successful tcgetattr initialized the value.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: raw points to writable termios storage.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: raw contains a valid termios value for the live terminal.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to enter Pane Host raw terminal mode");
        }
        Ok(Self { fd, original })
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        // SAFETY: the saved termios value came from this descriptor. Failure
        // during best-effort process teardown cannot be reported from Drop.
        let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.original) };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConsumerConnectionIdentity {
    consumer_id: String,
    consumer_generation: u64,
}

impl ConsumerConnectionIdentity {
    fn new(initial_generation: u64) -> Self {
        Self {
            consumer_id: format!("consumer-{}-{initial_generation}", std::process::id()),
            consumer_generation: initial_generation,
        }
    }

    fn consumer_id(&self) -> &str {
        &self.consumer_id
    }

    fn consumer_generation(&self) -> u64 {
        self.consumer_generation
    }

    fn reconnected(&self) -> Result<Self> {
        Ok(Self {
            consumer_id: self.consumer_id.clone(),
            consumer_generation: self
                .consumer_generation
                .checked_add(1)
                .context("Pane Host Consumer generation exhausted")?,
        })
    }
}

#[derive(Debug)]
struct ReconnectBackoff {
    next: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next: INITIAL_RECONNECT_DELAY,
        }
    }
}

impl ReconnectBackoff {
    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = std::cmp::min(self.next.saturating_mul(2), MAX_RECONNECT_DELAY);
        delay
    }

    fn reset(&mut self) {
        self.next = INITIAL_RECONNECT_DELAY;
    }
}

#[derive(Debug, Default)]
struct StartupTerminalReplyFilter {
    active_until: Option<Instant>,
    pending: Vec<u8>,
}

impl StartupTerminalReplyFilter {
    fn arm(&mut self, now: Instant) {
        self.active_until = Some(now + STARTUP_REPLY_FILTER_WINDOW);
        self.pending.clear();
    }

    fn consume(&mut self, input: &[u8], now: Instant, unavailable: bool) -> Vec<u8> {
        // Automatic replies received during recovery are not evidence that a
        // user's keystrokes were lost. Reuse the existing bounded parser;
        // forward focus reports normally once the attachment is Ready.
        let active = unavailable || self.active_until.is_some_and(|deadline| now < deadline);
        if !active {
            self.active_until = None;
            let mut output = std::mem::take(&mut self.pending);
            output.extend_from_slice(input);
            return output;
        }

        self.pending.extend_from_slice(input);
        let mut output = Vec::with_capacity(self.pending.len());
        let mut offset = 0;
        while offset < self.pending.len() {
            if self.pending[offset] != 0x1b {
                output.push(self.pending[offset]);
                offset += 1;
                continue;
            }
            let input = &self.pending[offset..];
            let reply =
                if unavailable && (input.starts_with(b"\x1b[I") || input.starts_with(b"\x1b[O")) {
                    StartupReplyMatch::Complete(3)
                } else {
                    startup_identity_reply(input)
                };
            match reply {
                StartupReplyMatch::Complete(length) => offset += length,
                StartupReplyMatch::Prefix => break,
                StartupReplyMatch::NoMatch => {
                    output.push(self.pending[offset]);
                    offset += 1;
                }
            }
        }
        self.pending.drain(..offset);
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupReplyMatch {
    Complete(usize),
    Prefix,
    NoMatch,
}

fn startup_identity_reply(input: &[u8]) -> StartupReplyMatch {
    const SECONDARY_ATTRIBUTES_PREFIX: &[u8] = b"\x1b[>";
    const GHOSTTY_VERSION_PREFIX: &[u8] = b"\x1bP>|ghostty ";

    if SECONDARY_ATTRIBUTES_PREFIX.starts_with(input) {
        return StartupReplyMatch::Prefix;
    }
    if input.starts_with(SECONDARY_ATTRIBUTES_PREFIX) {
        let mut saw_digit = false;
        for (index, byte) in input[SECONDARY_ATTRIBUTES_PREFIX.len()..]
            .iter()
            .copied()
            .enumerate()
        {
            let length = SECONDARY_ATTRIBUTES_PREFIX.len() + index + 1;
            match byte {
                b'0'..=b'9' => saw_digit = true,
                b';' if saw_digit => {}
                b'c' if saw_digit => return StartupReplyMatch::Complete(length),
                _ => return StartupReplyMatch::NoMatch,
            }
            if length >= MAX_STARTUP_REPLY_BYTES {
                return StartupReplyMatch::NoMatch;
            }
        }
        return StartupReplyMatch::Prefix;
    }

    if GHOSTTY_VERSION_PREFIX.starts_with(input) {
        return StartupReplyMatch::Prefix;
    }
    if input.starts_with(GHOSTTY_VERSION_PREFIX) {
        let payload = &input[GHOSTTY_VERSION_PREFIX.len()..];
        let mut index = 0;
        while index < payload.len() {
            if payload[index] == 0x1b {
                if index + 1 == payload.len() {
                    return StartupReplyMatch::Prefix;
                }
                if payload[index + 1] == b'\\' {
                    return StartupReplyMatch::Complete(GHOSTTY_VERSION_PREFIX.len() + index + 2);
                }
                return StartupReplyMatch::NoMatch;
            }
            if !payload[index].is_ascii_graphic() {
                return StartupReplyMatch::NoMatch;
            }
            index += 1;
            if GHOSTTY_VERSION_PREFIX.len() + index >= MAX_STARTUP_REPLY_BYTES {
                return StartupReplyMatch::NoMatch;
            }
        }
        return StartupReplyMatch::Prefix;
    }

    StartupReplyMatch::NoMatch
}

struct ConnectedBridge {
    reader: tokio::net::unix::OwnedReadHalf,
    writer: tokio::net::unix::OwnedWriteHalf,
    status: TerminalConsumerStatusV1,
}

struct BridgeConnection {
    outbound: mpsc::Sender<TerminalBridgeFrameV1>,
    reader: tokio::task::JoinHandle<Result<()>>,
    writer: tokio::task::JoinHandle<Result<()>>,
}
impl Drop for BridgeConnection {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

fn publish_status(
    status: TerminalConsumerStatusV1,
    channel: &ContentChannel,
    views: &tokio::sync::watch::Sender<polar_tmux_protocol::TerminalRecoveryViewV1>,
) -> bool {
    if status.content_generation != channel.content_revision().max(1) {
        return false;
    }
    let ready = status.lifecycle == TerminalConsumerLifecycleV1::Ready;
    let mut view = status.recovery.unwrap_or_else(|| {
        Box::new(polar_tmux_core::terminal_feedback(
            polar_tmux_protocol::TerminalRecoveryPhaseV1::StatusUnavailable,
            None,
        ))
    });
    view.input_warning = false;
    view.input_warning_message.clear();
    if view.phase == polar_tmux_protocol::TerminalRecoveryPhaseV1::Hidden {
        view.visible = false;
    }
    views.send_replace(*view);
    ready
}

pub async fn run(
    channel_id: String,
    channel_generation: u64,
    control_socket: &Path,
    host_socket: &Path,
    initial_execution_target: Option<&str>,
    initial_session: Option<&str>,
) -> Result<()> {
    use polar_tmux_protocol::TerminalRecoveryPhaseV1 as Phase;
    use std::sync::atomic::{AtomicU64, Ordering};
    let mut channel = ContentChannel::new(channel_id, channel_generation)
        .map_err(|error| anyhow::anyhow!("invalid pane-host identity: {error:?}"))?;
    if let Some(request) = initial_request(
        channel.channel_id(),
        channel.channel_generation(),
        initial_execution_target,
        initial_session,
    )? {
        let transition = channel
            .begin(&request)
            .map_err(|e| anyhow::anyhow!("initial binding: {e:?}"))?;
        channel
            .commit(transition)
            .map_err(|e| anyhow::anyhow!("initial binding: {e:?}"))?;
    }
    let desired_revision = Arc::new(AtomicU64::new(channel.content_revision().max(1)));
    let mut latest_size = terminal_size()?;
    let mut resize_pending = false;
    let _terminal_mode = TerminalModeGuard::enter_stdin()?;
    let listener = local_ipc::bind(control_socket)?;
    let _socket_guard = SocketGuard(control_socket);
    let terminal_output = Arc::new(AsyncTerminalFd::duplicate(libc::STDOUT_FILENO)?);
    let (input_sender, mut input_receiver) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
    let stdin_task = tokio::spawn(forward_terminal_input(input_sender));
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut resize = signal(SignalKind::window_change())?;
    let mut identity = ConsumerConnectionIdentity::new(channel.channel_generation());
    let mut reconnect_backoff = ReconnectBackoff::default();
    let mut startup_reply_filter = StartupTerminalReplyFilter::default();
    let (views, _) = tokio::sync::watch::channel(polar_tmux_core::terminal_feedback(
        Phase::HostUnavailable,
        None,
    ));
    let mut watchers = tokio::task::JoinSet::new();
    let mut controls =
        tokio::task::JoinSet::<Result<Option<(UnixStream, PaneContentRequestV1)>>>::new();
    let mut connection: Option<BridgeConnection> = None;
    let mut statuses = mpsc::channel::<TerminalConsumerStatusV1>(BRIDGE_QUEUE_CAPACITY).1;
    let mut connecting: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<ConnectedBridge>> + Send>>,
    > = None;
    let mut reconnect_at = tokio::time::Instant::now();
    let mut ready = false;
    let mut foreground_focus = false;
    let mut focus_sequence = 0;
    let mut viewport_revision = 1_u64;
    let mut control_intent: Option<polar_tmux_protocol::TerminalControlIntentV1> = None;
    let (control_views, _) =
        tokio::sync::watch::channel(polar_tmux_protocol::TerminalControlStatusV1::default());
    let result = async {
        loop {
            if connection.is_none() && views.borrow().phase != Phase::HostUnavailable {
                ready = false;
                let mut view = polar_tmux_core::terminal_feedback(Phase::HostUnavailable, None);
                view.content_generation = channel.content_revision().max(1);
                views.send_replace(view);
            }
            let resize_sender = connection.as_ref().filter(|_| resize_pending && ready).map(|c| c.outbound.clone());
            tokio::select! {
                _ = tokio::time::sleep_until(reconnect_at), if connection.is_none() && connecting.is_none() => {
                    let hello = TerminalBridgeHelloV1 {
                        recovery_feedback: true, initial_content_generation: channel.content_revision().max(1),
                        protocol_version: PROTOCOL_VERSION, session_runtime_api_version: SESSION_RUNTIME_API_VERSION,
                        consumer_id: identity.consumer_id().to_owned(), consumer_generation: identity.consumer_generation(),
                        terminal_size: latest_size, terminal_type: terminal_type(), locale: terminal_locale(),
                        initial_binding: channel.binding().cloned(),
                    };
                    tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_host_bridge_identity",
                        operation_id=channel.channel_id(), consumer_id=hello.consumer_id.as_str(), generation=hello.consumer_generation);
                    identity = identity.reconnected()?;
                    let path = host_socket.to_path_buf();
                    connecting = Some(Box::pin(async move {
                        tokio::time::timeout(Duration::from_secs(8), connect_terminal_bridge(&path, hello)).await
                            .context("Host handshake timed out")?
                    }));
                }
                completed = async { match connecting.as_mut() { Some(task) => task.await, None => std::future::pending().await } } => {
                    connecting = None;
                    match completed {
                        Ok(connected) => {
                            reconnect_backoff.reset();
                            // Initial and later statuses share the same transition path.
                            // In particular, Ready must replay an early native focus with
                            // the current PTY grid, rather than leaving the Hello grid active.
                            let (sender, receiver) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
                            sender.try_send(connected.status).context("initial bridge status queue unavailable")?;
                            statuses = receiver;
                            let (outbound, mut frames) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
                            let mut writer = connected.writer;
                            let trace_channel = channel.channel_id().to_owned();
                            let reader = tokio::spawn(read_host_frames(connected.reader, sender, Arc::clone(&terminal_output), Arc::clone(&desired_revision), control_views.clone()));
                            let writer = tokio::spawn(async move {
                                let mut focus_ordinal: u64 = 0;
                                while let Some(frame) = frames.recv().await {
                                    tokio::time::timeout(Duration::from_secs(8), local_ipc::write_terminal_bridge_frame(&mut writer, &frame)).await
                                        .context("Host write timed out")??;
                                    if let TerminalBridgeFrameV1::Control(intent) = &frame {
                                        tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_control_forwarded",
                                            consumer_id=trace_channel.as_str(), sequence=intent.sequence, viewport_revision=intent.viewport_revision,
                                            operation_id=format!("{}-{}", trace_channel, intent.sequence),
                                            content_generation=intent.binding_generation, focused=intent.focused,
                                            columns=intent.size.columns, rows=intent.size.rows);
                                    }
                                    if let TerminalBridgeFrameV1::Focus { focused, binding_generation, size } = frame {
                                        focus_ordinal += 1;
                                        tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_focus_forwarded", consumer_id=trace_channel.as_str(), ordinal=focus_ordinal, content_generation=binding_generation, focused, columns=size.columns, rows=size.rows);
                                    }
                                }
                                Ok(())
                            });
                            connection = Some(BridgeConnection { outbound, reader, writer });
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Pane Host bridge unavailable");
                            reconnect_at = tokio::time::Instant::now() + reconnect_backoff.next_delay();
                        }
                    }
                }
                _ = async {
                    match connection.as_mut() {
                        Some(c) => tokio::select! { _ = &mut c.reader => {}, _ = &mut c.writer => {} },
                        None => std::future::pending().await,
                    }
                } => {
                    connection = None;
                    reconnect_at = tokio::time::Instant::now();
                }
                Some(status) = statuses.recv(), if connection.is_some() => {
                    if status.content_generation != channel.content_revision().max(1) { continue; }
                    if status.binding.as_ref() != channel.binding() {
                        connection = None;
                        reconnect_at = tokio::time::Instant::now();
                        continue;
                    }
                    let was_ready = ready;
                    ready = publish_status(status, &channel, &views);
                    if ready && !was_ready {
                        startup_reply_filter.arm(Instant::now());
                        let sampled = terminal_size()?;
                        resize_pending |= sampled != latest_size;
                        latest_size = sampled;
                        if foreground_focus {
                            latest_size = terminal_size()?;
                            resize_pending = false;
                            if let (Some(c), Some(intent)) = (&connection, control_intent.as_mut()) {
                                viewport_revision += 1;
                                intent.size = latest_size; intent.viewport_revision = viewport_revision;
                                intent.binding_generation = channel.content_revision().max(1);
                                intent.cause = polar_tmux_protocol::TerminalControlCauseV1::Recovery;
                                if c.outbound.try_send(TerminalBridgeFrameV1::Control(intent.clone())).is_err() { connection = None; }
                            }
                        }
                    }
                }
                accepted = listener.accept(), if controls.len() < 32 => {
                    let (mut stream, _) = accepted?;
                    controls.spawn(async move {
                        local_ipc::verify_peer(&stream)?;
                        let request = tokio::time::timeout(Duration::from_secs(2), local_ipc::read_json_frame::<_, PaneContentRequestV1>(&mut stream)).await??;
                        Ok(request.map(|request| (stream, request)))
                    });
                }
                Some(completed) = controls.join_next(), if !controls.is_empty() => {
                    let Ok(Ok(Some((mut stream, request)))) = completed else { continue; };
                    let is_recovery = match recovery_request(&request, &channel) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    if is_recovery && matches!(request.command, PaneContentCommandV1::ObserveRecovery) {
                        if watchers.len() < 32 { watchers.spawn(serve_recovery_subscription(stream, request, views.subscribe())); }
                        continue;
                    }
                    let retire = matches!(request.command, PaneContentCommandV1::Retire);
                    let response = if is_recovery {
                        let native_control = match &request.command {
                            PaneContentCommandV1::Focus { focused, sequence } => Some((format!("legacy-{}", channel.channel_id()), *focused, *sequence, polar_tmux_protocol::TerminalControlCauseV1::Selection)),
                            PaneContentCommandV1::TerminalControl { native_instance_id, focused, sequence, cause } => Some((native_instance_id.clone(), *focused, *sequence, *cause)),
                            _ => None,
                        };
                        if let Some((native_instance_id, focused, sequence, cause)) = native_control {
                            if sequence > focus_sequence {
                                focus_sequence = sequence;
                                foreground_focus = focused;
                                latest_size = terminal_size()?;
                                resize_pending = false;
                                viewport_revision += 1;
                                let intent = polar_tmux_protocol::TerminalControlIntentV1 {
                                    native_instance_id, focused, sequence, cause,
                                    binding_generation: channel.content_revision().max(1), viewport_revision, size: latest_size,
                                };
                                tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_focus_received", consumer_id=channel.channel_id(), sequence, viewport_revision, columns=latest_size.columns, rows=latest_size.rows, focused);
                                control_intent = Some(intent.clone());
                                if let Some(c) = &connection {
                                    if c.outbound.try_send(TerminalBridgeFrameV1::Control(intent)).is_err() { connection = None; }
                                } else if focused { reconnect_at = tokio::time::Instant::now(); }
                            }
                        } else if matches!(request.command, PaneContentCommandV1::DismissRecovery) {
                            views.send_modify(|view| { view.input_warning = false; view.visible = view.phase != Phase::Hidden; });
                            if let Some(c) = &connection { let _ = c.outbound.try_send(TerminalBridgeFrameV1::DismissRecovery); }
                        } else if matches!(request.command, PaneContentCommandV1::ProbeTerminalControl) {
                            // Read cached control facts without touching tmux.
                        } else if let Some(c) = &connection {
                            if c.outbound.try_send(TerminalBridgeFrameV1::Retry).is_err() { connection = None; }
                        } else { reconnect_at = tokio::time::Instant::now(); }
                        if matches!(request.command, PaneContentCommandV1::TerminalControl { .. } | PaneContentCommandV1::ProbeTerminalControl) {
                            let mut response = recovery_response(&request, views.borrow().clone());
                            let mut status = control_views.borrow().clone();
                            if matches!(request.command, PaneContentCommandV1::TerminalControl { .. }) && status.sequence < focus_sequence {
                                status = polar_tmux_protocol::TerminalControlStatusV1 { sequence: focus_sequence,
                                    binding_generation: channel.content_revision().max(1), viewport_revision,
                                    phase: polar_tmux_protocol::TerminalControlPhaseV1::Deferred, geometry: None };
                            }
                            response.result = PaneContentResultV1::TerminalControl { state: status };
                            response
                        } else { recovery_response(&request, views.borrow().clone()) }
                    } else {
                        let response_identity = ResponseIdentity::from(&request);
                        match channel.begin(&request) {
                            Err(error) => response_identity.rejected("content_request_rejected", &sanitized_operation_message(&anyhow::anyhow!("{error:?}")), false),
                            Ok(transition) => {
                                let retained = matches!(transition.action(), ContentChannelAction::KeepBinding { .. } | ContentChannelAction::KeepDetached);
                                let frame = bridge_frame_for_action(transition.action(), request.content_revision);
                                let response = commit_response(&mut channel, transition, response_identity);
                                desired_revision.store(channel.content_revision().max(1), Ordering::Release);
                                connecting = None;
                                if !retained {
                                    ready = false;
                                    foreground_focus = false;
                                    control_intent = None;
                                }
                                let mut view = if retained { views.borrow().clone() } else {
                                    polar_tmux_core::terminal_feedback(if channel.binding().is_some() { Phase::Probing } else { Phase::Hidden }, None)
                                };
                                view.content_generation = channel.content_revision().max(1);
                                views.send_replace(view);
                                if let (Some(c), Some(frame)) = (&connection, frame) {
                                    if c.outbound.try_send(frame).is_err() { connection = None; }
                                }
                                reconnect_at = tokio::time::Instant::now();
                                response
                            }
                        }
                    };
                    if retire && matches!(response.result, PaneContentResultV1::Detached) {
                        let _ = tokio::time::timeout(Duration::from_secs(1), local_ipc::write_json_frame(&mut stream, &response)).await;
                        break Ok(());
                    }
                    controls.spawn(async move {
                        tokio::time::timeout(Duration::from_secs(2), local_ipc::write_json_frame(&mut stream, &response)).await??;
                        Ok(None)
                    });
                }
                Some(_) = watchers.join_next(), if !watchers.is_empty() => {}
                input = input_receiver.recv() => {
                    let Some(bytes) = input else { break Ok(()); };
                    let bytes = startup_reply_filter.consume(&bytes, Instant::now(), !ready);
                    if bytes.is_empty() { continue; }
                    let discarded_bytes = bytes.len();
                    if ready {
                        if let Some(c) = &connection {
                            if c.outbound.try_send(TerminalBridgeFrameV1::Input(bytes)).is_ok() { continue; }
                            connection = None;
                        }
                    }
                    tracing::debug!(target: "polar_station_operation", component="PaneHost", event="terminal_input_discarded", discarded_bytes);
                    if let Some(c) = &connection { let _ = c.outbound.try_send(TerminalBridgeFrameV1::Retry); }
                    else { reconnect_at = tokio::time::Instant::now(); }
                }
                _ = resize.recv() => {
                    let sampled = terminal_size()?;
                    if sampled == latest_size { continue; }
                    latest_size = sampled;
                    tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_resize_received", consumer_id=channel.channel_id(), columns=latest_size.columns, rows=latest_size.rows);
                    resize_pending = connection.is_some();
                }
                permit = async move { match resize_sender { Some(sender) => sender.reserve_owned().await, None => std::future::pending().await } }, if resize_pending && connection.is_some() => {
                    resize_pending = false;
                    if let Ok(permit) = permit {
                        viewport_revision += 1;
                        if foreground_focus {
                            if let Some(intent) = control_intent.as_mut() {
                                intent.size = latest_size; intent.viewport_revision = viewport_revision;
                                intent.binding_generation = channel.content_revision().max(1);
                                intent.cause = polar_tmux_protocol::TerminalControlCauseV1::Viewport;
                                permit.send(TerminalBridgeFrameV1::Control(intent.clone()));
                            } else { permit.send(TerminalBridgeFrameV1::Resize(latest_size)); }
                        } else { permit.send(TerminalBridgeFrameV1::Resize(latest_size)); }
                        tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_resize_forwarded", consumer_id=channel.channel_id(), columns=latest_size.columns, rows=latest_size.rows);
                    } else { connection = None; }
                }
                _ = terminate.recv() => break Ok(()),
                _ = hangup.recv() => break Ok(()),
                _ = interrupt.recv() => break Ok(()),
            }
        }
    }.await;
    drop(connection);
    stdin_task.abort();
    let _ = stdin_task.await;
    result
}

async fn connect_terminal_bridge(
    host_socket: &Path,
    hello: TerminalBridgeHelloV1,
) -> Result<ConnectedBridge> {
    local_ipc::verify_server_socket(host_socket)?;
    let mut host = UnixStream::connect(host_socket)
        .await
        .with_context(|| format!("failed to connect to Host at {}", host_socket.display()))?;
    let expected_binding = hello.initial_binding.clone();
    let consumer_id = hello.consumer_id.clone();
    let consumer_generation = hello.consumer_generation;
    local_ipc::write_json_frame(
        &mut host,
        &LocalClientFrame::TerminalBridgeV2 {
            hello: polar_tmux_protocol::TerminalBridgeHelloV2 {
                base: hello,
                terminal_control_version: polar_tmux_protocol::TERMINAL_CONTROL_API_VERSION,
            },
        },
    )
    .await?;
    let status = read_next_status(&mut host).await?;
    anyhow::ensure!(
        status.binding == expected_binding,
        "Host acknowledged a different terminal binding"
    );
    anyhow::ensure!(
        status.consumer_id == consumer_id && status.consumer_generation == consumer_generation,
        "Host Consumer identity mismatch"
    );
    anyhow::ensure!(
        status.recovery.is_some(),
        "Host does not support terminal recovery status; update the matching runtime"
    );
    anyhow::ensure!(
        matches!(
            local_ipc::read_terminal_bridge_frame(&mut host).await?,
            Some(TerminalBridgeFrameV1::ControlStatus(_))
        ),
        "Host does not support terminal control; install the matching runtime"
    );
    let (reader, writer) = host.into_split();
    Ok(ConnectedBridge {
        reader,
        writer,
        status,
    })
}

fn bridge_frame_for_action(
    action: &ContentChannelAction,
    binding_generation: u64,
) -> Option<TerminalBridgeFrameV1> {
    match action {
        ContentChannelAction::Attach { binding }
        | ContentChannelAction::RestartChild { binding }
        | ContentChannelAction::ReplaceChild { to: binding, .. }
        | ContentChannelAction::SwitchClient { to: binding, .. } => {
            Some(TerminalBridgeFrameV1::Bind {
                binding: binding.clone(),
                binding_generation,
            })
        }
        ContentChannelAction::Detach { .. } => {
            Some(TerminalBridgeFrameV1::Detach { binding_generation })
        }
        ContentChannelAction::Retire { .. } => Some(TerminalBridgeFrameV1::Retire),
        ContentChannelAction::KeepBinding { .. } => {
            Some(TerminalBridgeFrameV1::RetainBinding { binding_generation })
        }
        ContentChannelAction::KeepDetached => None,
    }
}

fn commit_response(
    channel: &mut ContentChannel,
    transition: PendingContentTransition,
    identity: ResponseIdentity,
) -> PaneContentResponseV1 {
    if channel.commit(transition).is_err() {
        return identity.rejected(
            "content_state_changed",
            "Pane content state changed before the operation completed",
            true,
        );
    }
    match channel.binding() {
        Some(binding) => identity.ready(binding.clone()),
        None => identity.detached(),
    }
}

fn recovery_request(request: &PaneContentRequestV1, channel: &ContentChannel) -> Result<bool> {
    if !matches!(
        request.command,
        PaneContentCommandV1::ObserveRecovery
            | PaneContentCommandV1::Focus { .. }
            | PaneContentCommandV1::TerminalControl { .. }
            | PaneContentCommandV1::ProbeTerminalControl
            | PaneContentCommandV1::ProbeRecovery
            | PaneContentCommandV1::DismissRecovery
    ) {
        return Ok(false);
    }
    request.validate()?;
    if matches!(
        request.command,
        PaneContentCommandV1::TerminalControl { .. }
    ) {
        anyhow::ensure!(
            request.content_revision == channel.content_revision().max(1),
            "stale terminal control binding"
        );
    }
    anyhow::ensure!(
        request.channel_id == channel.channel_id()
            && request.channel_generation == channel.channel_generation(),
        "stale recovery channel"
    );
    Ok(true)
}

fn recovery_response(
    request: &PaneContentRequestV1,
    view: polar_tmux_protocol::TerminalRecoveryViewV1,
) -> PaneContentResponseV1 {
    PaneContentResponseV1 {
        protocol_version: PROTOCOL_VERSION,
        api: request.api,
        request_id: request.request_id.clone(),
        channel_id: request.channel_id.clone(),
        channel_generation: request.channel_generation,
        content_revision: if matches!(request.command, PaneContentCommandV1::ObserveRecovery) {
            view.content_generation
        } else {
            request.content_revision
        },
        result: PaneContentResultV1::Recovery {
            view: Box::new(view),
        },
    }
}

async fn serve_recovery_subscription(
    mut stream: tokio::net::UnixStream,
    request: PaneContentRequestV1,
    mut views: tokio::sync::watch::Receiver<polar_tmux_protocol::TerminalRecoveryViewV1>,
) {
    use tokio::io::AsyncReadExt;
    loop {
        let response = recovery_response(&request, views.borrow_and_update().clone());
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                local_ipc::write_json_frame(&mut stream, &response)
            )
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
        tokio::select! {
            changed = views.changed() => if changed.is_err() { break; },
            _ = stream.read_u8() => break,
        }
    }
}

async fn read_host_frames(
    mut reader: tokio::net::unix::OwnedReadHalf,
    statuses: mpsc::Sender<TerminalConsumerStatusV1>,
    stdout: Arc<AsyncTerminalFd>,
    desired_revision: Arc<std::sync::atomic::AtomicU64>,
    control: tokio::sync::watch::Sender<polar_tmux_protocol::TerminalControlStatusV1>,
) -> Result<()> {
    let mut observed_revision = 0;
    let mut first_output = true;
    let mut consumer_id = String::new();
    let mut generation = 0;
    while let Some(frame) = local_ipc::read_terminal_bridge_frame(&mut reader).await? {
        match frame {
            TerminalBridgeFrameV1::ControlStatus(status) => {
                if status.binding_generation
                    == desired_revision.load(std::sync::atomic::Ordering::Acquire)
                    && status.sequence >= control.borrow().sequence
                {
                    control.send_replace(status);
                }
            }
            TerminalBridgeFrameV1::Output(bytes) => {
                if observed_revision == desired_revision.load(std::sync::atomic::Ordering::Acquire)
                {
                    let sequence = control.borrow().sequence;
                    let span = tracing::info_span!(target: "polar_station_operation", "terminal_native_output_write", component="PaneHost",
                        consumer_id=consumer_id.as_str(), sequence, operation_id=format!("{consumer_id}-{sequence}"), bytes=bytes.len(), outcome="cancelled");
                    let result = stdout.write_all(&bytes).instrument(span.clone()).await;
                    span.record("outcome", if result.is_ok() { "ok" } else { "failed" });
                    result?;
                    if first_output {
                        tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_terminal_output_written",
                            consumer_id=consumer_id.as_str(), generation, content_generation=observed_revision, bytes=bytes.len() as u64);
                        first_output = false;
                    }
                }
            }
            TerminalBridgeFrameV1::Status(status) => {
                if observed_revision != status.content_generation {
                    first_output = true;
                }
                observed_revision = status.content_generation;
                consumer_id = status.consumer_id.clone();
                generation = status.consumer_generation;
                if statuses.send(status).await.is_err() {
                    return Ok(());
                }
            }
            _ => bail!("Polar Tmux Host sent a Consumer-only terminal frame"),
        }
    }
    Ok(())
}

async fn forward_terminal_input(outbound: mpsc::Sender<Vec<u8>>) -> Result<()> {
    forward_terminal_input_from_fd(libc::STDIN_FILENO, outbound).await
}

async fn forward_terminal_input_from_fd(fd: RawFd, outbound: mpsc::Sender<Vec<u8>>) -> Result<()> {
    let stdin = AsyncTerminalFd::duplicate(fd)?;
    let mut buffer = vec![0_u8; TERMINAL_CHUNK_BYTES];
    loop {
        let read = stdin.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        outbound
            .send(buffer[..read].to_vec())
            .await
            .context("terminal bridge writer closed")?;
    }
}

async fn read_next_status(stream: &mut UnixStream) -> Result<TerminalConsumerStatusV1> {
    match local_ipc::read_terminal_bridge_frame(stream).await? {
        Some(TerminalBridgeFrameV1::Status(status)) => Ok(status),
        Some(TerminalBridgeFrameV1::Output(_)) => {
            bail!("Host emitted terminal output before bridge readiness")
        }
        Some(_) => bail!("Host sent a Consumer-only terminal frame"),
        None => bail!("Host closed before terminal bridge readiness"),
    }
}

fn initial_request(
    channel_id: &str,
    channel_generation: u64,
    execution_target: Option<&str>,
    tmux_session_id: Option<&str>,
) -> Result<Option<PaneContentRequestV1>> {
    let (Some(execution_target), Some(tmux_session_id)) = (execution_target, tmux_session_id)
    else {
        if execution_target.is_some() || tmux_session_id.is_some() {
            bail!("initial Pane content target and Session must be supplied together")
        }
        return Ok(None);
    };
    let request = PaneContentRequestV1 {
        protocol_version: PROTOCOL_VERSION,
        api: PaneContentApiV1::PaneContentV1,
        request_id: "pane-host-initial-binding".to_owned(),
        channel_id: channel_id.to_owned(),
        channel_generation,
        content_revision: 1,
        command: PaneContentCommandV1::Bind {
            binding: PaneContentBindingV1 {
                execution_target: ExecutionTarget::new(execution_target)?,
                tmux_session_id: tmux_session_id.to_owned(),
            },
        },
    };
    request
        .validate()
        .context("invalid initial Pane content binding")?;
    Ok(Some(request))
}

fn terminal_size() -> Result<TerminalSizeV1> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `size` points to writable storage for TIOCGWINSZ and stdin is
    // owned by this terminal bridge process.
    let result = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) };
    if result != 0 || size.ws_col == 0 || size.ws_row == 0 {
        bail!("pane-host standard input has no valid terminal dimensions");
    }
    Ok(TerminalSizeV1 {
        columns: size.ws_col,
        rows: size.ws_row,
        pixel_width: size.ws_xpixel,
        pixel_height: size.ws_ypixel,
    })
}

fn terminal_type() -> String {
    bounded_environment_value("TERM").unwrap_or_else(|| "xterm-256color".to_owned())
}

fn terminal_locale() -> Option<String> {
    bounded_environment_value("LC_ALL").or_else(|| bounded_environment_value("LANG"))
}

fn bounded_environment_value(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    (!value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn sanitized_operation_message(error: &anyhow::Error) -> String {
    let raw = error.to_string().replace(['\n', '\r', '\t'], " ");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "Pane content operation failed".to_owned()
    } else {
        trimmed.chars().take(512).collect()
    }
}

struct ResponseIdentity {
    request_id: String,
    channel_id: String,
    channel_generation: u64,
    content_revision: u64,
}

impl From<&PaneContentRequestV1> for ResponseIdentity {
    fn from(request: &PaneContentRequestV1) -> Self {
        Self {
            request_id: request.request_id.clone(),
            channel_id: request.channel_id.clone(),
            channel_generation: request.channel_generation,
            content_revision: request.content_revision,
        }
    }
}

impl ResponseIdentity {
    fn response(self, result: PaneContentResultV1) -> PaneContentResponseV1 {
        PaneContentResponseV1 {
            protocol_version: PROTOCOL_VERSION,
            api: PaneContentApiV1::PaneContentV1,
            request_id: self.request_id,
            channel_id: self.channel_id,
            channel_generation: self.channel_generation,
            content_revision: self.content_revision,
            result,
        }
    }

    fn ready(self, binding: PaneContentBindingV1) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Accepted { binding })
    }

    fn detached(self) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Detached)
    }

    fn rejected(self, code: &str, message: &str, retryable: bool) -> PaneContentResponseV1 {
        self.response(PaneContentResultV1::Rejected {
            error: ProtocolError::new(code, message, retryable),
        })
    }
}

struct SocketGuard<'a>(&'a Path);

impl Drop for SocketGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}


