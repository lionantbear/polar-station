//! ChatGPT Dictate adapter derived from the pinned dictate-lab experiment.
//! Authentication is shared with the existing Codex provider. This adapter is
//! explicitly selected; it does not replace or fall back from existing ASR.

use std::{sync::Arc, time::Duration};

use codex_api::AuthProvider;
use codex_http_client::ClientRouteClass;
use codex_login::default_client;
use http::{header::CONTENT_TYPE, HeaderValue};
use polar_voice::{VoiceError, VoicePcm, VoiceTranscriptionProvider};
use reqwest::multipart::{Form, Part};
use zeroize::Zeroizing;

use crate::{provider_correlation_id, CodexVoiceProvider};

const URL: &str = "https://chatgpt.com/backend-api/transcribe";
const MAX_WEBM_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub struct CodexDictateProvider {
    credentials: Arc<CodexVoiceProvider>,
    client: tokio::sync::OnceCell<codex_http_client::HttpClient>,
}

impl CodexDictateProvider {
    pub fn new(credentials: Arc<CodexVoiceProvider>) -> Self {
        Self { credentials, client: tokio::sync::OnceCell::new() }
    }

    async fn http_client(&self) -> Result<codex_http_client::HttpClient, VoiceError> {
        // Reuse the upstream route-aware connection pool, never authentication.
        // A failed construction is retried; credentials remain per-request.
        self.client.get_or_try_init(|| async {
            default_client::create_client_for_route(
                &self.credentials.http_client_factory, URL, ClientRouteClass::Api,
                default_client::ClientRedirectPolicy::Reject,
            ).map_err(|_| VoiceError::ProviderUnavailable)
        }).await.cloned()
    }

    /// A complete, bounded recording; this creates no thread or chat message.
    pub async fn transcribe_webm(
        &self,
        webm: Vec<u8>,
        duration_ms: u32,
    ) -> Result<String, VoiceError> {
        if webm.len() < 4
            || webm.len() > MAX_WEBM_BYTES
            || webm[..4] != [0x1a, 0x45, 0xdf, 0xa3]
            || !(300..=30_000).contains(&duration_ms)
        {
            return Err(VoiceError::InvalidAudio);
        }
        let started = std::time::Instant::now();
        let credentials = self.credentials.request_auth().await?;
        let auth_us = started.elapsed().as_micros() as u64;
        let client = self.http_client().await?;
        let part = Part::bytes(webm)
            .file_name("codex.webm")
            .mime_str("audio/webm;codecs=opus")
            .map_err(|_| VoiceError::InvalidAudio)?;
        let form = Form::new()
            .part("file", part)
            .text("dictation_session_id", provider_correlation_id())
            .text("attempt_id", provider_correlation_id())
            .text("duration_ms", duration_ms.to_string());
        let mut headers = credentials.to_auth_headers();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&format!(
                "multipart/form-data; boundary={}",
                form.boundary()
            ))
            .map_err(|_| VoiceError::InvalidAudio)?,
        );
        default_client::add_originator_header(&mut headers, "polar_voice");
        let mut response = client
            .post(URL)
            .headers(headers)
            .body_stream(form.into_stream())
            .timeout(Duration::from_secs(90))
            .send()
            .await
            .map_err(|_| VoiceError::ProviderUnavailable)?;
        if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
            self.credentials.reject_auth(credentials);
            return Err(VoiceError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(VoiceError::TranscriptionFailed);
        }
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| VoiceError::TranscriptionFailed)?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(VoiceError::TranscriptionInvalid);
            }
            body.extend_from_slice(&chunk);
        }
        let result = parse_transcript(&body);
        tracing::info!(target: "polar_dictate::latency", event="dictate.transcription",
            work_us=started.elapsed().as_micros() as u64, wait_us=auth_us,
            failed=result.is_err(), "Dictate processing stage");
        result
    }
}

fn parse_transcript(body: &[u8]) -> Result<String, VoiceError> {
    let response: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| VoiceError::TranscriptionInvalid)?;
    let text = response
        .get("text")
        .and_then(|value| value.as_str())
        .filter(|text| !text.trim().is_empty())
        .ok_or(VoiceError::TranscriptionInvalid)?;
    Ok(text.to_owned())
}

