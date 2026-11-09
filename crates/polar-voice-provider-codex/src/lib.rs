//! Codex-backed adapter for the provider-independent Polar Voice domain.
//!
//! This is the only Polar Voice crate allowed to know Codex authentication or
//! provider-private Realtime/WebRTC details. Callers still submit one complete
//! PCM recording and receive final text.

pub mod dictate;
pub mod realtime;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use codex_api::{
    build_session_headers, AuthProvider, Provider, RealtimeCallClient, RealtimeEventParser,
    RealtimeOutputModality, RealtimeSessionConfig, RealtimeSessionMode, RealtimeWebsocketClient,
    RealtimeWebsocketConnection, ReqwestTransport, RetryConfig, SharedAuthProvider,
};
use codex_http_client::{ClientRouteClass, HttpClientFactory, OutboundProxyPolicy};
use codex_login::{default_client, AuthManager, AuthRouteConfig, CodexAuth};
pub use codex_login::{AuthCredentialsStoreMode, AuthKeyringBackendKind};
use codex_protocol::{
    protocol::{RealtimeEvent, RealtimeVoice},
    SessionId,
};
use http::{HeaderMap, HeaderValue};
use polar_voice::{
    VoiceError, VoiceOpusPacket, VoicePcm, VoiceStreamFormat, VoiceStreamingSession,
    VoiceStreamingTranscriptionProvider, VoiceTranscriptionProvider,
};
use ropus::{Application, Channels, Encoder};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use polar_voice::transcription::TranscriptSnapshot;
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_sample::TrackLocalStaticSample, TrackLocal},
};
use zeroize::Zeroize;

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_V1_BASE_URL: &str = "https://api.openai.com/v1";
const ORIGINATOR: &str = "polar_voice";
const REALTIME_MODEL: &str = "gpt-live-1-codex";
const OPUS_SAMPLE_RATE_HZ: u32 = 48_000;
const OPUS_SAMPLES_PER_FRAME: usize = 960;
const OPUS_FRAME_DURATION: Duration = Duration::from_millis(20);
const TRANSCRIPT_WAIT: Duration = Duration::from_secs(30);
const JITTER_TARGET_PACKETS: usize = 3;
const JITTER_HARD_LIMIT_PACKETS: usize = 5;
const JITTER_RESERVE_PACKETS: u32 = 2;
const MAXIMUM_CONSECUTIVE_MISSING_PACKETS: u32 = 5;
const MAXIMUM_CONSECUTIVE_STALL_PACKETS: u32 = 50;
const PROVIDER_TAIL_SILENCE_PACKETS: u32 = 25;

