//! Codex-backed adapter for the provider-independent Polar Voice domain.
//!
//! This is the only Polar Voice crate allowed to know Codex authentication or
//! provider-private Realtime/WebRTC details. Batch transcription, packet playout
//! and streaming lifecycle have separate modules behind the shared Voice contracts.

pub mod dictate;
pub mod realtime;
mod jitter;
mod streaming;
pub use jitter::{ProviderJitterBuffer, ProviderJitterStats, JitterPlayout, OPUS_SILENCE_PACKET};
pub use streaming::merge_transcript_segments;
use streaming::wait_for_transcript;


use std::{
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use codex_api::{
    build_session_headers, AuthError, AuthProvider, Provider, RealtimeCallClient, RealtimeEventParser,
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
    VoiceError, VoicePcm, VoiceTranscriptionProvider,
};
use ropus::{Application, Channels, Encoder};
use tokio::sync::{Mutex,mpsc};
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
            sideband_headers.extend(auth_provider.to_auth_headers().map_err(|_| VoiceError::AuthRequired)?);
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
    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), AuthError> {
        let token = HeaderValue::from_str(&format!("Bearer {}", self.token))
            .map_err(|_| AuthError::Build("invalid bearer header".into()))?;
        let account = self.account_id.as_deref().map(HeaderValue::from_str).transpose()
            .map_err(|_| AuthError::Build("invalid account header".into()))?;
        headers.insert(http::header::AUTHORIZATION, token);
        if let Some(account) = account {
            headers.insert("ChatGPT-Account-ID", account);
        }
        if self.fedramp {
            headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
        Ok(())
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


