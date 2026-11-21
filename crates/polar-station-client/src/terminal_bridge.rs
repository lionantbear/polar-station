//! Shared Unix socket handshake and framing for the Station terminal protocol.

use std::{
    future::Future,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use polar_local_access_runtime::{
    codec::{write_json_frame, MAX_FRAME_BYTES},
    effective_uid,
};
use polar_station_protocol::{
    decode_terminal_bridge_frame_v1, encode_terminal_bridge_frame_v1, LocalClientFrame,
    TerminalBridgeFrameV1, TerminalBridgeHelloV1, TerminalBridgeHelloV2, TerminalConsumerStatusV1,
    TERMINAL_CONTROL_API_VERSION,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc, watch},
};
use tracing::Instrument;

mod startup_replies;
pub use startup_replies::StartupTerminalReplyFilter;

pub struct ConnectedBridge {
    reader: tokio::net::unix::OwnedReadHalf,
    writer: tokio::net::unix::OwnedWriteHalf,
    status: TerminalConsumerStatusV1,
}

pub async fn connect_terminal_bridge(
    host_socket: &Path,
    hello: TerminalBridgeHelloV1,
) -> Result<ConnectedBridge> {
    verify_server_socket(host_socket)?;
    let mut host = UnixStream::connect(host_socket)
        .await
        .with_context(|| format!("failed to connect to Host at {}", host_socket.display()))?;
    ensure!(
        host.peer_cred()
            .context("failed to verify Host credentials")?
            .uid()
            == effective_uid(),
        "local IPC Host has a different UID"
    );
    let expected_binding = hello.initial_binding.clone();
    let consumer_id = hello.consumer_id.clone();
    let consumer_generation = hello.consumer_generation;
    write_json_frame(
        &mut host,
        &LocalClientFrame::TerminalBridgeV2 {
            hello: TerminalBridgeHelloV2 {
                base: hello,
                terminal_control_version: TERMINAL_CONTROL_API_VERSION,
            },
        },
    )
    .await?;
    let status = read_next_status(&mut host).await?;
    ensure!(
        status.binding == expected_binding,
        "Host acknowledged a different terminal binding"
    );
    ensure!(
        status.consumer_id == consumer_id && status.consumer_generation == consumer_generation,
        "Host Consumer identity mismatch"
    );
    ensure!(
        status.recovery.is_some(),
        "Host does not support terminal recovery status; update the matching runtime"
    );
    ensure!(
        matches!(
            read_terminal_bridge_frame(&mut host).await?,
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

pub fn verify_server_socket(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect local IPC socket {}", path.display()))?;
    if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
        bail!(
            "local IPC server must be a Unix socket owned by the current user: {}",
            path.display()
        );
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "local IPC server permissions are not private: {}",
            path.display()
        );
    }
    Ok(())
}

pub async fn read_terminal_bridge_frame<R>(reader: &mut R) -> Result<Option<TerminalBridgeFrameV1>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 5];
    let read = reader
        .read(&mut header)
        .await
        .context("failed to read terminal bridge frame header")?;
    if read == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut header[read..])
        .await
        .context("truncated terminal bridge frame header")?;
    let length = u32::from_be_bytes(
        header[1..]
            .try_into()
            .expect("terminal bridge header has a four-byte length"),
    ) as usize;
    if length > MAX_FRAME_BYTES {
        bail!("terminal bridge frame length exceeds {MAX_FRAME_BYTES}");
    }
    let mut encoded = Vec::with_capacity(header.len() + length);
    encoded.extend_from_slice(&header);
    encoded.resize(header.len() + length, 0);
    reader
        .read_exact(&mut encoded[header.len()..])
        .await
        .context("failed to read terminal bridge frame payload")?;
    decode_terminal_bridge_frame_v1(&encoded)
        .context("invalid terminal bridge frame")
        .map(Some)
}

pub async fn write_terminal_bridge_frame<W>(
    writer: &mut W,
    frame: &TerminalBridgeFrameV1,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded =
        encode_terminal_bridge_frame_v1(frame).context("invalid terminal bridge frame")?;
    writer
        .write_all(&encoded)
        .await
        .context("failed to write terminal bridge frame")?;
    writer
        .flush()
        .await
        .context("failed to flush terminal bridge frame")
}

async fn read_next_status(stream: &mut UnixStream) -> Result<TerminalConsumerStatusV1> {
    match read_terminal_bridge_frame(stream).await? {
        Some(TerminalBridgeFrameV1::Status(status)) => Ok(status),
        Some(TerminalBridgeFrameV1::Output(_)) => {
            bail!("Host emitted terminal output before bridge readiness")
        }
        Some(_) => bail!("Host sent a Consumer-only terminal frame"),
        None => bail!("Host closed before terminal bridge readiness"),
    }
}

