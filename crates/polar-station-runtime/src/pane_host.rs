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

use anyhow::{bail, Context, Result};
use polar_tmux_core::{ContentChannel, ContentChannelAction};
use polar_station_client::terminal_content::{
    bridge_frame_for_action, commit_response, recovery_request, recovery_response,
    sanitized_operation_message, ResponseIdentity,
};
use polar_station_protocol::{
    ExecutionTarget, PaneContentApiV1, PaneContentBindingV1,
    PaneContentCommandV1, PaneContentRequestV1, PaneContentResultV1,
    TerminalBridgeFrameV1, TerminalBridgeHelloV1, TerminalConsumerLifecycleV1,
    TerminalConsumerStatusV1, TerminalSizeV1, PROTOCOL_VERSION, SESSION_RUNTIME_API_VERSION,
};
use tokio::{
    net::UnixStream,
    signal::unix::{signal, SignalKind},
    sync::mpsc,
};

use crate::local_ipc;
use polar_station_client::terminal_bridge::{
    connect_terminal_bridge, BridgeConnection, ConnectedBridge, ReconnectBackoff,
    StartupTerminalReplyFilter, BRIDGE_QUEUE_CAPACITY,
};


const TERMINAL_CHUNK_BYTES: usize = 16 * 1024;

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

fn publish_status(
    status: TerminalConsumerStatusV1,
    channel: &ContentChannel,
    views: &tokio::sync::watch::Sender<polar_station_protocol::TerminalRecoveryViewV1>,
) -> bool {
    if status.content_generation != channel.content_revision().max(1) {
        return false;
    }
    let ready = status.lifecycle == TerminalConsumerLifecycleV1::Ready;
    let mut view = status.recovery.unwrap_or_else(|| {
        Box::new(polar_tmux_core::terminal_feedback(
            polar_station_protocol::TerminalRecoveryPhaseV1::StatusUnavailable,
            None,
        ))
    });
    view.input_warning = false;
    view.input_warning_message.clear();
    if view.phase == polar_station_protocol::TerminalRecoveryPhaseV1::Hidden {
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
    initial_source: Option<polar_station_protocol::TerminalSourceV1>,
) -> Result<()> {
    use polar_station_protocol::TerminalRecoveryPhaseV1 as Phase;
    use std::sync::atomic::{AtomicU64, Ordering};
    let mut channel = ContentChannel::new(channel_id, channel_generation)
        .map_err(|error| anyhow::anyhow!("invalid pane-host identity: {error:?}"))?;
    if let Some(mut request) = initial_request(
        channel.channel_id(),
        channel.channel_generation(),
        initial_execution_target,
        initial_session,
    )? {
        if let PaneContentCommandV1::Bind { binding } = &mut request.command { binding.source = initial_source; }
        request.validate()?;
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
    let mut control_intent: Option<polar_station_protocol::TerminalControlIntentV1> = None;
    let (control_views, _) =
        tokio::sync::watch::channel(polar_station_protocol::TerminalControlStatusV1::default());
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
                    let hello = TerminalBridgeHelloV1 { portable_binding_version: channel.binding().and_then(|binding| binding.source.as_ref().map(|_| 1)),
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
                            let output = Arc::clone(&terminal_output);
                            let (bridge, receiver) = connected.start(
                                Arc::clone(&desired_revision),
                                control_views.clone(),
                                channel.channel_id().to_owned(),
                                move |bytes| {
                                    let output = Arc::clone(&output);
                                    async move { output.write_all(&bytes).await.context("terminal output closed") }
                                },
                            )?;
                            statuses = receiver;
                            connection = Some(bridge);
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Pane Host bridge unavailable");
                            reconnect_at = tokio::time::Instant::now() + reconnect_backoff.next_delay();
                        }
                    }
                }
                _ = async {
                    match connection.as_mut() {
                        Some(c) => c.disconnected().await,
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
                                intent.cause = polar_station_protocol::TerminalControlCauseV1::Recovery;
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
                            PaneContentCommandV1::Focus { focused, sequence } => Some((format!("legacy-{}", channel.channel_id()), *focused, *sequence, polar_station_protocol::TerminalControlCauseV1::Selection)),
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
                                let intent = polar_station_protocol::TerminalControlIntentV1 {
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
                                status = polar_station_protocol::TerminalControlStatusV1 { sequence: focus_sequence,
                                    binding_generation: channel.content_revision().max(1), viewport_revision,
                                    phase: polar_station_protocol::TerminalControlPhaseV1::Deferred, geometry: None };
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
                                // Imported bindings must negotiate the source guard, including
                                // when an existing unguarded surface is rebound.
                                if !retained && channel.binding().is_some_and(|binding| binding.source.is_some()) {
                                    connection = None;
                                }
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
                                intent.cause = polar_station_protocol::TerminalControlCauseV1::Viewport;
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

async fn serve_recovery_subscription(
    mut stream: tokio::net::UnixStream,
    request: PaneContentRequestV1,
    mut views: tokio::sync::watch::Receiver<polar_station_protocol::TerminalRecoveryViewV1>,
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
            binding: PaneContentBindingV1 { source: None,
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

struct SocketGuard<'a>(&'a Path);

impl Drop for SocketGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}


