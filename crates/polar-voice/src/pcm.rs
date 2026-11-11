//! PCM input facade. Codec state and prepared packets never escape Voice.
use crate::{
    buffered::VoiceOpusRecording, transcription::PcmOpusEncoder, VoiceError, VoiceRuntime,
    VoiceStreamAbortRequest, VoiceStreamAppendRequest, VoiceStreamBeginRequest, VoiceStreamFormat,
    VoiceTranscribeRequest, VoiceTranscribeResult, TranscriptionReference,
};
use std::sync::Arc;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmTranscriptionMode {
    Buffered,
    Incremental,
}

/// Ephemeral audio resource, not a recording database or transcription queue.
pub struct PcmTranscriptionSession {
    runtime: Arc<VoiceRuntime>,
    operation_id: String,
    reference: TranscriptionReference,
    mode: PcmTranscriptionMode,
    encoder: PcmOpusEncoder,
    pcm: Zeroizing<Vec<u8>>,
    packets: Vec<Zeroizing<Vec<u8>>>,
    start_sample: u64,
    through_sample: u64,
    closed: bool,
}

/// A sealed immutable audio request; execution uses VoiceRuntime's existing
/// idempotency and concurrency admission. Creating this does not contact ASR.
pub struct PcmTranscriptionJob {
    runtime: Arc<VoiceRuntime>,
    operation_id: String,
    reference: TranscriptionReference,
    audio: Option<(Zeroizing<Vec<u8>>, VoiceOpusRecording)>,
    stream: Option<String>,
}
impl PcmTranscriptionJob {
    pub async fn wait(mut self) -> Result<VoiceTranscribeResult, VoiceError> {
        if let Some(stream) = self.stream.clone() {
            let result = self.runtime.stream_finish(&stream).await;
            let _ = self
                .runtime
                .stream_abort(VoiceStreamAbortRequest::new(&stream))
                .await;
            self.stream = None;
            return result.map(|r| VoiceTranscribeResult { text: r.text });
        }
        let (mut pcm, opus) = self.audio.take().ok_or(VoiceError::InvalidAudio)?;
        let padded = pcm.len().max(crate::MINIMUM_AUDIO_BYTES);
        pcm.resize(padded, 0);
        self.runtime
            .transcribe_inner(
                VoiceTranscribeRequest::new(self.operation_id.clone(), std::mem::take(&mut *pcm)).with_reference(self.reference.clone()),
                Some(opus),
            )
            .await
    }
}
fn abort_dropped_stream(runtime: &Arc<VoiceRuntime>, operation_id: &str) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let runtime = runtime.clone();
        let request = VoiceStreamAbortRequest::new(operation_id);
        handle.spawn(async move { let _ = runtime.stream_abort(request).await; });
    }
}
impl Drop for PcmTranscriptionJob {
    fn drop(&mut self) {
        if let Some(stream) = &self.stream { abort_dropped_stream(&self.runtime, stream); }
    }
}
impl Drop for PcmTranscriptionSession {
    fn drop(&mut self) {
        if self.mode == PcmTranscriptionMode::Incremental && !self.closed {
            abort_dropped_stream(&self.runtime, &self.operation_id);
        }
    }
}
impl VoiceRuntime {
    pub async fn open_pcm(
        self: &Arc<Self>,
        operation_id: impl Into<String>,
        mode: PcmTranscriptionMode,
        start_sample: u64,
    ) -> Result<PcmTranscriptionSession, VoiceError> {
        self.open_pcm_with_reference(operation_id, mode, start_sample, TranscriptionReference::default()).await
    }
    pub async fn open_pcm_with_reference(
        self: &Arc<Self>, operation_id: impl Into<String>, mode: PcmTranscriptionMode,
        start_sample: u64, reference: TranscriptionReference,
    ) -> Result<PcmTranscriptionSession, VoiceError> {
        let operation_id = operation_id.into();
        crate::validate_operation_id(&operation_id)?;
        reference.require_support(self.transcription_reference_support(mode))?;
        let encoder = PcmOpusEncoder::new()?;
        if mode == PcmTranscriptionMode::Incremental {
            self.stream_begin(VoiceStreamBeginRequest::new(
                &operation_id,
                VoiceStreamFormat::opus_16khz_mono(),
            ).with_reference(reference.clone()))
            .await?;
        }
        Ok(PcmTranscriptionSession {
            runtime: self.clone(),
            operation_id,
            reference,
            mode,
            encoder,
            pcm: Zeroizing::new(Vec::new()),
            packets: Vec::new(),
            start_sample,
            through_sample: start_sample,
            closed: false,
        })
    }
}
impl PcmTranscriptionSession {
    pub fn through_sample(&self) -> u64 {
        self.through_sample
    }
    pub async fn subscribe(
        &self,
    ) -> Result<
        Option<tokio::sync::watch::Receiver<crate::transcription::TranscriptSnapshot>>,
        VoiceError,
    > {
        if self.mode == PcmTranscriptionMode::Incremental {
            self.runtime
                .stream_transcript_updates(&self.operation_id)
                .await
        } else {
            Ok(None)
        }
    }
    pub async fn append_pcm(&mut self, pcm: &[u8]) -> Result<(), VoiceError> {
        if self.closed || pcm.is_empty() || !pcm.len().is_multiple_of(2) {
            return Err(VoiceError::InvalidAudio);
        }
        if self.mode == PcmTranscriptionMode::Buffered
            && self.pcm.len() + pcm.len() > self.runtime.provider.maximum_audio_bytes()
        {
            return Err(VoiceError::InvalidAudio);
        }
        let through_sample = self
            .through_sample
            .checked_add(pcm.len() as u64 / 2)
            .ok_or(VoiceError::InvalidAudio)?;
        let packets = self.encoder.append(pcm, false)?;
        if self.mode == PcmTranscriptionMode::Buffered {
            self.pcm.extend_from_slice(pcm);
            self.packets
                .extend(packets.into_iter().map(|p| Zeroizing::new(p.payload)));
        } else {
            for packet in packets {
                self.runtime
                    .stream_append(VoiceStreamAppendRequest::new(
                        &self.operation_id,
                        packet.sequence,
                        packet.payload,
                    ))
                    .await?;
            }
        }
        self.through_sample = through_sample;
        Ok(())
    }
    /// Normal range boundaries are complete20ms frames. A boundary may precede
    /// the latest appended sample: only the retained tail is encoded again.
    pub fn seal_prefix(
        &mut self,
        operation_id: impl Into<String>,
        through_sample: u64,
    ) -> Result<PcmTranscriptionJob, VoiceError> {
        if self.mode != PcmTranscriptionMode::Buffered
            || self.closed
            || through_sample <= self.start_sample
            || through_sample > self.through_sample
            || !(through_sample - self.start_sample).is_multiple_of(320)
        {
            return Err(VoiceError::InvalidAudio);
        }
        self.seal(operation_id.into(), through_sample, false)
    }
    fn seal(
        &mut self,
        operation_id: String,
        through_sample: u64,
        final_input: bool,
    ) -> Result<PcmTranscriptionJob, VoiceError> {
        crate::validate_operation_id(&operation_id)?;
        let bytes = usize::try_from((through_sample - self.start_sample) * 2)
            .map_err(|_| VoiceError::InvalidAudio)?;
        if bytes == 0 || bytes > self.pcm.len() {
            return Err(VoiceError::InvalidAudio);
        }
        if final_input {
            self.packets.extend(
                self.encoder
                    .append(&[], true)?
                    .into_iter()
                    .map(|p| Zeroizing::new(p.payload)),
            );
        }
        let count = (bytes / 2)
            .div_ceil(320)
            .max(crate::MINIMUM_SPEECH_PACKETS as usize);
        if count > self.packets.len() {
            return Err(VoiceError::InvalidAudio);
        }
        let original = Zeroizing::new(self.pcm[..bytes].to_vec());
        let packets = self.packets.drain(..count).collect();
        let remaining = Zeroizing::new(self.pcm[bytes..].to_vec());
        self.encoder = PcmOpusEncoder::new()?;
        self.packets = self
            .encoder
            .append(&remaining, false)?
            .into_iter()
            .map(|p| Zeroizing::new(p.payload))
            .collect();
        self.pcm = remaining;
        self.start_sample = through_sample;
        Ok(PcmTranscriptionJob {
            runtime: self.runtime.clone(),
            operation_id,
            reference: self.reference.clone(),
            audio: Some((original, VoiceOpusRecording::from_packets(packets))),
            stream: None,
        })
    }
    pub async fn finish_input(
        &mut self,
        operation_id: impl Into<String>,
    ) -> Result<PcmTranscriptionJob, VoiceError> {
        if self.closed {
            return Err(VoiceError::InvalidStream);
        }
        if self.through_sample == self.start_sample {
            return Err(VoiceError::InvalidAudio);
        }
        let operation_id = operation_id.into();
        let result = if self.mode == PcmTranscriptionMode::Buffered {
            self.seal(operation_id, self.through_sample, true)
        } else {
            for packet in self.encoder.append(&[], true)? {
                self.runtime
                    .stream_append(VoiceStreamAppendRequest::new(
                        &self.operation_id,
                        packet.sequence,
                        packet.payload,
                    ))
                    .await?;
            }
            Ok(PcmTranscriptionJob {
                runtime: self.runtime.clone(),
                operation_id,
                reference: self.reference.clone(),
                audio: None,
                stream: Some(self.operation_id.clone()),
            })
        };
        if result.is_ok() {
            self.closed = true;
        }
        result
    }
    pub async fn abort(&mut self) -> Result<(), VoiceError> {
        self.closed = true;
        self.pcm.clear();
        self.packets.clear();
        if self.mode == PcmTranscriptionMode::Incremental {
            self.runtime
                .stream_abort(VoiceStreamAbortRequest::new(&self.operation_id))
                .await?;
        }
        Ok(())
    }
}
