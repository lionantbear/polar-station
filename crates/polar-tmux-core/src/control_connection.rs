//! A native Control Mode command channel. Session policy stays in the Host.
use std::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Duration};
use tracing::Instrument;

/// Native epoch guard checked by the existing writer immediately before I/O.
#[derive(Debug, Clone)]
pub struct TmuxControlEpoch {
    pub current: Arc<AtomicU64>,
    pub expected: u64,
}
impl TmuxControlEpoch {
    pub fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire) == self.expected
    }
}

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::{broadcast, mpsc, oneshot, watch},
};

use crate::{
    control_mode::decode_line, target::quote_tmux_argument, TmuxControlEvent, TmuxError, TmuxResult,
};

pub(crate) const BOOTSTRAP_MARKER: &str = "POLAR_CONTROL_BOOTSTRAP_READY";

const COMMAND_CAPACITY: usize = 64;
const MAX_REPLY_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;

struct Request {
    epoch: Option<TmuxControlEpoch>,
    trace: tracing::Span,
    command: String,
    reply: oneshot::Sender<TmuxResult<ControlReply>>,
}

#[derive(Debug)]
pub struct ControlReply {
    pub output: String,
    pub failed: bool,
}

#[derive(Debug)]
struct Connection {
    commands: mpsc::Sender<Request>,
    events: broadcast::Sender<TmuxControlEvent>,
    stop: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
    ready: watch::Receiver<bool>,
    bootstrap_start: Arc<tokio::sync::Notify>,
}

/// Clones share one child and one bounded command queue. A command is sent once;
/// caller cancellation never replays a mutation or assigns its reply elsewhere.
#[derive(Clone, Debug)]
pub struct TmuxControlConnection(Arc<Connection>);

impl TmuxControlConnection {
    /// The caller configures the child lease, piped streams and kill-on-drop.
    pub fn from_child(child: Child) -> TmuxResult<Self> {
        Self::start(child, false)
    }

    /// Pair only with TmuxClient::target_control_mode_plan.
    pub fn from_bootstrapped_child(child: Child) -> TmuxResult<Self> {
        Self::start(child, true)
    }