/// A canonical 20 ms Opus silence frame at 48 kHz. It is forwarded without
/// decode or re-encode for both loss concealment and the provider-only tail.
pub const OPUS_SILENCE_PACKET: &[u8] = &[0xf8, 0xff, 0xfe];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JitterPlayout {
    Wait,
    Packet(Vec<u8>),
    ReserveSilence,
    StallSilence,
    Silence,
    TailSilence,
    Complete,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderJitterStats {
    pub maximum_buffered_packets: usize,
    pub late_packets: u32,
    pub duplicate_packets: u32,
    pub jitter_reserve_packets: u32,
    pub stall_silence_packets: u32,
    pub concealment_packets: u32,
    pub maximum_consecutive_stalled: u32,
    pub maximum_consecutive_missing: u32,
}

#[derive(Debug, Default)]
struct ProviderIngressTiming {
    previous_packet_at: Option<Instant>,
    maximum_gap: Duration,
}

impl ProviderIngressTiming {
    fn observe(&mut self, packet_at: Instant) -> Option<Duration> {
        let gap = self
            .previous_packet_at
            .map(|previous_packet_at| packet_at.saturating_duration_since(previous_packet_at));
        self.previous_packet_at = Some(packet_at);
        if let Some(gap) = gap {
            self.maximum_gap = self.maximum_gap.max(gap);
        }
        gap
    }

    fn maximum_gap(&self) -> Duration {
        self.maximum_gap
    }
}

/// Recording-scoped, packet-preserving jitter buffer. Timing and WebRTC
/// playout are provider concerns; the Voice domain owns sequence validity and
/// the declared recording manifest.
#[derive(Debug)]
pub struct ProviderJitterBuffer {
    packets: BTreeMap<u32, Vec<u8>>,
    next_sequence: u32,
    started: bool,
    committed_packet_count: Option<u32>,
    jitter_reserve_remaining: u32,
    consecutive_stalled: u32,
    consecutive_missing: u32,
    tail_silence_remaining: u32,
    complete: bool,
    stats: ProviderJitterStats,
}

impl Default for ProviderJitterBuffer {
    fn default() -> Self {
        Self {
            packets: BTreeMap::new(),
            next_sequence: 0,
            started: false,
            committed_packet_count: None,
            jitter_reserve_remaining: JITTER_RESERVE_PACKETS,
            consecutive_stalled: 0,
            consecutive_missing: 0,
            tail_silence_remaining: 0,
            complete: false,
            stats: ProviderJitterStats::default(),
        }
    }
}

impl ProviderJitterBuffer {
    pub fn can_accept_packet(&self) -> bool {
        self.packets.len() < JITTER_HARD_LIMIT_PACKETS
    }

    pub fn push(&mut self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        if packet.sequence < self.next_sequence {
            self.stats.late_packets = self.stats.late_packets.saturating_add(1);
            return Ok(());
        }
        if self.packets.contains_key(&packet.sequence) {
            self.stats.duplicate_packets = self.stats.duplicate_packets.saturating_add(1);
            return Ok(());
        }
        if self.packets.len() >= JITTER_HARD_LIMIT_PACKETS {
            return Err(VoiceError::MediaJitterExceeded);
        }
        self.packets.insert(packet.sequence, packet.payload);
        self.stats.maximum_buffered_packets =
            self.stats.maximum_buffered_packets.max(self.packets.len());
        Ok(())
    }

    pub fn commit(&mut self, packet_count: u32) -> Result<(), VoiceError> {
        if self
            .committed_packet_count
            .is_some_and(|existing| existing != packet_count)
            || self
                .packets
                .keys()
                .any(|sequence| *sequence >= packet_count)
            || self.next_sequence > packet_count
        {
            return Err(VoiceError::InvalidStream);
        }
        self.committed_packet_count = Some(packet_count);
        self.tail_silence_remaining = PROVIDER_TAIL_SILENCE_PACKETS;
        Ok(())
    }

    pub fn tick(&mut self) -> Result<JitterPlayout, VoiceError> {
        if self.complete {
            return Ok(JitterPlayout::Complete);
        }
        if !self.started {
            if self.packets.len() < JITTER_TARGET_PACKETS && self.committed_packet_count.is_none() {
                return Ok(JitterPlayout::Wait);
            }
            self.started = true;
        }
        if self
            .committed_packet_count
            .is_some_and(|packet_count| self.next_sequence >= packet_count)
        {
            if self.tail_silence_remaining > 0 {
                self.tail_silence_remaining -= 1;
                return Ok(JitterPlayout::TailSilence);
            }
            self.complete = true;
            return Ok(JitterPlayout::Complete);
        }
        if let Some(payload) = self.packets.remove(&self.next_sequence) {
            self.next_sequence += 1;
            self.consecutive_stalled = 0;
            self.consecutive_missing = 0;
            self.jitter_reserve_remaining = JITTER_RESERVE_PACKETS;
            return Ok(JitterPlayout::Packet(payload));
        }

        if self.jitter_reserve_remaining > 0 {
            self.jitter_reserve_remaining -= 1;
            self.stats.jitter_reserve_packets = self.stats.jitter_reserve_packets.saturating_add(1);
            return Ok(JitterPlayout::ReserveSilence);
        }

        let confirmed_gap = self
            .packets
            .first_key_value()
            .is_some_and(|(sequence, _)| *sequence > self.next_sequence)
            || self.committed_packet_count.is_some();
        if confirmed_gap {
            self.next_sequence += 1;
            self.consecutive_stalled = 0;
            self.consecutive_missing = self.consecutive_missing.saturating_add(1);
            self.stats.concealment_packets = self.stats.concealment_packets.saturating_add(1);
            self.stats.maximum_consecutive_missing = self
                .stats
                .maximum_consecutive_missing
                .max(self.consecutive_missing);
            if self.consecutive_missing > MAXIMUM_CONSECUTIVE_MISSING_PACKETS {
                return Err(VoiceError::MediaJitterExceeded);
            }
            return Ok(JitterPlayout::Silence);
        }

        self.consecutive_stalled = self.consecutive_stalled.saturating_add(1);
        self.stats.stall_silence_packets = self.stats.stall_silence_packets.saturating_add(1);
        self.stats.maximum_consecutive_stalled = self
            .stats
            .maximum_consecutive_stalled
            .max(self.consecutive_stalled);
        if self.consecutive_stalled >= MAXIMUM_CONSECUTIVE_STALL_PACKETS {
            return Err(VoiceError::MediaJitterExceeded);
        }
        Ok(JitterPlayout::StallSilence)
    }

    pub fn stats(&self) -> ProviderJitterStats {
        self.stats
    }
}

#[derive(Clone)]
pub struct CodexVoiceProvider {
    auth_manager: Option<Arc<AuthManager>>,
    vault_auth: Option<Arc<VaultAuth>>,
    http_client_factory: HttpClientFactory,
    prepared: Arc<Mutex<Option<ProviderResources>>>,
}

impl CodexVoiceProvider {
    /// Loads the explicitly supplied node voice home and reuses its native AuthManager
    /// across all transcription calls.
    pub async fn from_codex_home(codex_home: PathBuf) -> Self {
        Self::from_codex_auth_store(
            codex_home,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .await
    }

    /// Reuse an existing native login in its configured store, without copying
    /// credentials to another owner. Product composition supplies the same
    /// store/backend options as the local Codex configuration.
    pub async fn from_codex_auth_store(
        codex_home: PathBuf,
        store: AuthCredentialsStoreMode,
        keyring_backend: AuthKeyringBackendKind,
    ) -> Self {
        let http_client_factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
        let auth_route_config =
            AuthRouteConfig::from_http_client_factory(http_client_factory.clone());
        let auth_manager = Arc::new(
            AuthManager::new(
                codex_home,
                /*enable_codex_api_key_env*/ false,
                store,
                /*forced_chatgpt_workspace_id*/ None,
                /*chatgpt_base_url*/ None,
                keyring_backend,
                auth_route_config,
            )
            .await,
        );
        Self {
            prepared: Arc::new(Mutex::new(None)),
            auth_manager: Some(auth_manager),
            vault_auth: None,
            http_client_factory,
        }
    }

    /// Installed product composition: Vault is the only persistent auth owner.
    pub async fn from_vault(
        vault: Arc<polar_vault::PolarVault>,
        scope: String,
        name: String,
    ) -> Self {
        let provider = Self {
            prepared: Arc::new(Mutex::new(None)),
            auth_manager: None,
            vault_auth: Some(Arc::new(VaultAuth {
                vault,
                scope,
                name,
                snapshot: std::sync::RwLock::new(None),
                rejected: std::sync::RwLock::new(None),
                revision: std::sync::atomic::AtomicU64::new(0),
            })),
            http_client_factory: HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        };
        provider.vault_auth.as_ref().unwrap().refresh().await;
        provider
    }

    pub fn auth_manager(&self) -> Option<&Arc<AuthManager>> {
        self.auth_manager.as_ref()
    }

    async fn current_auth(&self) -> Result<CodexAuth, VoiceError> {
        let manager = self.auth_manager.as_ref().ok_or(VoiceError::AuthRequired)?;
        let auth = manager.auth().await.ok_or(VoiceError::AuthRequired)?;
        if !auth.uses_codex_backend() || manager.refresh_failure_for_auth(&auth).is_some() {
            return Err(VoiceError::AuthRequired);
        }
        Ok(auth)
    }

    /// All Voice request paths share the same credential owner and rejection state.
    async fn request_auth(&self) -> Result<Arc<SnapshotAuth>, VoiceError> {
        if let Some(vault) = self.vault_auth.as_ref() {
            vault.refresh().await;
            let snapshot = vault
                .snapshot
                .read()
                .unwrap()
                .clone()
                .ok_or(VoiceError::AuthRequired)?;
            if vault.rejected.read().unwrap().as_ref() == Some(&snapshot) {
                return Err(VoiceError::AuthRequired);
            }
            Ok(snapshot)
        } else {
            snapshot_auth_provider(&self.current_auth().await?)
        }
    }

    fn reject_auth(&self, snapshot: Arc<SnapshotAuth>) {
        if let Some(vault) = self.vault_auth.as_ref() {
            *vault.rejected.write().unwrap() = Some(snapshot);
        }
    }

    async fn transcribe_complete_pcm(&self, audio: &[u8]) -> Result<String, VoiceError> {
        let mut resources = self.connect_media_session().await?;
        let result = async {
            resources
                .peer()
                .replay_pcm(audio)
                .await
                .map_err(|_| voice_failure("media_replay", VoiceError::TranscriptionFailed))?;
            resources.sideband().writer().send_payload(serde_json::json!({"type":"session.close"}).to_string())
                .await.map_err(|_| VoiceError::TranscriptionFailed)?;
            wait_for_transcript(&resources).await
        }
        .await;

        resources.close_all().await;
        result
    }

    async fn connect_media_session(&self) -> Result<ProviderResources, VoiceError> {
        self.connect_media_session_for(None, None).await
    }

    async fn connect_media_session_for(
        &self,
        request: Option<&polar_voice::realtime::VoiceRealtimeStart>,
        output: Option<Arc<realtime::AudioQueue>>,
    ) -> Result<ProviderResources, VoiceError> {
        let snapshot = self.request_auth().await?;
        let auth_provider: SharedAuthProvider = snapshot.clone();
        let call_provider = chatgpt_provider();
        let request_url = call_provider.url_for_path("realtime/calls");
        let http_client = default_client::create_client_for_route(
            &self.http_client_factory,
            &request_url,
            ClientRouteClass::Api,
            default_client::ClientRedirectPolicy::Default,
        )
        .map_err(|_| voice_failure("http_client", VoiceError::ProviderUnavailable))?;
        let transport = ReqwestTransport::from_http_client(http_client);

        let peer = ServerMediaPeer::new_with_channels(if request.is_some() { 2 } else { 1 })
            .await
            .map_err(|_| voice_failure("peer_create", VoiceError::ProviderUnavailable))?;
        let mut resources = ProviderResources::new(peer);
        resources.auth_scope=Some(snapshot.clone());
        if let Some(output) = output {
            realtime::attach_output(&resources.peer().peer, output);
        }
        let connect_result = async {
            let offer = resources
                .peer()
                .create_offer()
                .await
                .map_err(|_| voice_failure("offer_create", VoiceError::TranscriptionFailed))?;
            let session_id = provider_correlation_id();
            let thread_correlation_id = provider_correlation_id();
            let session = if let Some(request) = request {
                realtime::session_config(request, &thread_correlation_id)
            } else {
                transcription_session(&thread_correlation_id)
            };
            let call_headers = realtime_call_headers(&session_id, &thread_correlation_id);
            let call =
                RealtimeCallClient::new(transport, call_provider, Arc::clone(&auth_provider))
                    .create_with_session_and_headers(offer, session.clone(), call_headers)
                    .await
                    .map_err(|error| {
                        provider_api_failure("call_create", &error, VoiceError::TranscriptionFailed)
                    })?;

            let mut sideband_headers = realtime_call_headers(&session_id, &thread_correlation_id);
            sideband_headers.extend(auth_provider.to_auth_headers());
            let sideband =
                RealtimeWebsocketClient::new(sideband_provider(), self.http_client_factory.clone())
                    .connect_webrtc_sideband(
                        session,
                        &call.call_id,
                        sideband_headers,
                        default_client::default_headers(),
                        codex_api::RealtimeTranscriptState::default(),
                    )
                    .await
                    .map_err(|error| {
                        provider_api_failure(
                            "sideband_connect",
                            &error,
                            VoiceError::TranscriptionFailed,
                        )
                    })?;
            resources.install_sideband(sideband);

            resources
                .peer()
                .apply_answer(call.sdp)
                .await
                .map_err(|_| voice_failure("answer_apply", VoiceError::TranscriptionFailed))?;
            resources
                .peer()
                .wait_until_connected()
                .await
                .map_err(|_| voice_failure("peer_connect", VoiceError::Timeout))
        }
        .await;
        match connect_result {
            Ok(()) => Ok(resources),
            Err(error) => {
                if error == VoiceError::AuthRequired {
                    self.reject_auth(snapshot);
                }
                resources.close_all().await;
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl VoiceTranscriptionProvider for CodexVoiceProvider {
    async fn transcribe(&self, audio: VoicePcm) -> Result<String, VoiceError> {
        self.transcribe_complete_pcm(audio.as_bytes()).await
    }
}

enum StreamingCommand {
    Packet {
        packet: VoiceOpusPacket,
        arrived_at: Instant,
    },
    Commit(u32),
    Abort,
}

struct CodexStreamingSession {
    transcript: watch::Receiver<TranscriptSnapshot>,
    commands: mpsc::Sender<StreamingCommand>,
    result: Mutex<Option<oneshot::Receiver<Result<String, VoiceError>>>>,
    terminal_error: Arc<Mutex<Option<VoiceError>>>,
}

impl CodexStreamingSession {
    async fn terminal_or(&self, fallback: VoiceError) -> VoiceError {
        self.terminal_error.lock().await.clone().unwrap_or(fallback)
    }
}

#[async_trait::async_trait]
impl VoiceStreamingSession for CodexStreamingSession {
    fn transcript_updates(&self) -> Option<watch::Receiver<TranscriptSnapshot>> {
        Some(self.transcript.clone())
    }

    async fn append(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        if self
            .commands
            .send(StreamingCommand::Packet {
                packet,
                arrived_at: Instant::now(),
            })
            .await
            .is_err()
        {
            return Err(self.terminal_or(VoiceError::TranscriptionFailed).await);
        }
        Ok(())
    }

    async fn commit(&self, packet_count: u32) -> Result<String, VoiceError> {
        if self
            .commands
            .send(StreamingCommand::Commit(packet_count))
            .await
            .is_err()
        {
            return Err(self.terminal_or(VoiceError::TranscriptionFailed).await);
        }
        self.result
            .lock()
            .await
            .take()
            .ok_or(VoiceError::InvalidStream)?
            .await
            .map_err(|_| VoiceError::TranscriptionFailed)?
    }

    async fn abort(&self) -> Result<(), VoiceError> {
        self.commands
            .send(StreamingCommand::Abort)
            .await
            .map_err(|_| VoiceError::TranscriptionFailed)?;
        if let Some(result) = self.result.lock().await.take() {
            let _ = result.await;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl VoiceStreamingTranscriptionProvider for CodexVoiceProvider {
    async fn prepare(&self) -> Result<(),VoiceError> {
        let auth=match self.request_auth().await {
            Ok(auth)=>auth,
            Err(error)=>{self.release_prepared().await;return Err(error);}
        };
        let mut slot=self.prepared.lock().await;
        if let Some(resources)=slot.as_ref() {
            if resources.prepared_usable(&auth).await {return Ok(());}
        }
        if let Some(mut resources)=slot.take() {resources.close_all().await;}
        *slot=Some(self.connect_media_session().await?);
        Ok(())
    }
    async fn release_prepared(&self) {
        let resources=self.prepared.lock().await.take();
        if let Some(mut resources)=resources {resources.close_all().await;}
    }
    async fn begin(
        &self,
        format: VoiceStreamFormat,
    ) -> Result<Arc<dyn VoiceStreamingSession>, VoiceError> {
        if format != VoiceStreamFormat::opus_16khz_mono() {
            return Err(VoiceError::InvalidStream);
        }
        let auth=match self.request_auth().await {
            Ok(auth)=>auth,
            Err(error)=>{self.release_prepared().await;return Err(error);}
        };
        let prepared=self.prepared.lock().await.take();
        let resources=match prepared {
            Some(resources) if resources.prepared_usable(&auth).await => resources,
            Some(mut resources)=>{resources.close_all().await;self.connect_media_session().await?},
            None=>self.connect_media_session().await?,
        };
        let (command_tx, command_rx) = mpsc::channel(1);
        let (result_tx, result_rx) = oneshot::channel();
        let terminal_error = Arc::new(Mutex::new(None));
        let actor_terminal_error = Arc::clone(&terminal_error);
        let (updates, transcript) = watch::channel(TranscriptSnapshot::default());
        tokio::spawn(async move {
            let result = run_streaming_session(resources, command_rx, updates).await;
            if let Err(error) = result.as_ref() {
                *actor_terminal_error.lock().await = Some(error.clone());
            }
            let _ = result_tx.send(result);
        });
        Ok(Arc::new(CodexStreamingSession {
            transcript,
            commands: command_tx,
            result: Mutex::new(Some(result_rx)),
            terminal_error,
        }))
    }
}

async fn run_streaming_session(
    mut resources: ProviderResources,
    mut commands: mpsc::Receiver<StreamingCommand>,
    updates: watch::Sender<TranscriptSnapshot>,
) -> Result<String, VoiceError> {
    let mut jitter = ProviderJitterBuffer::default();
    let mut ingress_timing = ProviderIngressTiming::default();
    let mut maximum_media_write = Duration::ZERO;
    let mut playout = tokio::time::interval(OPUS_FRAME_DURATION);
    playout.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    playout.tick().await;
    let result = async {
        loop {
            tokio::select! {
                biased;
                command = commands.recv(), if jitter.can_accept_packet() => match command {
                    Some(StreamingCommand::Packet { packet, arrived_at }) => {
                        if let Some(gap) = ingress_timing.observe(arrived_at) {
                            if gap > Duration::from_millis(100) {
                                tracing::warn!(
                                    event = "polar_voice_provider_ingress_gap",
                                    sequence = packet.sequence,
                                    gap_ms = gap.as_millis(),
                                    "Polar Voice provider observed a delayed Opus packet"
                                );
                            }
                        }
                        jitter.push(packet)?;
                    }
                    Some(StreamingCommand::Commit(packet_count)) => jitter.commit(packet_count)?,
                    Some(StreamingCommand::Abort) | None => {
                        return Err(VoiceError::TranscriptionFailed);
                    }
                },
                event = resources.sideband().next_event() => {
                    let event = event.map_err(|_| VoiceError::TranscriptionFailed)?;
                    update_transcript(&updates, event)?;
                },
                _ = playout.tick() => match jitter.tick()? {
                    JitterPlayout::Wait => {}
                    JitterPlayout::Packet(packet) => write_playout_packet(
                        &resources,
                        &packet,
                        &mut maximum_media_write,
                    ).await?,
                    JitterPlayout::ReserveSilence | JitterPlayout::StallSilence | JitterPlayout::Silence | JitterPlayout::TailSilence => write_playout_packet(
                        &resources,
                        OPUS_SILENCE_PACKET,
                        &mut maximum_media_write,
                    ).await?,
                    JitterPlayout::Complete => {
                        // Reuse the pinned SDK's session.close protocol message,
                        // but drain final events before closing the WebSocket.
                        // Writer::close marks its reader closed immediately.
                        resources.sideband().writer().send_payload(
                            serde_json::json!({"type": "session.close"}).to_string(),
                        ).await.map_err(|_| voice_failure("stream_finish", VoiceError::TranscriptionFailed))?;
                        tokio::select! {
                            result = wait_for_transcript_updates(&resources, Some(&updates)) => return result,
                            command = commands.recv() => return Err(match command {
                                Some(StreamingCommand::Abort) | None => VoiceError::TranscriptionFailed,
                                _ => VoiceError::InvalidStream,
                            }),
                        }
                    },
                }
            }
        }
    }
    .await;
    let stats = jitter.stats();
    let transcript = updates.borrow().clone();
    tracing::info!(
        event = "polar_voice_provider_stream_finished",
        success = result.is_ok(),
        error = ?result.as_ref().err(),
        confirmed_segments = transcript.confirmed.len(),
        partial_bytes = transcript.partial.len(),
        speech_pending = transcript.speech_pending,
        maximum_buffered_packets = stats.maximum_buffered_packets,
        late_packets = stats.late_packets,
        duplicate_packets = stats.duplicate_packets,
        jitter_reserve_packets = stats.jitter_reserve_packets,
        stall_silence_packets = stats.stall_silence_packets,
        concealment_packets = stats.concealment_packets,
        maximum_consecutive_stalled = stats.maximum_consecutive_stalled,
        maximum_consecutive_missing = stats.maximum_consecutive_missing,
        maximum_ingress_gap_ms = ingress_timing.maximum_gap().as_millis(),
        maximum_media_write_ms = maximum_media_write.as_millis(),
        "Polar Voice provider stream finished"
    );
    resources.close_all().await;
    result
}

async fn write_playout_packet(
    resources: &ProviderResources,
    packet: &[u8],
    maximum_media_write: &mut Duration,
) -> Result<(), VoiceError> {
    let started_at = Instant::now();
    resources
        .peer()
        .write_opus_packet(packet)
        .await
        .map_err(|_| voice_failure("media_stream", VoiceError::TranscriptionFailed))?;
    let elapsed = started_at.elapsed();
    *maximum_media_write = (*maximum_media_write).max(elapsed);
    if elapsed > OPUS_FRAME_DURATION {
        tracing::warn!(
            event = "polar_voice_provider_media_write_delayed",
            elapsed_ms = elapsed.as_millis(),
            "Polar Voice provider WebRTC media write exceeded one Opus frame"
        );
    }
    Ok(())
}

fn update_transcript(updates: &watch::Sender<TranscriptSnapshot>, event: Option<RealtimeEvent>) -> Result<(), VoiceError> {
    match event {
        Some(RealtimeEvent::InputAudioSpeechStarted(_)) => {
            updates.send_modify(|snapshot| snapshot.speech_pending = true);
        }
        Some(RealtimeEvent::InputTranscriptDelta(value)) => {
            updates.send_modify(|snapshot| snapshot.append_delta(&value.delta));
        }
        Some(RealtimeEvent::InputTranscriptDone(value)) => {
            updates.send_modify(|snapshot| snapshot.finish_segment(value.text));
        }
        Some(RealtimeEvent::Error(_)) | None => return Err(VoiceError::TranscriptionFailed),
        _ => {}
    }
    Ok(())
}

async fn wait_for_transcript(resources: &ProviderResources) -> Result<String, VoiceError> {
    wait_for_transcript_updates(resources, None).await
}

async fn wait_for_transcript_updates(resources: &ProviderResources, updates: Option<&watch::Sender<TranscriptSnapshot>>) -> Result<String, VoiceError> {
    let segments = collect_until_closed(|| async {
        resources.sideband().next_event().await.map_err(|_| voice_failure("sideband_event", VoiceError::TranscriptionFailed))
    }, updates, tokio::time::Instant::now() + TRANSCRIPT_WAIT).await?;
    final_transcript_text(resources, &segments, updates).await.ok_or(VoiceError::TranscriptionFailed)
}

async fn collect_until_closed<F, Fut>(mut next: F, updates: Option<&watch::Sender<TranscriptSnapshot>>, deadline: tokio::time::Instant) -> Result<Vec<String>, VoiceError>
where F: FnMut() -> Fut, Fut: std::future::Future<Output=Result<Option<RealtimeEvent>,VoiceError>> {
    let mut segments = updates.map(|tx| tx.borrow().confirmed.clone()).unwrap_or_default();
    let mut pending = updates.is_some_and(|tx| tx.borrow().speech_pending);
    loop {
        let event=tokio::time::timeout_at(deadline,next()).await.map_err(|_|VoiceError::Timeout)??;
        match event {
            Some(RealtimeEvent::InputAudioSpeechStarted(_)) => {
                pending=true;
                if let Some(updates)=updates { updates.send_modify(|snapshot|snapshot.speech_pending=true); }
            }
            Some(RealtimeEvent::InputTranscriptDelta(delta)) => {
                pending=true;
                if let Some(updates)=updates { updates.send_modify(|snapshot|snapshot.append_delta(&delta.delta)); }
            }
            Some(RealtimeEvent::InputTranscriptDone(done)) => {
                pending=false;
                if let Some(updates)=updates { updates.send_modify(|snapshot|snapshot.finish_segment(done.text.clone())); }
                segments.push(done.text);
            }
            Some(RealtimeEvent::Error(_)) => return Err(VoiceError::TranscriptionFailed),
            Some(_) => {},
            None => {
                if pending || segments.is_empty() { return Err(VoiceError::TranscriptionFailed); }
                return Ok(segments);
            }
        }
    }
}

async fn final_transcript_text(resources: &ProviderResources, completed: &[String], updates: Option<&watch::Sender<TranscriptSnapshot>>) -> Option<String> {
    if let Some(updates) = updates {
        let snapshot = updates.borrow();
        return (!snapshot.speech_pending && !snapshot.confirmed.is_empty()).then(|| snapshot.confirmed_text());
    }
    transcript_from_active_tail(resources, completed).await
}

async fn transcript_from_active_tail(
    resources: &ProviderResources,
    completed_segments: &[String],
) -> Option<String> {
    let active = resources.sideband().events().take_transcript_tail().await;
    let user_segments = active
        .into_iter()
        .filter(|entry| entry.role == "user")
        .map(|entry| entry.text)
        .collect::<Vec<_>>();
    merge_transcript_segments(&user_segments)
        .or_else(|| merge_transcript_segments(completed_segments))
}

/// Joins the final transcript events emitted for one recording. Frameless
/// Realtime may split a recording at voice-activity boundaries, so one commit
/// can legitimately produce multiple completed input transcript segments.
pub fn merge_transcript_segments(segments: &[String]) -> Option<String> {
    let mut merged = Vec::new();
    let mut previous_key: Option<String> = None;
    for segment in segments {
        let segment = segment.trim();
        let comparison_key = segment
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if comparison_key.is_empty()
            || previous_key
                .as_ref()
                .is_some_and(|previous| previous == &comparison_key)
        {
            continue;
        }
        merged.push(segment);
        previous_key = Some(comparison_key);
    }
    (!merged.is_empty()).then(|| merged.join(" "))
}

fn chatgpt_provider() -> Provider {
    provider_for_base(CHATGPT_CODEX_BASE_URL)
}

fn sideband_provider() -> Provider {
    provider_for_base(OPENAI_V1_BASE_URL)
}

fn provider_for_base(base_url: &str) -> Provider {
    let mut headers = HeaderMap::new();
    headers.insert("version", HeaderValue::from_static("0.0.0"));
    Provider {
        name: "OpenAI".to_string(),
        base_url: base_url.to_string(),
        query_params: None,
        headers,
        retry: RetryConfig {
            max_attempts: 2,
            base_delay: Duration::from_millis(250),
            retry_429: true,
            retry_5xx: true,
            retry_transport: true,
        },
        stream_idle_timeout: Duration::from_secs(45),
    }
}

fn transcription_session(realtime_session_id: &str) -> RealtimeSessionConfig {
    RealtimeSessionConfig {
        instructions: String::new(),
        initial_items: Vec::new(),
        model: Some(REALTIME_MODEL.to_string()),
        session_id: Some(realtime_session_id.to_string()),
        event_parser: RealtimeEventParser::FramelessBidi,
        session_mode: RealtimeSessionMode::Conversational,
        output_modality: RealtimeOutputModality::Audio,
        voice: RealtimeVoice::Cove,
        delegation_ack_filler: None,
    }
}

fn realtime_call_headers(session_id: &str, thread_correlation_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("openai-alpha", HeaderValue::from_static("quicksilver=v2"));
    if let Ok(value) = HeaderValue::from_str(thread_correlation_id) {
        headers.insert("x-session-id", value);
    }
    headers.extend(build_session_headers(
        Some(session_id.to_string()),
        Some(thread_correlation_id.to_string()),
    ));
    default_client::add_originator_header(&mut headers, ORIGINATOR);
    headers
}

fn provider_correlation_id() -> String {
    SessionId::new().to_string()
}

#[async_trait::async_trait]
impl polar_voice::VoiceReadinessProvider for CodexVoiceProvider {
    async fn refresh_readiness(&self) {
        if let Some(vault) = self.vault_auth.as_ref() {
            vault.refresh().await;
        }
    }
    fn readiness_revision(&self) -> u64 {
        if let Some(vault) = self.vault_auth.as_ref() {
            vault.revision.load(std::sync::atomic::Ordering::Acquire)
        } else {
            self.auth_manager
                .as_ref()
                .map(|m| *m.auth_change_receiver().borrow())
                .unwrap_or(0)
        }
    }
    fn readiness(&self) -> polar_voice::VoiceReadiness {
        let usable = if let Some(vault) = self.vault_auth.as_ref() {
            let snapshot = vault.snapshot.read().unwrap();
            snapshot.is_some() && *snapshot != *vault.rejected.read().unwrap()
        } else {
            self.auth_manager.as_ref().is_some_and(|manager| {
                manager.auth_cached().is_some_and(|auth| {
                    auth.uses_codex_backend() && manager.refresh_failure_for_auth(&auth).is_none()
                })
            })
        };
        if usable {
            polar_voice::VoiceReadiness::Ready
        } else {
            polar_voice::VoiceReadiness::AuthRequired
        }
    }
}





struct VaultAuth {
    vault: Arc<polar_vault::PolarVault>,
    scope: String,
    name: String,
    snapshot: std::sync::RwLock<Option<Arc<SnapshotAuth>>>,
    rejected: std::sync::RwLock<Option<Arc<SnapshotAuth>>>,
    revision: std::sync::atomic::AtomicU64,
}
impl VaultAuth {
    async fn refresh(&self) {
        let next = match self
            .vault
            .get_secret_material(&self.scope, &self.name)
            .await
        {
            Ok(Some(raw)) => parse_vault_auth(&zeroize::Zeroizing::new(raw)).map(Arc::new),
            _ => None,
        };
        let mut current = self.snapshot.write().unwrap();
        if *current != next {
            *current = next;
            self.revision
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }
}
fn parse_vault_auth(raw: &str) -> Option<SnapshotAuth> {
    let document: codex_login::AuthDotJson = serde_json::from_str(raw).ok()?;
    let mut tokens = document.tokens?;
    tokens.refresh_token.zeroize();
    if tokens.access_token.trim().is_empty() {
        return None;
    }
    Some(SnapshotAuth {
        token: tokens.access_token,
        account_id: tokens.account_id,
        fedramp: tokens.id_token.is_fedramp_account(),
    })
}

#[derive(PartialEq, Eq)]
struct SnapshotAuth {
    token: String,
    account_id: Option<String>,
    fedramp: bool,
}

impl Drop for SnapshotAuth {
    fn drop(&mut self) {
        self.token.zeroize();
        if let Some(account_id) = self.account_id.as_mut() {
            account_id.zeroize();
        }
    }
}

impl AuthProvider for SnapshotAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.token)) {
            headers.insert(http::header::AUTHORIZATION, value);
        }
        if let Some(account_id) = self.account_id.as_deref() {
            if let Ok(value) = HeaderValue::from_str(account_id) {
                headers.insert("ChatGPT-Account-ID", value);
            }
        }
        if self.fedramp {
            headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
    }
}

fn snapshot_auth_provider(auth: &CodexAuth) -> Result<Arc<SnapshotAuth>, VoiceError> {
    Ok(Arc::new(SnapshotAuth {
        token: auth.get_token().map_err(|_| VoiceError::AuthRequired)?,
        account_id: auth.get_account_id(),
        fedramp: auth.is_fedramp_account(),
    }))
}

fn voice_failure(stage: &str, error: VoiceError) -> VoiceError {
    if std::env::var_os("POLAR_VOICE_STAGE_DIAGNOSTICS").is_some() {
        eprintln!("Polar Voice provider failure stage: {stage}");
    }
    error
}

fn provider_api_failure(
    stage: &str,
    provider_error: &codex_api::ApiError,
    error: VoiceError,
) -> VoiceError {
    if std::env::var_os("POLAR_VOICE_STAGE_DIAGNOSTICS").is_some() {
        let kind = match provider_error {
            codex_api::ApiError::Api { status, .. } => format!("api_status_{status}"),
            codex_api::ApiError::Transport(transport) => match transport {
                codex_http_client::TransportError::Http { status, .. } => {
                    format!("http_status_{status}")
                }
                codex_http_client::TransportError::RetryLimit => "retry_limit".to_string(),
                codex_http_client::TransportError::Timeout => "transport_timeout".to_string(),
                codex_http_client::TransportError::Network(_) => "network".to_string(),
                codex_http_client::TransportError::Connection(_) => "connection".to_string(),
                codex_http_client::TransportError::Build(_) => "request_build".to_string(),
                codex_http_client::TransportError::Policy(_) => "network_policy".to_string(),
                codex_http_client::TransportError::ResponseTooLarge { .. } => {
                    "response_too_large".to_string()
                }
            },
            codex_api::ApiError::Stream(message) if message.contains("403") => {
                "stream_http_403".to_string()
            }
            codex_api::ApiError::Stream(message) if message.contains("401") => {
                "stream_http_401".to_string()
            }
            codex_api::ApiError::Stream(message) if message.contains("TLS") => {
                "stream_tls".to_string()
            }
            codex_api::ApiError::Stream(_) => "stream".to_string(),
            codex_api::ApiError::InvalidRequest { .. } => "invalid_request".to_string(),
            codex_api::ApiError::RateLimit(_) => "rate_limit".to_string(),
            codex_api::ApiError::QuotaExceeded => "quota_exceeded".to_string(),
            codex_api::ApiError::ServerOverloaded { .. } => "server_overloaded".to_string(),
            _ => "provider_error".to_string(),
        };
        eprintln!("Polar Voice provider failure stage: {stage}; kind: {kind}");
    }
    let status = match provider_error {
        codex_api::ApiError::Api { status, .. } => Some(*status),
        codex_api::ApiError::Transport(codex_http_client::TransportError::Http {
            status, ..
        }) => Some(*status),
        _ => None,
    };
    if matches!(
        status,
        Some(http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN)
    ) {
        VoiceError::AuthRequired
    } else {
        error
    }
}

struct ProviderResources {
    auth_scope: Option<Arc<SnapshotAuth>>,
    peer: Option<ServerMediaPeer>,
    sideband: Option<RealtimeWebsocketConnection>,
}

impl ProviderResources {
    fn new(peer: ServerMediaPeer) -> Self {
        Self {
            auth_scope: None,
            peer: Some(peer),
            sideband: None,
        }
    }

    /// Inspect only already-queued protocol events; never wait or poll on a timer.
    async fn prepared_usable(&self, auth: &Arc<SnapshotAuth>) -> bool {
        if self.auth_scope.as_ref()!=Some(auth) || self.peer().peer.connection_state()!=RTCPeerConnectionState::Connected {
            return false;
        }
        loop {
            let event=tokio::select! {
                biased;
                event=self.sideband().next_event()=>Some(event),
                _=std::future::ready(())=>None,
            };
            match event {
                None=>return true,
                Some(Ok(Some(RealtimeEvent::SessionUpdated{..})))=>{},
                _=>return false, // Never carry prior audio/text or an ended session into capture.
            }
        }
    }

    fn peer(&self) -> &ServerMediaPeer {
        match self.peer.as_ref() {
            Some(peer) => peer,
            None => unreachable!("Polar Voice media peer already closed"),
        }
    }

    fn install_sideband(&mut self, sideband: RealtimeWebsocketConnection) {
        self.sideband = Some(sideband);
    }

    fn sideband(&self) -> &RealtimeWebsocketConnection {
        match self.sideband.as_ref() {
            Some(sideband) => sideband,
            None => unreachable!("Polar Voice sideband not installed"),
        }
    }

    async fn close_all(&mut self) {
        if let Some(sideband) = self.sideband.take() {
            let _ = sideband.close().await;
        }
        if let Some(peer) = self.peer.take() {
            let _ = peer.close().await;
        }
    }
}

impl Drop for ProviderResources {
    fn drop(&mut self) {
        let sideband = self.sideband.take();
        let peer = self.peer.take();
        if sideband.is_none() && peer.is_none() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(sideband) = sideband {
                    let _ = sideband.close().await;
                }
                if let Some(peer) = peer {
                    let _ = peer.close().await;
                }
            });
        }
    }
}

struct ServerMediaPeer {
    peer: Arc<webrtc::peer_connection::RTCPeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    connected: Mutex<mpsc::Receiver<()>>,
}

impl ServerMediaPeer {
    async fn new_with_channels(channels: u16) -> Result<Self, webrtc::Error> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)?;
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();
        let peer = Arc::new(api.new_peer_connection(RTCConfiguration::default()).await?);
        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: OPUS_SAMPLE_RATE_HZ,
                channels,
                ..Default::default()
            },
            "polar-voice-audio".to_string(),
            "polar-voice".to_string(),
        ));
        let sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        tokio::spawn(async move {
            let mut buffer = vec![0_u8; 1_500];
            while sender.read(&mut buffer).await.is_ok() {}
        });

        let (connected_tx, connected_rx) = mpsc::channel(1);
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));
        peer.on_peer_connection_state_change(Box::new(move |state| {
            let connected_tx = Arc::clone(&connected_tx);
            Box::pin(async move {
                if state == RTCPeerConnectionState::Connected {
                    if let Some(sender) = connected_tx.lock().await.take() {
                        let _ = sender.send(()).await;
                    }
                }
            })
        }));

        Ok(Self {
            peer,
            track,
            connected: Mutex::new(connected_rx),
        })
    }

    async fn create_offer(&self) -> Result<String, webrtc::Error> {
        let offer = self.peer.create_offer(None).await?;
        let mut gathering_complete = self.peer.gathering_complete_promise().await;
        self.peer.set_local_description(offer).await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), gathering_complete.recv()).await;
        self.peer
            .local_description()
            .await
            .map(|description| description.sdp)
            .ok_or(webrtc::Error::ErrConnectionClosed)
    }

    async fn apply_answer(&self, sdp: String) -> Result<(), webrtc::Error> {
        self.peer
            .set_remote_description(RTCSessionDescription::answer(sdp)?)
            .await
    }

    async fn wait_until_connected(&self) -> Result<(), ()> {
        tokio::time::timeout(Duration::from_secs(10), self.connected.lock().await.recv())
            .await
            .map_err(|_| ())?
            .ok_or(())
    }

    async fn replay_pcm(&self, pcm: &[u8]) -> Result<(), String> {
        let mut frames = pcm16le_to_opus_frames(pcm)?;
        let silence = vec![0_i16; OPUS_SAMPLES_PER_FRAME * 15];
        frames.extend(opus_frames_from_48k_samples(&silence)?);
        self.write_opus_packets(&frames).await
    }

    async fn write_opus_packets(&self, packets: &[Vec<u8>]) -> Result<(), String> {
        for packet in packets {
            self.write_opus_packet(packet).await?;
            tokio::time::sleep(OPUS_FRAME_DURATION).await;
        }
        Ok(())
    }

    async fn write_opus_packet(&self, packet: &[u8]) -> Result<(), String> {
        self.track
            .write_sample(&Sample {
                data: Bytes::copy_from_slice(packet),
                duration: OPUS_FRAME_DURATION,
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn close(&self) -> Result<(), webrtc::Error> {
        self.peer.close().await
    }
}

pub fn upsample_pcm16_16k_to_48k(input: &[i16]) -> Vec<i16> {
    input
        .iter()
        .flat_map(|sample| [*sample, *sample, *sample])
        .collect()
}

pub fn pcm16le_to_opus_frames(pcm: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if !pcm.len().is_multiple_of(2) {
        return Err("unaligned pcm16le".to_string());
    }
    let samples = pcm
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let upsampled = upsample_pcm16_16k_to_48k(&samples);
    opus_frames_from_48k_samples(&upsampled)
}

fn opus_frames_from_48k_samples(samples: &[i16]) -> Result<Vec<Vec<u8>>, String> {
    let mut encoder = Encoder::builder(OPUS_SAMPLE_RATE_HZ, Channels::Mono, Application::Voip)
        .build()
        .map_err(|error| error.to_string())?;
    let mut frames = Vec::with_capacity(samples.len().div_ceil(OPUS_SAMPLES_PER_FRAME));
    for chunk in samples.chunks(OPUS_SAMPLES_PER_FRAME) {
        let mut padded = [0_i16; OPUS_SAMPLES_PER_FRAME];
        padded[..chunk.len()].copy_from_slice(chunk);
        let mut output = [0_u8; 1_275];
        let encoded = encoder
            .encode(&padded, &mut output)
            .map_err(|error| error.to_string())?;
        frames.push(output[..encoded].to_vec());
    }
    Ok(frames)
}


