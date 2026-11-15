//! Thin adapter over the already-pinned SILK VAD. No recording segmentation
//! policy or platform-specific microphone code belongs here.
use ropus::silk::encoder::{silk_vad_get_sa_q8, silk_vad_init, SilkEncoderState};

pub struct SpeechActivityDetector {
    state: SilkEncoderState,
}
impl Default for SpeechActivityDetector {
    fn default() -> Self {
        let mut state = SilkEncoderState { fs_khz: 16, frame_length: 320, ..Default::default() };
        silk_vad_init(&mut state.s_vad);
        Self { state }
    }
}
impl SpeechActivityDetector {
    /// One 20 ms mono frame. Threshold is the pinned SILK DTX threshold (0.05 Q8).
    pub fn is_speech(&mut self, frame: &[i16; 320]) -> bool {
        silk_vad_get_sa_q8(&mut self.state, frame);
        self.state.speech_activity_q8 >= 13
    }
}

