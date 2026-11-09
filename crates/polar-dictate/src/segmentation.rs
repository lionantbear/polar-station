//! Pause boundaries over shared speech activity. Audio storage remains independent.
use polar_voice::activity::SpeechActivityDetector;

#[derive(Default)]
pub(crate) struct PauseSegmenter {
    detector: SpeechActivityDetector,
    pending: Vec<i16>,
    through: u64,
    segment_start: u64,
    silent_frames: u32,
    heard_speech: bool,
}
impl PauseSegmenter {
    pub(crate) fn append(&mut self, start_sample: u64, pcm: &[u8]) -> Vec<u64> {
        let expected = self.through + self.pending.len() as u64;
        if start_sample < expected { return Vec::new(); }
        if start_sample != expected {
            // A restored stream starts a new detector at its actual audio offset.
            *self = Self { through: start_sample, segment_start: start_sample, ..Default::default() };
        }
        self.pending.extend(pcm.chunks_exact(2).map(|v| i16::from_le_bytes([v[0], v[1]])));
        let count = self.pending.len() / 320 * 320;
        let samples = self.pending.drain(..count).collect::<Vec<_>>();
        let mut boundaries = Vec::new();
        for frame in samples.chunks_exact(320) {
            let speech = self.detector.is_speech(frame.try_into().expect("one canonical frame"));
            if let Some(boundary) = self.observe(speech) { boundaries.push(boundary); }
        }
        boundaries
    }
    fn observe(&mut self, speech: bool) -> Option<u64> {
        self.through += 320;
        self.heard_speech |= speech;
        self.silent_frames = if speech { 0 } else { self.silent_frames + 1 };
        let length = self.through - self.segment_start;
        if length >= crate::SEGMENT_SAMPLES || (self.heard_speech && length >= 32_000 && self.silent_frames >= 30) {
            self.segment_start = self.through;
            self.silent_frames = 0;
            self.heard_speech = false;
            Some(self.through)
        } else { None }
    }
}

