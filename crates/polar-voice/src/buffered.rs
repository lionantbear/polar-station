//! Adapts a batch transcription provider (such as Dictate) to the existing
//! Voice stream lifecycle. Packet integrity/idempotency remains in VoiceRuntime.
use crate::{
    VoiceError, VoiceOpusPacket, VoicePcm, VoiceStreamFormat, VoiceStreamingSession,
    VoiceStreamingTranscriptionProvider, VoiceTranscriptionProvider,
};
use async_trait::async_trait;
use ropus::{Channels, DecodeMode, Decoder};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

/// Complete, sequence-checked recording. Construction belongs to the existing
/// stream adapter; providers may package the original packets without decoding.
pub struct VoiceOpusRecording {
    packets: Vec<Zeroizing<Vec<u8>>>,
}
impl VoiceOpusRecording {
    pub fn packets(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.packets.iter().map(|packet| packet.as_slice())
    }
    pub fn duration_ms(&self) -> u32 {
        self.packets.len() as u32 * 20
    }
    pub(crate) fn decode(&self) -> Result<VoicePcm, VoiceError> {
        let mut decoder =
            Decoder::new(16_000, Channels::Mono).map_err(|_| VoiceError::InvalidAudio)?;
        let mut pcm = Zeroizing::new(Vec::with_capacity(self.packets.len() * 640));
        let mut samples = Zeroizing::new([0i16; 320]);
        for packet in self.packets() {
            let count = decoder
                .decode(packet, samples.as_mut(), DecodeMode::Normal)
                .map_err(|_| VoiceError::InvalidAudio)?;
            if count != 320 {
                return Err(VoiceError::InvalidAudio);
            }
            for sample in samples.iter() {
                pcm.extend_from_slice(&sample.to_le_bytes());
            }
        }
        Ok(VoicePcm::new(std::mem::take(&mut *pcm)))
    }
}

pub struct BufferedTranscriptionProvider(pub Arc<dyn VoiceTranscriptionProvider>);
struct BufferedSession {
    provider: Arc<dyn VoiceTranscriptionProvider>,
    packets: Mutex<Option<BTreeMap<u32, Zeroizing<Vec<u8>>>>>,
}
#[async_trait]
impl VoiceStreamingTranscriptionProvider for BufferedTranscriptionProvider {
    async fn begin(
        &self,
        format: VoiceStreamFormat,
    ) -> Result<Arc<dyn VoiceStreamingSession>, VoiceError> {
        if format != VoiceStreamFormat::opus_16khz_mono() {
            return Err(VoiceError::InvalidStream);
        }
        Ok(Arc::new(BufferedSession {
            provider: self.0.clone(),
            packets: Mutex::new(Some(BTreeMap::new())),
        }))
    }
}
#[async_trait]
impl VoiceStreamingSession for BufferedSession {
    async fn append(&self, packet: VoiceOpusPacket) -> Result<(), VoiceError> {
        crate::validate_opus_packet(packet.sequence, &packet.payload)?;
        let mut slot = self.packets.lock().await;
        let packets = slot.as_mut().ok_or(VoiceError::StreamNotFound)?;
        if let Some(old) = packets.get(&packet.sequence) {
            if old.as_slice() != packet.payload {
                return Err(VoiceError::OperationConflict);
            }
            return Ok(());
        }
        packets.insert(packet.sequence, Zeroizing::new(packet.payload));
        Ok(())
    }
    async fn commit(&self, packet_count: u32) -> Result<String, VoiceError> {
        let packets = self
            .packets
            .lock()
            .await
            .take()
            .ok_or(VoiceError::StreamNotFound)?;
        if !(crate::MINIMUM_SPEECH_PACKETS..=crate::MAXIMUM_SPEECH_PACKETS).contains(&packet_count)
            || packets.len() != packet_count as usize
            || packets.keys().copied().ne(0..packet_count)
        {
            return Err(VoiceError::StreamIncomplete);
        }
        self.provider
            .transcribe_opus(VoiceOpusRecording {
                packets: packets.into_values().collect(),
            })
            .await
    }
    async fn abort(&self) -> Result<(), VoiceError> {
        self.packets.lock().await.take();
        Ok(())
    }
}


