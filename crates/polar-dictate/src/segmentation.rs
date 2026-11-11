//! Long-form boundaries; saving audio never commits a transcription request.
use polar_voice::activity::SpeechActivityDetector;

#[derive(Default)]
pub(crate) struct PauseSegmenter {
    detector: SpeechActivityDetector,
    pending: Vec<i16>,
    through: u64,
    segment_start: u64,
    silence_start: Option<u64>,
    best_pause: Option<(u64, u64)>,
}
impl PauseSegmenter {
    pub(crate) fn append(&mut self, start_sample: u64, pcm: &[u8]) -> Vec<u64> {
        let expected = self.through + self.pending.len() as u64;
        if start_sample < expected { return Vec::new(); }
        if start_sample != expected {
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
        // A gap is a candidate only once speech resumes. Retain its full audio.
        if speech {
            if let Some(start) = self.silence_start.take() {
                let end = self.through;
                let length = end - start;
                if start >= self.segment_start + 30 * crate::SAMPLE_RATE
                    && self.best_pause.is_none_or(|(_, best)| length > best) {
                    self.best_pause = Some((end, length));
                }
            }
        } else { self.silence_start.get_or_insert(self.through); }
        self.through += 320;
        if self.through - self.segment_start < crate::SEGMENT_SAMPLES { return None; }
        let end = self.best_pause.take().map_or(self.through, |(end, _)| end);
        self.segment_start = end;
        // An open gap remains a candidate for the next range only if it starts there.
        if self.silence_start.is_some_and(|start| start < end) { self.silence_start = None; }
        Some(end)
    }
}

