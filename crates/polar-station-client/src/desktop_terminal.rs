//! One desktop terminal connection, driven by the existing DesktopClient runtime.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use polar_station_protocol::{
    PaneContentBindingV1, PaneContentCommandV1, PaneContentRequestV1, PaneContentResponseV1,
    PaneContentResultV1, TerminalBridgeFrameV1, TerminalBridgeHelloV1,
    TerminalConsumerLifecycleV1, TerminalConsumerStatusV1, TerminalControlCauseV1,
    TerminalControlIntentV1, TerminalControlStatusV1, TerminalSizeV1, PROTOCOL_VERSION,
    TerminalRecoveryPhaseV1, TerminalRecoveryViewV1, SESSION_RUNTIME_API_VERSION,
};
use polar_tmux_types::{terminal_feedback, ContentChannel};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::terminal_bridge::{
    connect_terminal_bridge, BridgeConnection, ConnectedBridge, ReconnectBackoff,
    StartupTerminalReplyFilter, BRIDGE_QUEUE_CAPACITY,
};
use crate::terminal_content::{bridge_frame_for_action, commit_response, recovery_request,
    recovery_response, ResponseIdentity};

const MAX_INPUT_BYTES: usize = 16 * 1024;
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(8);
const RETIRE_TIMEOUT: Duration = Duration::from_secs(1);
const BINDING_GENERATION: u64 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopTerminalConfig {
    pub binding: PaneContentBindingV1,
    pub size: TerminalSizeV1,
    #[serde(default = "default_terminal_type")]
    pub terminal_type: String,
    pub locale: Option<String>,
    #[serde(default)]
    pub channel: Option<DesktopTerminalChannel>,
}