pub const BRIDGE_QUEUE_CAPACITY: usize = 64;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(50);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub struct ReconnectBackoff {
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
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = std::cmp::min(self.next.saturating_mul(2), MAX_RECONNECT_DELAY);
        delay
    }

    pub fn reset(&mut self) {
        self.next = INITIAL_RECONNECT_DELAY;
    }
}

pub struct BridgeConnection {
    pub outbound: mpsc::Sender<TerminalBridgeFrameV1>,
    reader: tokio::task::JoinHandle<Result<()>>,
    writer: tokio::task::JoinHandle<Result<()>>,
}
impl Drop for BridgeConnection {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

impl BridgeConnection {
    pub async fn disconnected(&mut self) {
        tokio::select! { _ = &mut self.reader => {}, _ = &mut self.writer => {} }
    }
}

impl ConnectedBridge {
    /// Start one ordered reader and writer. The output sink can apply backpressure
    /// asynchronously. It owns all bytes until the terminal has consumed them.
    pub fn start<F>(
        self,
        desired_revision: Arc<AtomicU64>,
        control_views: watch::Sender<polar_station_protocol::TerminalControlStatusV1>,
        trace_channel: String,
        output: impl FnMut(Vec<u8>) -> F + Send + 'static,
    ) -> Result<(BridgeConnection, mpsc::Receiver<TerminalConsumerStatusV1>)>
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        sender
            .try_send(self.status.clone())
            .context("initial bridge status queue unavailable")?;
        let (outbound, mut frames) = mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        let mut writer = self.writer;
        let reader = tokio::spawn(read_host_frames(
            self.reader,
            sender,
            output,
            desired_revision,
            control_views,
            self.status,
        ));
        let writer = tokio::spawn(async move {
            let mut focus_ordinal: u64 = 0;
            while let Some(frame) = frames.recv().await {
                tokio::time::timeout(
                    Duration::from_secs(8),
                    write_terminal_bridge_frame(&mut writer, &frame),
                )
                .await
                .context("Host write timed out")??;
                if let TerminalBridgeFrameV1::Control(intent) = &frame {
                    tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_control_forwarded",
                        consumer_id=trace_channel.as_str(), sequence=intent.sequence, viewport_revision=intent.viewport_revision,
                        operation_id=format!("{}-{}", trace_channel, intent.sequence),
                        content_generation=intent.binding_generation, focused=intent.focused,
                        columns=intent.size.columns, rows=intent.size.rows);
                }
                if let TerminalBridgeFrameV1::Focus {
                    focused,
                    binding_generation,
                    size,
                } = frame
                {
                    focus_ordinal += 1;
                    tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_focus_forwarded", consumer_id=trace_channel.as_str(), ordinal=focus_ordinal, content_generation=binding_generation, focused, columns=size.columns, rows=size.rows);
                }
            }
            Ok(())
        });
        Ok((
            BridgeConnection {
                outbound,
                reader,
                writer,
            },
            receiver,
        ))
    }
}

async fn read_host_frames<F>(
    mut reader: tokio::net::unix::OwnedReadHalf,
    statuses: mpsc::Sender<TerminalConsumerStatusV1>,
    mut output: impl FnMut(Vec<u8>) -> F,
    desired_revision: Arc<AtomicU64>,
    control: watch::Sender<polar_station_protocol::TerminalControlStatusV1>,
    initial_status: TerminalConsumerStatusV1,
) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    let mut observed_revision = initial_status.content_generation;
    let mut first_output = true;
    let consumer_id = initial_status.consumer_id;
    let generation = initial_status.consumer_generation;
    while let Some(frame) = read_terminal_bridge_frame(&mut reader).await? {
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
                    let byte_count = bytes.len();
                    let result = output(bytes).instrument(span.clone()).await;
                    span.record("outcome", if result.is_ok() { "ok" } else { "failed" });
                    result?;
                    if first_output {
                        tracing::info!(target: "polar_station_operation", component="PaneHost", event="pane_terminal_output_written",
                            consumer_id=consumer_id.as_str(), generation, content_generation=observed_revision, bytes=byte_count as u64);
                        first_output = false;
                    }
                }
            }
            TerminalBridgeFrameV1::Status(status) => {
                ensure!(
                    status.consumer_id == consumer_id && status.consumer_generation == generation,
                    "Host changed Consumer identity during the connection"
                );
                if observed_revision != status.content_generation {
                    first_output = true;
                }
                observed_revision = status.content_generation;
                if statuses.send(status).await.is_err() {
                    return Ok(());
                }
            }
            _ => bail!("Polar Tmux Host sent a Consumer-only terminal frame"),
        }
    }
    Ok(())
}


