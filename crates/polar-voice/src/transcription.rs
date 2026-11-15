//! Cumulative transcription observations. Recording, editing and delivery belong
//! to callers. A final segment replaces its provisional text; it is not a delta.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSnapshot {
    #[serde(default)]
    pub speech_pending: bool,
    pub confirmed: Vec<String>,
    pub partial: String,
}

impl TranscriptSnapshot {
    pub fn append_delta(&mut self, delta: &str) {
        self.speech_pending = true;
        self.partial.push_str(delta);
    }

    pub fn finish_segment(&mut self, text: String) {
        self.speech_pending = false;
        self.partial.clear();
        if !text.trim().is_empty() {
            self.confirmed.push(text);
        }
    }

    pub fn confirmed_text(&self) -> String {
        clean_transcript_spacing(&self.confirmed.join(" "))
    }
}

/// Normalize ASR spacing without rewriting words or invoking a language model.
/// Only ASCII spaces are collapsed; other whitespace remains provider-owned.
pub fn clean_transcript_spacing(text: &str) -> String {
    fn han(c: char) -> bool {
        matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{2ffff}')
    }
    let mut result = String::with_capacity(text.len());
    let mut space = false;
    let mut previous = None;
    for c in text.trim().chars() {
        if c == ' ' { space = true; continue; }
        if space && previous.is_some_and(|left| !(han(left) && han(c))) {
            result.push(' ');
        }
        result.push(c);
        previous = Some(c);
        space = false;
    }
    result
}

/// Canonical 16 kHz PCM to 20 ms Opus packets. Keeps codec state across saved
/// blocks; the final incomplete frame is padded only when input is finished.
pub struct PcmOpusEncoder {
    encoder: ropus::Encoder,
    pending: Vec<i16>,
    sequence: u32,
}
impl PcmOpusEncoder {
    pub fn new() -> Result<Self, crate::VoiceError> {
        let encoder = ropus::Encoder::builder(16_000, ropus::Channels::Mono, ropus::Application::Voip)
            .build().map_err(|_| crate::VoiceError::InvalidAudio)?;
        Ok(Self { encoder, pending: Vec::new(), sequence: 0 })
    }
    pub fn append(&mut self, pcm: &[u8], finish: bool) -> Result<Vec<crate::VoiceOpusPacket>, crate::VoiceError> {
        if !pcm.len().is_multiple_of(2) { return Err(crate::VoiceError::InvalidAudio); }
        self.pending.extend(pcm.chunks_exact(2).map(|v| i16::from_le_bytes([v[0], v[1]])));
        if finish && !self.pending.is_empty() {
            self.pending.resize(self.pending.len().div_ceil(320) * 320, 0);
        }
        if finish && self.sequence < crate::MINIMUM_SPEECH_PACKETS {
            let minimum = (crate::MINIMUM_SPEECH_PACKETS - self.sequence) as usize * 320;
            self.pending.resize(self.pending.len().max(minimum), 0);
        }
        let count = self.pending.len() / 320 * 320;
        let mut packets = Vec::new();
        for frame in self.pending[..count].chunks_exact(320) {
            let mut encoded = [0; crate::MAXIMUM_OPUS_PACKET_BYTES];
            let size = self.encoder.encode(frame, &mut encoded).map_err(|_| crate::VoiceError::InvalidAudio)?;
            packets.push(crate::VoiceOpusPacket::new(self.sequence, encoded[..size].to_vec()));
            self.sequence += 1;
        }
        self.pending.drain(..count);
        Ok(packets)
    }
}


