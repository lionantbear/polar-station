//! Provider-independent batch and streaming speech transcription.
//!
//! The domain accepts either one complete canonical PCM recording or an
//! ordered stream of canonical Opus packets and returns final text. It
//! deliberately has no thread, message, send, draft, or provider-credential
//! concepts.

pub mod buffered;
pub mod realtime;
pub mod transcription;
pub mod activity;

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OnceCell, OwnedSemaphorePermit, Semaphore};
use zeroize::Zeroize;

pub const PCM_ENCODING: &str = "pcm_s16le";
pub const PCM_SAMPLE_RATE_HZ: u32 = 16_000;
pub const PCM_CHANNELS: u8 = 1;
pub const MINIMUM_AUDIO_BYTES: usize = 9_600;
pub const MAXIMUM_AUDIO_BYTES: usize = 960_000;
pub const OPUS_ENCODING: &str = "opus";
pub const OPUS_INPUT_SAMPLE_RATE_HZ: u32 = 16_000;
pub const OPUS_RTP_CLOCK_RATE_HZ: u32 = 48_000;
pub const OPUS_CHANNELS: u8 = 1;
pub const OPUS_FRAME_DURATION_MS: u16 = 20;
pub const OPUS_SAMPLES_PER_PACKET: u16 = 960;
pub const MINIMUM_SPEECH_PACKETS: u32 = 15;
pub const MAXIMUM_SPEECH_PACKETS: u32 = 1_500;
pub const MAXIMUM_OPUS_PACKET_BYTES: usize = 1_275;
/// Stable Polar Voice device transport identifier.
///
/// The version describes the wire contract only; it does not describe the
/// hosting platform.  A Worker, VPS, Polar Host, or another adapter may
/// implement the same transport.
pub const VOICE_TRANSPORT_PROTOCOL_V1: &str = "polar.voice.transport.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceServiceProfile {
    pub id: String,
    pub label_zh: String,
    pub label_en: String,
    pub endpoint: String,
    pub protocol: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub route: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceServiceProfileSet {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_transcribe_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_synthesis_profile_id: Option<String>,
    pub profiles: Vec<VoiceServiceProfile>,
}

impl VoiceServiceProfileSet {
    /// Validates the device-facing profile contract before it is staged into
    /// a connector binding. Provider names and credentials are intentionally
    /// absent from this shared domain type.
    pub fn is_valid(&self) -> bool {
        if self.revision == 0 || self.profiles.is_empty() {
            return false;
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            if !valid_profile_id(&profile.id)
                || profile.protocol != VOICE_TRANSPORT_PROTOCOL_V1
                || profile.endpoint.len() <= 6
                || !profile.endpoint.starts_with("wss://")
                || profile.endpoint[6..].contains(char::is_whitespace)
                || profile.capabilities.is_empty()
                || profile
                    .capabilities
                    .iter()
                    .any(|capability| !matches!(capability.as_str(), "transcribe" | "synthesize"))
                || profile.capabilities.len()
                    != profile
                        .capabilities
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                || self.profiles[..index]
                    .iter()
                    .any(|previous| previous.id == profile.id)
            {
                return false;
            }
        }
        self.default_transcribe_profile_id
            .as_deref()
            .is_none_or(|id| {
                self.profiles.iter().any(|profile| {
                    profile.id == id
                        && profile
                            .capabilities
                            .iter()
                            .any(|capability| capability == "transcribe")
                })
            })
            && self
                .default_synthesis_profile_id
                .as_deref()
                .is_none_or(|id| {
                    self.profiles.iter().any(|profile| {
                        profile.id == id
                            && profile
                                .capabilities
                                .iter()
                                .any(|capability| capability == "synthesize")
                    })
                })
    }
}

fn valid_profile_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && id.len() <= 64
        && chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceTranscribeRequest {
    operation_id: String,
    audio: Vec<u8>,
}