fn default_terminal_type() -> String {
    "xterm-256color".into()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopTerminalChannel {
    pub id: String,
    pub generation: u64,
}

pub(super) struct ContentCommand {
    request: PaneContentRequestV1,
    response: oneshot::Sender<PaneContentResponseV1>,
}

#[derive(Clone)]
pub(super) struct ContentPort {
    sender: mpsc::Sender<ContentCommand>,
    pub(super) recovery: watch::Receiver<TerminalRecoveryViewV1>,
}

impl ContentPort {
    pub(super) async fn exchange(&self, request: PaneContentRequestV1) -> Result<PaneContentResponseV1> {
        let (response, receipt) = oneshot::channel();
        self.sender.send(ContentCommand { request, response }).await
            .map_err(|_| anyhow::anyhow!("direct terminal is closed"))?;
        receipt.await.context("direct terminal closed before control receipt")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Viewport {
    size: TerminalSizeV1,
    focused: bool,
}

#[derive(Default)]
struct ControlRevision {
    focused: bool,
    selection: u64,
    viewport: u64,
}

struct NativeSelection {
    instance: String,
    sequence: u64,
    focused: bool,
    cause: TerminalControlCauseV1,
}

struct Input {
    connection_generation: u64,
    bytes: Vec<u8>,
}

struct Output {
    binding_epoch: u64,
    bytes: Vec<u8>,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct DesktopTerminal {
    input: mpsc::Sender<Input>,
    viewport: watch::Sender<Viewport>,
    output: mpsc::Receiver<Output>,
    output_epoch: Arc<AtomicU64>,
    status: watch::Receiver<Option<TerminalConsumerStatusV1>>,
    control: watch::Receiver<TerminalControlStatusV1>,
    generation: Arc<AtomicU64>,
    close: Option<oneshot::Sender<()>>,
}

impl DesktopTerminal {
    pub fn is_ready(&self) -> bool {
        self.generation.load(Ordering::Acquire) != 0
    }

    pub fn status(&self) -> Option<TerminalConsumerStatusV1> {
        self.status.borrow().clone()
    }

    pub fn control_status(&self) -> TerminalControlStatusV1 {
        self.control.borrow().clone()
    }

    pub fn read_output(&mut self) -> Option<Vec<u8>> {
        while let Ok(frame) = self.output.try_recv() {
            if frame.binding_epoch == self.output_epoch() {
                return Some(frame.bytes);
            }
        }
        None
    }

    pub fn output_epoch(&self) -> u64 {
        self.output_epoch.load(Ordering::Acquire)
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= MAX_INPUT_BYTES,
            "desktop terminal input exceeds frame limit"
        );
        let connection_generation = self.generation.load(Ordering::Acquire);
        ensure!(connection_generation != 0, "desktop terminal is not ready");
        self.input
            .try_send(Input {
                connection_generation,
                bytes: bytes.to_vec(),
            })
            .map_err(|_| anyhow::anyhow!("desktop terminal input queue is unavailable"))
    }

    pub fn set_viewport(&self, size: TerminalSizeV1, focused: bool) -> Result<()> {
        size.validate()?;
        ensure!(!self.viewport.is_closed(), "desktop terminal is closed");
        self.viewport.send_if_modified(|current| {
            let next = Viewport { size, focused };
            if *current == next {
                return false;
            }
            *current = next;
            true
        });
        Ok(())
    }
}

impl Drop for DesktopTerminal {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
    }
}

pub(super) struct TerminalTask {
    hello: TerminalBridgeHelloV1,
    input: mpsc::Receiver<Input>,
    viewport: watch::Receiver<Viewport>,
    output: mpsc::Sender<Output>,
    output_epoch: Arc<AtomicU64>,
    status: watch::Sender<Option<TerminalConsumerStatusV1>>,
    control: watch::Sender<TerminalControlStatusV1>,
    generation: Arc<AtomicU64>,
    binding_generation: Arc<AtomicU64>,
    close: oneshot::Receiver<()>,
    wake: Wake,
    channel: ContentChannel,
    content: mpsc::Receiver<ContentCommand>,
    recovery: watch::Sender<TerminalRecoveryViewV1>,
    input_epoch: u64,
    native_selection: Option<NativeSelection>,
}

pub(super) struct TerminalLaunch {
    pub(super) terminal: DesktopTerminal,
    pub(super) task: TerminalTask,
    pub(super) port: Option<(DesktopTerminalChannel, ContentPort)>,
}

impl TerminalTask {
    pub(super) fn prepare(
        config: DesktopTerminalConfig,
        wake: Wake,
    ) -> Result<TerminalLaunch> {
        let hello = TerminalBridgeHelloV1 {
            portable_binding_version: config.binding.source.as_ref().map(|_| 1),
            protocol_version: PROTOCOL_VERSION,
            session_runtime_api_version: SESSION_RUNTIME_API_VERSION,
            recovery_feedback: true,
            initial_content_generation: BINDING_GENERATION,
            consumer_id: polar_identity::generation::new_uuid_v4().to_string(),
            consumer_generation: 1,
            terminal_size: config.size,
            terminal_type: config.terminal_type,
            locale: config.locale,
            initial_binding: Some(config.binding),
        };
        hello
            .validate()
            .context("invalid desktop terminal configuration")?;
        let channel = ContentChannel::bound(
            config.channel.as_ref().map_or(hello.consumer_id.as_str(), |channel| channel.id.as_str()),
            config.channel.as_ref().map_or(1, |channel| channel.generation),
            BINDING_GENERATION,
            hello.initial_binding.clone().context("missing initial terminal binding")?,
        ).map_err(|error| anyhow::anyhow!("invalid direct terminal channel: {error:?}"))?;
        let (content_sender, content) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        let mut initial_view = terminal_feedback(TerminalRecoveryPhaseV1::Probing, None);
        initial_view.content_generation = BINDING_GENERATION;
        let (recovery, recovery_receiver) = watch::channel(initial_view);
        let port = config.channel.map(|channel| (channel, ContentPort {
            sender: content_sender,
            recovery: recovery_receiver,
        }));
        let (input, input_rx) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        let (output, output_rx) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        let (viewport, viewport_rx) = watch::channel(Viewport {
            size: config.size,
            focused: false,
        });
        let (status, status_rx) = watch::channel(None);
        let (control, control_rx) = watch::channel(Default::default());
        let (close, close_rx) = oneshot::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let output_epoch = Arc::new(AtomicU64::new(BINDING_GENERATION));
        Ok(TerminalLaunch {
            terminal: DesktopTerminal {
                input,
                viewport,
                output: output_rx,
                output_epoch: output_epoch.clone(),
                status: status_rx,
                control: control_rx,
                generation: generation.clone(),
                close: Some(close),
            },
            task: Self {
                hello,
                input: input_rx,
                viewport: viewport_rx,
                output,
                output_epoch,
                status,
                control,
                generation,
                binding_generation: Arc::new(AtomicU64::new(BINDING_GENERATION)),
                close: close_rx,
                wake,
                channel,
                content,
                recovery,
                input_epoch: 0,
                native_selection: None,
            },
            port,
        })
    }

    pub(super) async fn run(mut self, host_socket: PathBuf) {
        let mut backoff = ReconnectBackoff::default();
        loop {
            if self.hello.initial_binding.is_none() {
                tokio::select! {
                    _ = &mut self.close => return,
                    Some(command) = self.content.recv() => {
                        if self.apply_content(command, None) == ContentEffect::Retire { return; }
                    }
                }
                continue;
            }
            self.hello.terminal_size = self.viewport.borrow_and_update().size;
            let attempt = tokio::time::timeout(
                BRIDGE_TIMEOUT,
                connect_terminal_bridge(&host_socket, self.hello.clone()),
            );
            let result = tokio::select! {
                biased;
                _ = &mut self.close => return,
                Some(command) = self.content.recv() => {
                    if self.apply_content(command, None) == ContentEffect::Retire { return; }
                    if !self.advance_connection() { return; }
                    continue;
                }
                result = attempt => result,
            };
            match result {
                Ok(Ok(connected)) => {
                    backoff.reset();
                    match self.run_connection(connected).await {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(error) => tracing::debug!(%error, "Desktop terminal connection ended"),
                    }
                }
                Ok(Err(error)) => tracing::debug!(%error, "Desktop terminal Host unavailable"),
                Err(error) => tracing::debug!(%error, "Desktop terminal handshake timed out"),
            }
            self.set_unavailable();
            if !self.advance_connection() { return; }
            tokio::select! {
                biased;
                _ = &mut self.close => return,
                Some(command) = self.content.recv() => {
                    if self.apply_content(command, None) == ContentEffect::Retire { return; }
                }
                _ = tokio::time::sleep(backoff.next_delay()) => {}
            }
        }
    }

    async fn run_connection(&mut self, connected: ConnectedBridge) -> Result<bool> {
        let output = self.output.clone();
        let wake = self.wake.clone();
        let binding_epoch = self.output_epoch.load(Ordering::Acquire);
        let (mut bridge, mut statuses) = connected.start(
            self.binding_generation.clone(),
            self.control.clone(),
            self.hello.consumer_id.clone(),
            move |bytes| {
                let output = output.clone();
                let wake = wake.clone();
                async move {
                    if output.send(Output { binding_epoch, bytes }).await.is_ok() {
                        wake();
                    }
                    Ok(())
                }
            },
        )?;
        let outbound = bridge.outbound.clone();
        let mut control_revision = ControlRevision::default();
        let mut control_status = self.control.subscribe();
        let mut ready = false;
        let mut pending_input: Option<Input> = None;
        let mut pending_viewport = None;
        let mut startup_replies = StartupTerminalReplyFilter::default();
        loop {
            tokio::select! {
                biased;
                _ = &mut self.close => {
                    retire(&mut bridge).await;
                    return Ok(true);
                }
                _ = bridge.disconnected() => return Ok(false),
                status = statuses.recv() => {
                    let Some(status) = status else { return Ok(false); };
                    if status.content_generation != self.hello.initial_content_generation { continue; }
                    ensure!(status.binding == self.hello.initial_binding,
                        "Host changed the desktop terminal binding");
                    let next_ready = status.lifecycle == TerminalConsumerLifecycleV1::Ready;
                    if next_ready && !ready {
                        self.input_epoch = self.input_epoch.checked_add(1).context("terminal input epoch exhausted")?;
                        pending_viewport = Some(true);
                        startup_replies.arm(Instant::now());
                    }
                    if !next_ready {
                        pending_input = None;
                    }
                    ready = next_ready;
                    self.generation.store(if ready { self.input_epoch } else { 0 }, Ordering::Release);
                    self.publish_recovery(status.recovery.as_deref().cloned(), TerminalRecoveryPhaseV1::StatusUnavailable);
                    self.status.send_replace(Some(status));
                    (self.wake)();
                }
                changed = self.viewport.changed(), if ready && pending_viewport.is_none() => {
                    if changed.is_err() { return Ok(false); }
                    pending_viewport = Some(false);
                }
                changed = control_status.changed() => {
                    if changed.is_err() { return Ok(false); }
                    (self.wake)();
                }
                Some(command) = self.content.recv() => {
                    match self.apply_content(command, Some(&outbound)) {
                        ContentEffect::Continue => {},
                        ContentEffect::Viewport => pending_viewport = Some(false),
                        ContentEffect::Retire => { retire(&mut bridge).await; return Ok(true); }
                        ContentEffect::Restart => { retire(&mut bridge).await; return Ok(false); }
                    }
                }
                permit = outbound.reserve(), if ready && (pending_viewport.is_some() || pending_input.is_some()) => {
                    let permit = permit.context("desktop terminal bridge stopped")?;
                    if let Some(recovering) = pending_viewport.take() {
                        permit.send(self.viewport_frame(&mut control_revision, recovering)?);
                    } else if let Some(input) = pending_input.take() {
                        permit.send(TerminalBridgeFrameV1::Input(input.bytes));
                    }
                }
                value = self.input.recv(), if pending_input.is_none() => {
                    let Some(input) = value else { return Ok(false); };
                    if ready && input.connection_generation == self.input_epoch {
                        let bytes = startup_replies.consume(&input.bytes, Instant::now(), false);
                        if !bytes.is_empty() {
                            pending_input = Some(Input { bytes, ..input });
                        }
                    }
                }
            }
        }
    }

    fn viewport_frame(
        &mut self,
        revisions: &mut ControlRevision,
        recovering: bool,
    ) -> Result<TerminalBridgeFrameV1> {
        let mut viewport = *self.viewport.borrow_and_update();
        if let Some(selection) = &self.native_selection {
            viewport.focused = selection.focused;
        }
        let cause = if recovering {
            TerminalControlCauseV1::Recovery
        } else if let Some(selection) = &self.native_selection {
            if revisions.selection < selection.sequence { selection.cause } else { TerminalControlCauseV1::Viewport }
        } else if viewport.focused == revisions.focused {
            TerminalControlCauseV1::Viewport
        } else if viewport.focused {
            TerminalControlCauseV1::Selection
        } else {
            TerminalControlCauseV1::Blur
        };
        if let Some(selection) = &self.native_selection {
            revisions.selection = selection.sequence;
        } else if cause != TerminalControlCauseV1::Viewport {
            revisions.selection = revisions
                .selection
                .checked_add(1)
                .context("terminal selection revision exhausted")?;
        }
        revisions.focused = viewport.focused;
        revisions.viewport = revisions
            .viewport
            .checked_add(1)
            .context("terminal viewport revision exhausted")?;
        Ok(TerminalBridgeFrameV1::Control(TerminalControlIntentV1 {
            native_instance_id: self.native_selection.as_ref().map_or(&self.hello.consumer_id, |s| &s.instance).clone(),
            binding_generation: self.hello.initial_content_generation,
            sequence: revisions.selection,
            viewport_revision: revisions.viewport,
            cause,
            focused: viewport.focused,
            size: viewport.size,
        }))
    }

    fn set_unavailable(&self) {
        self.generation.store(0, Ordering::Release);
        self.control.send_replace(Default::default());
        if self.status.send_replace(None).is_some() {
            (self.wake)();
        }
        self.publish_recovery(None, if self.channel.binding().is_some() {
            TerminalRecoveryPhaseV1::HostUnavailable
        } else {
            TerminalRecoveryPhaseV1::Hidden
        });
    }

    fn advance_connection(&mut self) -> bool {
        let Some(next) = self.hello.consumer_generation.checked_add(1) else { return false; };
        self.hello.consumer_generation = next;
        true
    }

    fn apply_content(
        &mut self,
        command: ContentCommand,
        outbound: Option<&mpsc::Sender<TerminalBridgeFrameV1>>,
    ) -> ContentEffect {
        let request = command.request;
        let identity = ResponseIdentity::from(&request);
        let mut effect = ContentEffect::Continue;
        let response = match recovery_request(&request, &self.channel) {
            Err(error) => identity.rejected("stale_terminal_control", &error.to_string(), false),
            Ok(true) => {
                let frame = match &request.command {
                    PaneContentCommandV1::ProbeRecovery => Some(TerminalBridgeFrameV1::Retry),
                    PaneContentCommandV1::DismissRecovery => Some(TerminalBridgeFrameV1::DismissRecovery),
                    PaneContentCommandV1::TerminalControl { native_instance_id, sequence, focused, cause } => {
                        if self.update_selection(native_instance_id.clone(), *sequence, *focused, *cause) {
                            effect = ContentEffect::Viewport;
                        }
                        None
                    }
                    PaneContentCommandV1::Focus { focused, sequence } => {
                        if self.update_selection(self.hello.consumer_id.clone(), *sequence, *focused,
                            if *focused { TerminalControlCauseV1::Selection } else { TerminalControlCauseV1::Blur }) {
                            effect = ContentEffect::Viewport;
                        }
                        None
                    }
                    _ => None,
                };
                if let (Some(outbound), Some(frame)) = (outbound, frame) {
                    if outbound.try_send(frame).is_err() { effect = ContentEffect::Restart; }
                }
                let mut response = recovery_response(&request, self.recovery.borrow().clone());
                if matches!(request.command, PaneContentCommandV1::ProbeTerminalControl | PaneContentCommandV1::TerminalControl { .. }) {
                    response.result = PaneContentResultV1::TerminalControl { state: self.control.borrow().clone() };
                }
                response
            }
            Ok(false) => match self.channel.begin(&request) {
                Err(error) => identity.rejected("content_request_rejected", &format!("{error:?}"), false),
                Ok(transition) => {
                    let frame = bridge_frame_for_action(transition.action(), request.content_revision);
                    let response = commit_response(&mut self.channel, transition, identity);
                    self.hello.initial_binding = self.channel.binding().cloned();
                    self.hello.portable_binding_version = self.hello.initial_binding.as_ref()
                        .and_then(|binding| binding.source.as_ref().map(|_| 1));
                    self.hello.initial_content_generation = self.channel.content_revision();
                    self.binding_generation.store(self.hello.initial_content_generation, Ordering::Release);
                    match frame {
                        Some(TerminalBridgeFrameV1::Retire) => {
                            self.output_epoch.store(self.channel.content_revision(), Ordering::Release);
                            self.set_unavailable();
                            effect = ContentEffect::Retire;
                        }
                        Some(frame @ TerminalBridgeFrameV1::RetainBinding { .. }) => {
                            if let Some(outbound) = outbound {
                                if outbound.try_send(frame).is_err() {
                                    self.set_unavailable();
                                    effect = ContentEffect::Restart;
                                }
                            }
                        }
                        Some(_) => {
                            self.output_epoch.store(self.channel.content_revision(), Ordering::Release);
                            self.set_unavailable();
                            self.publish_recovery(None, if self.channel.binding().is_some() {
                                TerminalRecoveryPhaseV1::Probing
                            } else {
                                TerminalRecoveryPhaseV1::Hidden
                            });
                            effect = ContentEffect::Restart;
                        }
                        None => {}
                    }
                    response
                }
            },
        };
        let _ = command.response.send(response);
        effect
    }

    fn update_selection(&mut self, instance: String, sequence: u64, focused: bool, cause: TerminalControlCauseV1) -> bool {
        if self.native_selection.as_ref().is_some_and(|previous| previous.instance == instance && previous.sequence >= sequence) {
            return false;
        }
        self.native_selection = Some(NativeSelection { instance, sequence, focused, cause });
        true
    }

    fn publish_recovery(&self, view: Option<TerminalRecoveryViewV1>, phase: TerminalRecoveryPhaseV1) {
        let mut view = view.unwrap_or_else(|| terminal_feedback(phase, None));
        view.content_generation = self.channel.content_revision();
        view.input_warning = false;
        view.input_warning_message.clear();
        if view.phase == TerminalRecoveryPhaseV1::Hidden { view.visible = false; }
        self.recovery.send_if_modified(|current| {
            if *current == view { return false; }
            *current = view;
            true
        });
    }
}

#[derive(PartialEq, Eq)]
enum ContentEffect {
    Continue,
    Viewport,
    Restart,
    Retire,
}

impl Drop for TerminalTask {
    fn drop(&mut self) {
        self.set_unavailable();
    }
}

async fn retire(bridge: &mut BridgeConnection) {
    let _ = tokio::time::timeout(RETIRE_TIMEOUT, async {
        if bridge
            .outbound
            .send(TerminalBridgeFrameV1::Retire)
            .await
            .is_ok()
        {
            bridge.disconnected().await;
        }
    })
    .await;
}