    fn start(mut child: Child, bootstrap_target: bool) -> TmuxResult<Self> {
        tracing::info!(target: "polar_station_operation", component="Tmux", event="control_connection_opened");
        let input = child.stdin.take().ok_or_else(closed)?;
        let output = child.stdout.take().ok_or_else(closed)?;
        let (commands, requests) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(256);
        let (stop, stopped) = watch::channel(false);
        let published = events.clone();
        let (finished_tx, finished) = watch::channel(false);
        let (ready_tx, ready) = watch::channel(false);
        let bootstrap_start = Arc::new(tokio::sync::Notify::new());
        let start = bootstrap_start.clone();
        tokio::spawn(async move {
            let mut stopped = stopped;
            // Install all session observers before draining initial subscriptions.
            if bootstrap_target {
                tokio::select! {
                    _ = start.notified() => {},
                    _ = stopped.changed() => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let _ = finished_tx.send(true);
                        return;
                    },
                }
            }
            let _ = run(
                input,
                output,
                requests,
                &published,
                stopped,
                bootstrap_target,
                ready_tx,
            )
            .await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = published.send(TmuxControlEvent::Exit);
            let _ = finished_tx.send(true);
        });
        Ok(Self(Arc::new(Connection {
            commands,
            events,
            stop,
            finished,
            ready,
            bootstrap_start,
        })))
    }

    pub async fn wait_ready(&self) -> TmuxResult<()> {
        self.0.bootstrap_start.notify_one();
        let mut ready = self.0.ready.clone();
        loop {
            if *ready.borrow_and_update() {
                return Ok(());
            }
            ready.changed().await.map_err(|_| closed())?;
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TmuxControlEvent> {
        self.0.events.subscribe()
    }

    pub fn close(&self) {
        let _ = self.0.stop.send(true);
    }

    pub async fn shutdown(&self) {
        self.close();
        let mut finished = self.0.finished.clone();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.0.commands.is_closed() || *self.0.stop.borrow()
    }

    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub async fn command(&self, args: &[String], timeout: Duration) -> TmuxResult<ControlReply> {
        self.command_batch(&[args.to_vec()], timeout).await
    }

    pub async fn command_batch(
        &self,
        commands: &[Vec<String>],
        timeout: Duration,
    ) -> TmuxResult<ControlReply> {
        self.command_batch_guarded(commands, timeout, None).await
    }

    pub async fn command_batch_guarded(
        &self,
        commands: &[Vec<String>],
        timeout: Duration,
        epoch: Option<TmuxControlEpoch>,
    ) -> TmuxResult<ControlReply> {
        let command = commands
            .iter()
            .map(|args| {
                // -N is CLI connection policy, not an in-server command.
                let args = if args.first().is_some_and(|arg| arg == "-N") {
                    &args[1..]
                } else {
                    &args[..]
                };
                args.iter()
                    .map(|arg| quote_tmux_argument(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" ; ");
        self.command_text_guarded(command, timeout, epoch).await
    }

    /// Already encoded native tmux syntax, used for the existing subscriptions.
    pub async fn command_text(
        &self,
        command: String,
        timeout: Duration,
    ) -> TmuxResult<ControlReply> {
        self.command_text_guarded(command, timeout, None).await
    }

    async fn command_text_guarded(
        &self,
        command: String,
        timeout: Duration,
        epoch: Option<TmuxControlEpoch>,
    ) -> TmuxResult<ControlReply> {
        if command.len() > MAX_REPLY_BYTES || command.contains(['\n', '\r']) {
            return Err(TmuxError::invalid_input(
                "invalid Control Mode command size or delimiter",
            ));
        }
        let (reply, received) = oneshot::channel();
        let span = tracing::info_span!(target: "polar_station_operation", "control_command",
            component="Tmux", queue_depth=self.0.commands.max_capacity()-self.0.commands.capacity(), outcome="cancelled");
        let result = tokio::time::timeout(
            timeout,
            async {
                self.0
                    .commands
                    .send(Request { command, reply, trace: span.clone(), epoch })
                    .await
                    .map_err(|_| closed())?;
                tracing::info!(target: "polar_station_operation", component="Tmux", event="control_command_queued");
                received.await.map_err(|_| closed())?
            }
            .instrument(span.clone()),
        )
        .await
        .map_err(|_| TmuxError::timeout(timeout.as_millis()))
        .and_then(|reply| reply);
        span.record(
            "outcome",
            if matches!(&result, Ok(reply) if !reply.failed) {
                "ok"
            } else {
                "failed"
            },
        );
        result
    }
}

fn closed() -> TmuxError {
    TmuxError::target_unavailable("the target Control Mode channel is unavailable")
}

struct Pending {
    request: Request,
    marker: String,
    output: String,
    failed: bool,
    marker_seen: bool,
}

async fn run(
    mut input: tokio::process::ChildStdin,
    output: tokio::process::ChildStdout,
    mut requests: mpsc::Receiver<Request>,
    events: &broadcast::Sender<TmuxControlEvent>,
    mut stopped: watch::Receiver<bool>,
    bootstrap_target: bool,
    ready_tx: watch::Sender<bool>,
) -> TmuxResult<()> {
    let mut reader = BufReader::new(output);
    let mut buffer = Vec::new();
    let mut pending: Option<Pending> = None;
    let mut sequence = 0_u64;
    let mut ready = false;
    let mut bootstrap_seen = false;
    if bootstrap_target {
        tracing::info!(target: "polar_station_operation", component="Tmux", event="control_bootstrap_started");
    }
    let mut inside_block = false;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        tokio::select! {
            _ = stopped.changed() => return Ok(()),
            _ = tokio::time::sleep_until(deadline), if !ready || pending.is_some() => {
                return Err(TmuxError::timeout(10_000));
            }
            request = requests.recv(), if ready && pending.is_none() => {
                let Some(request) = request else { return Ok(()); };
                if request.reply.is_closed() { continue; }
                if request.epoch.as_ref().is_some_and(|epoch| !epoch.is_current()) {
                    tracing::info!(target: "polar_station_operation", parent: &request.trace, component="Tmux", event="control_command_superseded");
                    let _ = request.reply.send(Err(TmuxError::target_unavailable("terminal selection superseded")));
                    continue;
                }
                tracing::info!(target: "polar_station_operation", parent: &request.trace, component="Tmux", event="control_command_execute");
                sequence += 1;
                let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                let marker = format!("POLAR_CONTROL_DONE_{}_{nonce}_{sequence}", std::process::id());
                // Separate input queues: a command error may abort its command
                // list, but must not discard the completion marker's list.
                let wire = format!("{}\ndisplay-message -p '{}'\n", request.command, marker);
                pending = Some(Pending { request, marker, output: String::new(), failed:false, marker_seen:false });
                deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                tokio::select! {
                    _ = stopped.changed() => return Ok(()),
                    result = tokio::time::timeout_at(deadline, async {
                        input.write_all(wire.as_bytes()).await?;
                        input.flush().await
                    }) => { result.map_err(|_| TmuxError::timeout(10_000))?.map_err(|_| closed())?; }
                }
                if let Some(active) = &pending {
                    tracing::info!(target: "polar_station_operation", parent: &active.request.trace, component="Tmux", event="control_command_written");
                }
            }
            result = read_line_bounded(&mut reader, &mut buffer) => {
                if result.map_err(|_| closed())? == 0 { return Err(closed()); }
                if buffer.len() > MAX_LINE_BYTES { return Err(TmuxError::parse_failed("Control Mode line too long")); }
                let line = std::str::from_utf8(&buffer).map_err(|_| TmuxError::parse_failed("Control Mode invalid UTF-8"))?
                    .trim_end_matches(['\r', '\n']).strip_prefix("\x1bP1000p")
                    .unwrap_or_else(|| std::str::from_utf8(&buffer).unwrap().trim_end_matches(['\r', '\n']));
                if line.starts_with("%begin ") {
                    inside_block = true;
                } else if line.starts_with("%end ") || line.starts_with("%error ") {
                    let failed = line.starts_with("%error ");
                    inside_block = false;
                    if !ready {
                        if failed {
                            tracing::info!(target: "polar_station_operation", component="Tmux", event="control_bootstrap_failed");
                            return Err(closed());
                        }
                        if !bootstrap_target || bootstrap_seen {
                            ready = true;
                            ready_tx.send_replace(true);
                            tracing::info!(target: "polar_station_operation", component="Tmux", event="control_bootstrap_completed");
                        }
                    }
                    if let Some(active) = &mut pending {
                        active.failed |= failed;
                        if active.marker_seen {
                            let active = pending.take().unwrap();
                            tracing::info!(target: "polar_station_operation", parent: &active.request.trace, component="Tmux", event="control_command_replied");
                            let _ = active.request.reply.send(Ok(ControlReply { output:active.output, failed:active.failed }));
                        }
                    }
                } else if inside_block {
                    if !ready && bootstrap_target && line == BOOTSTRAP_MARKER {
                        bootstrap_seen = true;
                    }
                    if let Some(active) = &mut pending {
                        if line == active.marker { active.marker_seen = true; }
                        else {
                            if active.output.len() + line.len() + 1 > MAX_REPLY_BYTES {
                                return Err(TmuxError::parse_failed("Control Mode reply too long"));
                            }
                            active.output.push_str(line);
                            active.output.push('\n');
                        }
                    }
                } else if let Some(event) = decode_line(line).map_err(|_| TmuxError::parse_failed("invalid Control Mode event"))? {
                    let ended = matches!(event, TmuxControlEvent::Exit);
                    for event in crate::expand_target_subscription(event).map_err(|_| TmuxError::parse_failed("invalid target subscription"))? {
                        let _ = events.send(event);
                    }
                    if ended { return Ok(()); }
                }
                buffer.clear();
            }
        }
    }
}

async fn read_line_bounded(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    buffer: &mut Vec<u8>,
) -> std::io::Result<usize> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(0);
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if buffer.len() + count > MAX_LINE_BYTES {
            return Err(std::io::Error::other("Control Mode line too long"));
        }
        let complete = available[count - 1] == b'\n';
        buffer.extend_from_slice(&available[..count]);
        reader.consume(count);
        if complete {
            return Ok(buffer.len());
        }
    }
}