impl VoiceTranscribeRequest {
    pub fn new(operation_id: impl Into<String>, audio: Vec<u8>) -> Self {
        Self {
            operation_id: operation_id.into(),
            audio,
        }
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn audio(&self) -> &[u8] {
        &self.audio
    }
}

impl Drop for VoiceTranscribeRequest {
    fn drop(&mut self) {
        self.audio.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceTranscribeResult {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamFormat {
    pub encoding: String,
    pub input_sample_rate_hz: u32,
    pub channels: u8,
    pub frame_duration_ms: u16,
    pub rtp_clock_rate_hz: u32,
    pub samples_per_packet: u16,
}

impl VoiceStreamFormat {
    pub fn opus_16khz_mono() -> Self {
        Self {
            encoding: OPUS_ENCODING.to_string(),
            input_sample_rate_hz: OPUS_INPUT_SAMPLE_RATE_HZ,
            channels: OPUS_CHANNELS,
            frame_duration_ms: OPUS_FRAME_DURATION_MS,
            rtp_clock_rate_hz: OPUS_RTP_CLOCK_RATE_HZ,
            samples_per_packet: OPUS_SAMPLES_PER_PACKET,
        }
    }

    fn is_canonical(&self) -> bool {
        self == &Self::opus_16khz_mono()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamBeginRequest {
    operation_id: String,
    format: VoiceStreamFormat,
}

impl VoiceStreamBeginRequest {
    pub fn new(operation_id: impl Into<String>, format: VoiceStreamFormat) -> Self {
        Self {
            operation_id: operation_id.into(),
            format,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamBeginResult {
    pub operation_id: String,
    pub next_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamAppendRequest {
    operation_id: String,
    sequence: u32,
    packet: Vec<u8>,
}

impl VoiceStreamAppendRequest {
    pub fn new(operation_id: impl Into<String>, sequence: u32, packet: Vec<u8>) -> Self {
        Self {
            operation_id: operation_id.into(),
            sequence,
            packet,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceOpusPacket {
    pub sequence: u32,
    pub payload: Vec<u8>,
}

impl VoiceOpusPacket {
    pub fn new(sequence: u32, payload: Vec<u8>) -> Self {
        Self { sequence, payload }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamAppendResult {
    pub next_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamCommitRequest {
    operation_id: String,
    packet_count: u32,
    encoded_bytes: usize,
    packet_sha256: String,
}

impl VoiceStreamCommitRequest {
    pub fn new(
        operation_id: impl Into<String>,
        packet_count: u32,
        encoded_bytes: usize,
        packet_sha256: impl Into<String>,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            packet_count,
            encoded_bytes,
            packet_sha256: packet_sha256.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamCommitResult {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamAbortRequest {
    operation_id: String,
}

impl VoiceStreamAbortRequest {
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceStreamAbortResult {
    pub aborted: bool,
}

pub struct VoicePcm {
    bytes: Vec<u8>,
}

impl VoicePcm {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn encoding(&self) -> &'static str {
        PCM_ENCODING
    }

    pub const fn sample_rate_hz(&self) -> u32 {
        PCM_SAMPLE_RATE_HZ
    }

    pub const fn channels(&self) -> u8 {
        PCM_CHANNELS
    }
}

impl Drop for VoicePcm {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceError {
    InvalidAudio,
    InvalidOperation,
    OperationConflict,
    ProviderUnavailable,
    AuthRequired,
    Timeout,
    TestRecordingFailed,
    TranscriptionFailed,
    InvalidStream,
    StreamNotFound,
    StreamIncomplete,
    MediaJitterExceeded,
    TranscriptionInvalid,
}

impl VoiceError {
    /// Whether the same complete input may be retried without user repair.
    /// This says nothing about retaining or discarding the caller's recording.
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::ProviderUnavailable
                | Self::Timeout
                | Self::TranscriptionFailed
                | Self::MediaJitterExceeded
        )
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidAudio => "voice.invalid_audio",
            Self::InvalidOperation => "voice.invalid_operation",
            Self::OperationConflict => "voice.operation_conflict",
            Self::ProviderUnavailable => "voice.provider_unavailable",
            Self::AuthRequired => "voice.auth_required",
            Self::Timeout => "voice.timeout",
            Self::TestRecordingFailed => "voice.test_recording_failed",
            Self::TranscriptionFailed => "voice.transcription_failed",
            Self::InvalidStream => "voice.invalid_stream",
            Self::StreamNotFound => "voice.stream_not_found",
            Self::StreamIncomplete => "voice.stream_incomplete",
            Self::MediaJitterExceeded => "voice.media_jitter_exceeded",
            Self::TranscriptionInvalid => "voice.transcription_invalid",
        }
    }
}

impl fmt::Display for VoiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Error for VoiceError {}

#[async_trait::async_trait]
pub trait VoiceTranscriptionProvider: Send + Sync {
    async fn transcribe(&self, audio: VoicePcm) -> Result<String, VoiceError>;
    /// Packet-aware providers override this to avoid a decode/re-encode round trip.
    /// The default preserves existing PCM-only provider behavior.
    async fn transcribe_opus(
        &self,
        audio: buffered::VoiceOpusRecording,
    ) -> Result<String, VoiceError> {
        self.transcribe(audio.decode()?).await
    }
}

#[async_trait::async_trait]
pub trait VoiceStreamingTranscriptionProvider: Send + Sync {
    /// Optional transport-only preparation. It creates no recording or audio.
    async fn prepare(&self) -> Result<(), VoiceError> { Ok(()) }
    async fn release_prepared(&self) {}
    /// Creates a recording-scoped provider session and returns only after its
    /// media path can accept packets.
    async fn begin(
        &self,
        format: VoiceStreamFormat,
    ) -> Result<Arc<dyn VoiceStreamingSession>, VoiceError>;
}

#[async_trait::async_trait]
pub trait VoiceStreamingSession: Send + Sync {
    async fn append(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError>;
    async fn commit(&self, packet_count: u32) -> Result<String, VoiceError>;
    async fn abort(&self) -> Result<(), VoiceError>;
    /// A cumulative snapshot; observers may skip previews without losing confirmed text.
    fn transcript_updates(&self) -> Option<tokio::sync::watch::Receiver<transcription::TranscriptSnapshot>> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct VoiceRuntimeConfig {
    pub maximum_host_concurrency: usize,
    /// Streaming-only bound; batch PCM limits remain unchanged.
    pub maximum_stream_packets: u32,
    pub provider_timeout: Duration,
    pub idempotent_result_retention: Duration,
    /// Explicit test-only directory for retaining the exact accepted PCM as WAV.
    /// `None` is the privacy-preserving production default.
    pub test_recordings_directory: Option<PathBuf>,
}

impl Default for VoiceRuntimeConfig {
    fn default() -> Self {
        Self {
            maximum_host_concurrency: 8,
            maximum_stream_packets: MAXIMUM_SPEECH_PACKETS,
            provider_timeout: Duration::from_secs(45),
            idempotent_result_retention: Duration::from_secs(60),
            test_recordings_directory: None,
        }
    }
}

struct OperationEntry {
    audio_digest: blake3::Hash,
    created_at: Instant,
    result: OnceCell<Result<VoiceTranscribeResult, VoiceError>>,
}

struct ActiveStream {
    format: VoiceStreamFormat,
    provider_session: Arc<dyn VoiceStreamingSession>,
    state: AsyncMutex<ActiveStreamState>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Default)]
struct ActiveStreamState {
    next_sequence: u32,
    packets: BTreeMap<u32, Vec<u8>>,
    encoded_bytes: usize,
    accepting_packets: bool,
    terminal_error: Option<VoiceError>,
}

pub struct VoiceRuntime {
    provider: Arc<dyn VoiceTranscriptionProvider>,
    streaming_provider: Option<Arc<dyn VoiceStreamingTranscriptionProvider>>,
    config: VoiceRuntimeConfig,
    concurrency: Arc<Semaphore>,
    operations: Mutex<HashMap<String, Arc<OperationEntry>>>,
    streams: AsyncMutex<HashMap<String, Arc<ActiveStream>>>,
}

impl VoiceRuntime {
    pub fn new<P>(provider: Arc<P>, config: VoiceRuntimeConfig) -> Self
    where
        P: VoiceTranscriptionProvider + 'static,
    {
        let maximum_host_concurrency = config.maximum_host_concurrency.max(1);
        Self {
            provider,
            streaming_provider: None,
            config,
            concurrency: Arc::new(Semaphore::new(maximum_host_concurrency)),
            operations: Mutex::new(HashMap::new()),
            streams: AsyncMutex::new(HashMap::new()),
        }
    }

    pub fn with_streaming_provider<P>(mut self, provider: Arc<P>) -> Self
    where
        P: VoiceStreamingTranscriptionProvider + 'static,
    {
        self.streaming_provider = Some(provider);
        self
    }

    pub async fn transcribe(
        &self,
        mut request: VoiceTranscribeRequest,
    ) -> Result<VoiceTranscribeResult, VoiceError> {
        validate_operation_id(request.operation_id())?;
        validate_audio(request.audio())?;

        let operation_id = request.operation_id().to_string();
        let digest = blake3::hash(request.audio());
        let entry = {
            let mut operations = self.operations.lock().expect("voice operations lock");
            let retention = self.config.idempotent_result_retention;
            operations.retain(|_, entry| entry.created_at.elapsed() <= retention);
            match operations.get(&operation_id) {
                Some(entry) if entry.audio_digest != digest => {
                    return Err(VoiceError::OperationConflict)
                }
                Some(entry) => Arc::clone(entry),
                None => {
                    let entry = Arc::new(OperationEntry {
                        audio_digest: digest,
                        created_at: Instant::now(),
                        result: OnceCell::new(),
                    });
                    operations.insert(operation_id.clone(), Arc::clone(&entry));
                    entry
                }
            }
        };

        // Wrap the bytes before any await so cancellation while waiting for a
        // concurrency permit still runs the zeroizing `VoicePcm` destructor.
        let audio = VoicePcm::new(std::mem::take(&mut request.audio));
        entry
            .result
            .get_or_init(|| async move {
                let test_recording = self
                    .config
                    .test_recordings_directory
                    .as_deref()
                    .map(|directory| {
                        retain_test_recording(directory, &operation_id, audio.as_bytes())
                    })
                    .transpose()?;
                let result = async {
                    let _permit = self
                        .concurrency
                        .acquire()
                        .await
                        .map_err(|_| VoiceError::ProviderUnavailable)?;
                    let provider_call = self.provider.transcribe(audio);
                    let text = tokio::time::timeout(self.config.provider_timeout, provider_call)
                        .await
                        .map_err(|_| VoiceError::Timeout)??;
                    let text = normalize_transcript(text)?;
                    Ok(VoiceTranscribeResult { text })
                }
                .await;
                if let Some(test_recording) = test_recording {
                    test_recording.retain_result(&result)?;
                }
                result
            })
            .await
            .clone()
    }

    /// Prepare/release only the optional transport; no operation or audio is admitted.
    pub async fn prepare_streaming(&self, enabled: bool) -> Result<(), VoiceError> {
        if let Some(provider)=&self.streaming_provider {
            if enabled {
                tokio::time::timeout(self.config.provider_timeout,provider.prepare()).await.map_err(|_|VoiceError::Timeout)??;
            } else { provider.release_prepared().await; }
        }
        Ok(())
    }

    pub async fn stream_begin(
        &self,
        request: VoiceStreamBeginRequest,
    ) -> Result<VoiceStreamBeginResult, VoiceError> {
        validate_operation_id(&request.operation_id)?;
        if !request.format.is_canonical() {
            return Err(VoiceError::InvalidStream);
        }
        let provider = self
            .streaming_provider
            .as_ref()
            .ok_or(VoiceError::ProviderUnavailable)?;
        let mut streams = self.streams.lock().await;
        if let Some(existing) = streams.get(&request.operation_id) {
            if existing.format != request.format {
                return Err(VoiceError::OperationConflict);
            }
            let next_sequence = existing.state.lock().await.next_sequence;
            return Ok(VoiceStreamBeginResult {
                operation_id: request.operation_id,
                next_sequence,
            });
        }
        let permit = Arc::clone(&self.concurrency)
            .try_acquire_owned()
            .map_err(|_| VoiceError::ProviderUnavailable)?;
        let provider_session = tokio::time::timeout(
            self.config.provider_timeout,
            provider.begin(request.format.clone()),
        )
        .await
        .map_err(|_| VoiceError::Timeout)??;
        streams.insert(
            request.operation_id.clone(),
            Arc::new(ActiveStream {
                format: request.format,
                provider_session,
                state: AsyncMutex::new(ActiveStreamState {
                    accepting_packets: true,
                    ..ActiveStreamState::default()
                }),
                _permit: permit,
            }),
        );
        Ok(VoiceStreamBeginResult {
            operation_id: request.operation_id,
            next_sequence: 0,
        })
    }

    pub async fn stream_transcript_updates(&self, operation_id: &str) -> Result<Option<tokio::sync::watch::Receiver<transcription::TranscriptSnapshot>>, VoiceError> {
        let streams = self.streams.lock().await;
        let stream = streams.get(operation_id).ok_or(VoiceError::StreamNotFound)?;
        Ok(stream.provider_session.transcript_updates())
    }

    pub async fn stream_append(
        &self,
        request: VoiceStreamAppendRequest,
    ) -> Result<VoiceStreamAppendResult, VoiceError> {
        validate_operation_id(&request.operation_id)?;
        validate_opus_packet_with_limit(request.sequence, &request.packet, self.config.maximum_stream_packets)?;
        let stream = self
            .streams
            .lock()
            .await
            .get(&request.operation_id)
            .cloned()
            .ok_or(VoiceError::StreamNotFound)?;
        let mut state = stream.state.lock().await;
        if let Some(error) = state.terminal_error.as_ref() {
            return Err(error.clone());
        }
        if !state.accepting_packets {
            return Err(VoiceError::InvalidStream);
        }
        let is_new = match state.packets.get(&request.sequence) {
            Some(existing) if existing != &request.packet => {
                return Err(VoiceError::OperationConflict)
            }
            Some(_) => false,
            None => true,
        };
        if is_new && state.packets.len() >= self.config.maximum_stream_packets as usize {
            return Err(VoiceError::InvalidStream);
        }

        let provider_result = tokio::time::timeout(
            self.config.provider_timeout,
            stream.provider_session.append(VoiceOpusPacket::new(
                request.sequence,
                request.packet.clone(),
            )),
        )
        .await
        .map_err(|_| VoiceError::Timeout)
        .and_then(|result| result);
        if let Err(error) = provider_result {
            state.accepting_packets = false;
            state.terminal_error = Some(error.clone());
            return Err(error);
        }
        if is_new {
            state.encoded_bytes = state
                .encoded_bytes
                .checked_add(request.packet.len())
                .ok_or(VoiceError::InvalidStream)?;
            state.packets.insert(request.sequence, request.packet);
            while state.packets.contains_key(&state.next_sequence) {
                state.next_sequence += 1;
            }
        }
        Ok(VoiceStreamAppendResult {
            next_sequence: state.next_sequence,
        })
    }

    /// Finish the exact stream already admitted by this runtime.
    pub async fn stream_finish(&self, operation_id: &str) -> Result<VoiceStreamCommitResult, VoiceError> {
        let stream = self.streams.lock().await.get(operation_id).cloned().ok_or(VoiceError::StreamNotFound)?;
        let request = {
            let state = stream.state.lock().await;
            let packets = state.packets.values().cloned().collect::<Vec<_>>();
            VoiceStreamCommitRequest::new(operation_id, packets.len() as u32, state.encoded_bytes, packet_sha256(&packets))
        };
        self.stream_commit(request).await
    }

    pub async fn stream_commit(
        &self,
        request: VoiceStreamCommitRequest,
    ) -> Result<VoiceStreamCommitResult, VoiceError> {
        validate_operation_id(&request.operation_id)?;
        let stream = self
            .streams
            .lock()
            .await
            .get(&request.operation_id)
            .cloned()
            .ok_or(VoiceError::StreamNotFound)?;
        let retained_packets = {
            let mut state = stream.state.lock().await;
            if let Some(error) = state.terminal_error.as_ref() {
                return Err(error.clone());
            }
            validate_stream_commit_with_limit(&request, &state, self.config.maximum_stream_packets)?;
            state.accepting_packets = false;
            state.packets.values().cloned().collect::<Vec<_>>()
        };
        let retained_recording = self
            .config
            .test_recordings_directory
            .as_deref()
            .map(|directory| {
                retain_test_stream(directory, &request.operation_id, &retained_packets)
            })
            .transpose()?;
        let provider_result = tokio::time::timeout(
            self.config.provider_timeout,
            stream.provider_session.commit(request.packet_count),
        )
        .await;
        let provider_result = match provider_result {
            Ok(result) => result,
            Err(_) => {
                let _ = stream.provider_session.abort().await;
                Err(VoiceError::Timeout)
            }
        };
        self.streams.lock().await.remove(&request.operation_id);
        let result = provider_result.and_then(|text| {
            normalize_transcript(text).map(|text| VoiceStreamCommitResult { text })
        });
        if let Some(retained_recording) = retained_recording {
            retained_recording.retain_result(&result)?;
        }
        result
    }

    pub async fn stream_abort(
        &self,
        request: VoiceStreamAbortRequest,
    ) -> Result<VoiceStreamAbortResult, VoiceError> {
        validate_operation_id(&request.operation_id)?;
        let stream = self
            .streams
            .lock()
            .await
            .remove(&request.operation_id)
            .ok_or(VoiceError::StreamNotFound)?;
        let provider_is_terminal = {
            let mut state = stream.state.lock().await;
            state.accepting_packets = false;
            state.terminal_error.is_some()
        };
        if !provider_is_terminal {
            tokio::time::timeout(
                self.config.provider_timeout,
                stream.provider_session.abort(),
            )
            .await
            .map_err(|_| VoiceError::Timeout)??;
        }
        Ok(VoiceStreamAbortResult { aborted: true })
    }
}

fn validate_opus_packet(sequence: u32, packet: &[u8]) -> Result<(), VoiceError> {
    validate_opus_packet_with_limit(sequence, packet, MAXIMUM_SPEECH_PACKETS)
}

fn validate_opus_packet_with_limit(sequence: u32, packet: &[u8], maximum: u32) -> Result<(), VoiceError> {
    if sequence >= maximum
        || packet.is_empty()
        || packet.len() > MAXIMUM_OPUS_PACKET_BYTES
    {
        return Err(VoiceError::InvalidStream);
    }
    if ropus::opus::decoder::opus_packet_get_nb_samples(
        packet,
        i32::try_from(OPUS_RTP_CLOCK_RATE_HZ).expect("Opus RTP clock fits i32"),
    ) != Ok(i32::from(OPUS_SAMPLES_PER_PACKET))
    {
        return Err(VoiceError::InvalidStream);
    }
    Ok(())
}

fn next_contiguous_sequence(packets: &BTreeMap<u32, Vec<u8>>) -> u32 {
    let mut next = 0_u32;
    while packets.contains_key(&next) {
        next += 1;
    }
    next
}

fn validate_stream_commit_with_limit(request: &VoiceStreamCommitRequest, state: &ActiveStreamState, maximum: u32) -> Result<(), VoiceError> {
    if !state.accepting_packets
        || !(MINIMUM_SPEECH_PACKETS..=maximum).contains(&request.packet_count)
    {
        return Err(VoiceError::InvalidStream);
    }
    if usize::try_from(request.packet_count).ok() != Some(state.packets.len())
        || next_contiguous_sequence(&state.packets) != request.packet_count
    {
        return Err(VoiceError::StreamIncomplete);
    }
    let packets = state.packets.values().cloned().collect::<Vec<_>>();
    if request.encoded_bytes != state.encoded_bytes
        || request.packet_sha256 != packet_sha256(&packets)
    {
        return Err(VoiceError::InvalidStream);
    }
    Ok(())
}

fn packet_sha256(packets: &[Vec<u8>]) -> String {
    let mut digest = Sha256::new();
    for (sequence, packet) in packets.iter().enumerate() {
        digest.update(
            u32::try_from(sequence)
                .expect("validated stream sequence fits u32")
                .to_be_bytes(),
        );
        digest.update(
            u16::try_from(packet.len())
                .expect("validated Opus packet length fits u16")
                .to_be_bytes(),
        );
        digest.update(packet);
    }
    format!("{:x}", digest.finalize())
}

struct RetainedTestStream {
    packet_path: PathBuf,
    decoded_wav_path: PathBuf,
    trace_path: PathBuf,
    operation_id: String,
    packet_sha256: String,
    packet_count: usize,
    encoded_bytes: usize,
    accepted_at_unix_ms: u128,
}

impl RetainedTestStream {
    fn retain_result(
        &self,
        result: &Result<VoiceStreamCommitResult, VoiceError>,
    ) -> Result<(), VoiceError> {
        let completed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let (transcript, error_code) = match result {
            Ok(result) => (Some(result.text.as_str()), None),
            Err(error) => (None, Some(error.code())),
        };
        let document = serde_json::json!({
            "schema": "polar.voice.stream-test-trace.v1",
            "operationId": self.operation_id,
            "packetFile": self.packet_path.file_name().and_then(|name| name.to_str()),
            "decodedWavFile": self.decoded_wav_path.file_name().and_then(|name| name.to_str()),
            "packetSha256": self.packet_sha256,
            "packetCount": self.packet_count,
            "devicePacketCount": self.packet_count,
            "encodedBytes": self.encoded_bytes,
            "acceptedAtUnixMs": self.accepted_at_unix_ms,
            "completedAtUnixMs": completed_at_unix_ms,
            "transcript": transcript,
            "errorCode": error_code,
        });
        write_private_new_file(
            &self.trace_path,
            &serde_json::to_vec_pretty(&document).map_err(|_| VoiceError::TestRecordingFailed)?,
        )
    }
}

fn retain_test_stream(
    directory: &Path,
    operation_id: &str,
    packets: &[Vec<u8>],
) -> Result<RetainedTestStream, VoiceError> {
    prepare_private_directory(directory)?;
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let digest = packet_sha256(packets);
    let stem = format!(
        "{timestamp_ms}-{}-{}-{}p",
        sanitize_operation_id(operation_id),
        &digest[..8],
        packets.len(),
    );
    let packet_path = directory.join(format!("{stem}.opuspack"));
    let decoded_wav_path = directory.join(format!("{stem}.decoded.wav"));
    let trace_path = directory.join(format!("{stem}.trace.json"));

    let mut packet_bytes =
        Vec::with_capacity(packets.iter().map(Vec::len).sum::<usize>() + packets.len() * 6);
    for (sequence, packet) in packets.iter().enumerate() {
        packet_bytes.extend_from_slice(
            &u32::try_from(sequence)
                .map_err(|_| VoiceError::TestRecordingFailed)?
                .to_be_bytes(),
        );
        packet_bytes.extend_from_slice(
            &u16::try_from(packet.len())
                .map_err(|_| VoiceError::TestRecordingFailed)?
                .to_be_bytes(),
        );
        packet_bytes.extend_from_slice(packet);
    }
    write_private_new_file(&packet_path, &packet_bytes)?;

    let decoded = decode_opus_packets(packets)?;
    let mut decoded_wav = Vec::with_capacity(44 + decoded.len() * 2);
    decoded_wav.extend_from_slice(&wav_header_for(
        decoded.len() * 2,
        OPUS_RTP_CLOCK_RATE_HZ,
        OPUS_CHANNELS,
    ));
    for sample in decoded {
        decoded_wav.extend_from_slice(&sample.to_le_bytes());
    }
    if let Err(error) = write_private_new_file(&decoded_wav_path, &decoded_wav) {
        let _ = fs::remove_file(&packet_path);
        return Err(error);
    }

    Ok(RetainedTestStream {
        packet_path,
        decoded_wav_path,
        trace_path,
        operation_id: operation_id.to_string(),
        packet_sha256: digest,
        packet_count: packets.len(),
        encoded_bytes: packets.iter().map(Vec::len).sum(),
        accepted_at_unix_ms: timestamp_ms,
    })
}

fn decode_opus_packets(packets: &[Vec<u8>]) -> Result<Vec<i16>, VoiceError> {
    let mut decoder = ropus::Decoder::new(OPUS_RTP_CLOCK_RATE_HZ, ropus::Channels::Mono)
        .map_err(|_| VoiceError::TestRecordingFailed)?;
    let mut decoded = Vec::with_capacity(packets.len() * usize::from(OPUS_SAMPLES_PER_PACKET));
    for packet in packets {
        let mut frame = [0_i16; OPUS_SAMPLES_PER_PACKET as usize];
        let samples = decoder
            .decode(packet, &mut frame, ropus::DecodeMode::Normal)
            .map_err(|_| VoiceError::TestRecordingFailed)?;
        if samples != usize::from(OPUS_SAMPLES_PER_PACKET) {
            return Err(VoiceError::TestRecordingFailed);
        }
        decoded.extend_from_slice(&frame);
    }
    Ok(decoded)
}

fn prepare_private_directory(directory: &Path) -> Result<(), VoiceError> {
    if !directory.is_absolute() {
        return Err(VoiceError::TestRecordingFailed);
    }
    fs::create_dir_all(directory).map_err(|_| VoiceError::TestRecordingFailed)?;
    if !fs::metadata(directory)
        .map_err(|_| VoiceError::TestRecordingFailed)?
        .is_dir()
    {
        return Err(VoiceError::TestRecordingFailed);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| VoiceError::TestRecordingFailed)?;
    }
    Ok(())
}

fn write_private_new_file(path: &Path, bytes: &[u8]) -> Result<(), VoiceError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
        return Err(VoiceError::TestRecordingFailed);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| VoiceError::TestRecordingFailed)?;
    }
    Ok(())
}

struct RetainedTestRecording {
    wav_path: PathBuf,
    trace_path: PathBuf,
    operation_id: String,
    pcm_blake3: String,
    pcm_bytes: usize,
    duration_ms: u64,
    accepted_at_unix_ms: u128,
}

impl RetainedTestRecording {
    fn retain_result(
        &self,
        result: &Result<VoiceTranscribeResult, VoiceError>,
    ) -> Result<(), VoiceError> {
        let completed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let (transcript, error_code) = match result {
            Ok(result) => (Some(result.text.as_str()), None),
            Err(error) => (None, Some(error.code())),
        };
        let document = serde_json::json!({
            "schema": "polar.voice.test-trace.v1",
            "operationId": self.operation_id,
            "wavFile": self.wav_path.file_name().and_then(|name| name.to_str()),
            "pcmBlake3": self.pcm_blake3,
            "pcmBytes": self.pcm_bytes,
            "durationMs": self.duration_ms,
            "acceptedAtUnixMs": self.accepted_at_unix_ms,
            "completedAtUnixMs": completed_at_unix_ms,
            "transcript": transcript,
            "errorCode": error_code,
        });
        let encoded =
            serde_json::to_vec_pretty(&document).map_err(|_| VoiceError::TestRecordingFailed)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let write_result = (|| -> std::io::Result<()> {
            let mut file = options.open(&self.trace_path)?;
            file.write_all(&encoded)?;
            file.sync_all()
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&self.trace_path);
            let _ = fs::remove_file(&self.wav_path);
            return Err(VoiceError::TestRecordingFailed);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.trace_path, fs::Permissions::from_mode(0o600))
                .map_err(|_| VoiceError::TestRecordingFailed)?;
        }
        Ok(())
    }
}

fn retain_test_recording(
    directory: &Path,
    operation_id: &str,
    pcm: &[u8],
) -> Result<RetainedTestRecording, VoiceError> {
    if !directory.is_absolute() {
        return Err(VoiceError::TestRecordingFailed);
    }
    fs::create_dir_all(directory).map_err(|_| VoiceError::TestRecordingFailed)?;
    let metadata = fs::metadata(directory).map_err(|_| VoiceError::TestRecordingFailed)?;
    if !metadata.is_dir() {
        return Err(VoiceError::TestRecordingFailed);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| VoiceError::TestRecordingFailed)?;
    }

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sanitized_operation = sanitize_operation_id(operation_id);
    let operation_hash = blake3::hash(operation_id.as_bytes()).to_hex().to_string();
    let pcm_hash = blake3::hash(pcm).to_hex().to_string();
    let duration_ms = (pcm.len() as u64 / 2) * 1_000 / u64::from(PCM_SAMPLE_RATE_HZ);
    let filename = format!(
        "{timestamp_ms}-{sanitized_operation}-{}-{}-{}b-{duration_ms}ms.wav",
        &operation_hash[..8],
        &pcm_hash[..8],
        pcm.len(),
    );
    let path = directory.join(filename);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| VoiceError::TestRecordingFailed)?;
    let write_result = (|| -> std::io::Result<()> {
        file.write_all(&wav_header(pcm.len()))?;
        file.write_all(pcm)?;
        file.sync_all()
    })();
    if write_result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(VoiceError::TestRecordingFailed);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| VoiceError::TestRecordingFailed)?;
    }
    Ok(RetainedTestRecording {
        trace_path: path.with_extension("trace.json"),
        wav_path: path,
        operation_id: operation_id.to_string(),
        pcm_blake3: pcm_hash,
        pcm_bytes: pcm.len(),
        duration_ms,
        accepted_at_unix_ms: timestamp_ms,
    })
}

fn sanitize_operation_id(operation_id: &str) -> String {
    let sanitized = operation_id
        .chars()
        .take(48)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "operation".to_string()
    } else {
        sanitized
    }
}

fn wav_header(pcm_bytes: usize) -> [u8; 44] {
    wav_header_for(pcm_bytes, PCM_SAMPLE_RATE_HZ, PCM_CHANNELS)
}

fn wav_header_for(pcm_bytes: usize, sample_rate_hz: u32, channels: u8) -> [u8; 44] {
    let data_size = u32::try_from(pcm_bytes).expect("validated voice PCM fits a WAV data chunk");
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_size).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&u16::from(channels).to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate_hz.to_le_bytes());
    let byte_rate = sample_rate_hz * u32::from(channels) * 2;
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&(u16::from(channels) * 2).to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    header
}

fn validate_operation_id(operation_id: &str) -> Result<(), VoiceError> {
    let operation_id = operation_id.trim();
    if operation_id.is_empty() || operation_id.len() > 128 {
        return Err(VoiceError::InvalidOperation);
    }
    Ok(())
}

fn validate_audio(audio: &[u8]) -> Result<(), VoiceError> {
    if audio.len() < MINIMUM_AUDIO_BYTES
        || audio.len() > MAXIMUM_AUDIO_BYTES
        || !audio.len().is_multiple_of(2)
    {
        return Err(VoiceError::InvalidAudio);
    }
    Ok(())
}

fn normalize_transcript(text: String) -> Result<String, VoiceError> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(VoiceError::TranscriptionFailed);
    }
    if text.chars().any(|character| {
        character == '\u{fffd}' || (character.is_control() && !character.is_whitespace())
    }) {
        return Err(VoiceError::TranscriptionInvalid);
    }
    Ok(text)
}

/// Credential readiness, independent of installed capabilities or remote reachability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceReadiness {
    Ready,
    AuthRequired,
    Unavailable,
}

/// Nonblocking snapshot of the provider's existing authentication owner.
#[async_trait::async_trait]
pub trait VoiceReadinessProvider: Send + Sync {
    /// Refresh local credential metadata only; never perform provider network I/O.
    async fn refresh_readiness(&self) {}
    fn readiness(&self) -> VoiceReadiness;
    fn readiness_revision(&self) -> u64 {
        0
    }
}