/// libwebm owns container serialization. This sink bounds output growth even
/// for maximum-size valid packets; intermediate compressed audio is zeroed.
#[derive(Default)]
struct WebmSink(Zeroizing<Vec<u8>>);
impl std::io::Write for WebmSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_WEBM_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("WebM recording exceeds limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn opus_to_webm<'a>(
    packets: impl ExactSizeIterator<Item = &'a [u8]>,
) -> Result<Vec<u8>, VoiceError> {
    use webm::mux::{AudioCodecId, SegmentBuilder, SegmentMode, Writer};
    let count = packets.len();
    if !(polar_voice::MINIMUM_SPEECH_PACKETS as usize
        ..=polar_voice::MAXIMUM_SPEECH_PACKETS as usize)
        .contains(&count)
    {
        return Err(VoiceError::InvalidAudio);
    }
    let builder = SegmentBuilder::new(Writer::new_non_seek(WebmSink::default()))
        .map_err(|_| VoiceError::InvalidAudio)?
        .set_mode(SegmentMode::Live)
        .map_err(|_| VoiceError::InvalidAudio)?;
    // Opus decoding clock is 48 kHz; original capture rate is informational in
    // OpusHead. No pre-skip is invented: the existing wire has no delay metadata.
    let (builder, track) = builder
        .add_audio_track(48_000, 1, AudioCodecId::Opus, None)
        .map_err(|_| VoiceError::InvalidAudio)?;
    let mut head = Vec::from(&b"OpusHead"[..]);
    head.extend_from_slice(&[1, 1]); // version, mono, channel mapping family 0
    head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&16_000u32.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    let mut segment = builder
        .set_codec_private(track, &head)
        .map_err(|_| VoiceError::InvalidAudio)?
        .build();
    for (index, packet) in packets.enumerate() {
        if packet.is_empty()
            || packet.len() > polar_voice::MAXIMUM_OPUS_PACKET_BYTES
            || ropus::opus::decoder::opus_packet_get_nb_samples(packet, 48_000) != Ok(960)
        {
            return Err(VoiceError::InvalidAudio);
        }
        segment
            .add_frame(track, packet, index as u64 * 20_000_000, true)
            .map_err(|_| VoiceError::InvalidAudio)?;
    }
    let mut sink = segment
        .finalize(Some(count as u64 * 20_000_000))
        .map_err(|_| VoiceError::InvalidAudio)?
        .into_inner();
    Ok(std::mem::take(&mut *sink.0))
}

/// Reuse the in-process codec and muxer already used by the voice provider.
/// No filesystem, external encoder or shell environment is required.
async fn pcm_to_webm(pcm: &[u8]) -> Result<Vec<u8>, VoiceError> {
    if pcm.len() < polar_voice::MINIMUM_AUDIO_BYTES
        || pcm.len() > polar_voice::MAXIMUM_AUDIO_BYTES
        || !pcm.len().is_multiple_of(2)
    {
        return Err(VoiceError::InvalidAudio);
    }
    let pcm = Zeroizing::new(pcm.to_vec());
    tokio::task::spawn_blocking(move || {
        let packets = Zeroizing::new(crate::pcm16le_to_opus_frames(&pcm)
            .map_err(|_| VoiceError::InvalidAudio)?);
        opus_to_webm(packets.iter().map(Vec::as_slice))
    }).await.map_err(|_| VoiceError::TranscriptionFailed)?
}

#[async_trait::async_trait]
impl VoiceTranscriptionProvider for CodexDictateProvider {
    async fn transcribe_opus(
        &self,
        audio: polar_voice::buffered::VoiceOpusRecording,
    ) -> Result<String, VoiceError> {
        let duration_ms = audio.duration_ms();
        let webm = tokio::task::spawn_blocking(move || opus_to_webm(audio.packets()))
            .await
            .map_err(|_| VoiceError::TranscriptionFailed)??;
        self.transcribe_webm(webm, duration_ms).await
    }

    async fn transcribe(&self, audio: VoicePcm) -> Result<String, VoiceError> {
        let duration_ms = (audio.as_bytes().len() / 32) as u32;
        let webm = pcm_to_webm(audio.as_bytes()).await?;
        self.transcribe_webm(webm, duration_ms).await
    }
}


