use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SAMPLE_RATE: u64 = 16_000;
pub const MAX_RECORDING_SAMPLES: u64 = SAMPLE_RATE * 600;
pub const SEGMENT_SAMPLES: u64 = SAMPLE_RATE * 30;
pub const MAX_BLOCK_SAMPLES: u64 = SAMPLE_RATE;
pub const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Recording,
    Stopped,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentState {
    Pending,
    Running,
    Complete,
    Failed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    #[default]
    Pending,
    Prepared,
    Delivered,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub index: u32,
    pub start_sample: u64,
    pub sample_count: u64,
    pub attempt: u32,
    pub state: SegmentState,
    pub original: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recording {
    /// Ephemeral event projection; never stored or treated as confirmed text.
    #[serde(skip)]
    pub live_preview: Option<String>,
    #[serde(default)]
    pub confirmed_transcript: Vec<String>,
    #[serde(default)]
    pub cleaned_prefix_parts: usize,
    #[serde(default)]
    pub cleaned_prefix: String,
    #[serde(default = "crate::TranscriptionMode::legacy_recording")]
    pub transcription_mode: crate::TranscriptionMode,
    #[serde(default)]
    pub glossary: String,
    pub id: String,
    pub created_at_ms: u64,
    pub revision: u64,
    pub capture: CaptureState,
    pub saved_samples: u64,
    pub next_sequence: u64,
    pub processing_enabled: bool,
    pub segments: Vec<Segment>,
    pub cleaned: Option<String>,
    pub cleanup_error: Option<String>,
    pub cleanup_enabled: bool,
    pub context_enabled: bool,
    #[serde(default)]
    #[serde(skip)]
    pub context: Option<String>,
    #[serde(default)]
    pub context_digest: Option<String>,
    pub cleanup_attempt: u64,
    pub cleanup_running: bool,
    #[serde(default)]
    pub cleanup_deferred: bool,
    pub delivery: DeliveryState,
    pub delivery_token: Option<String>,
    pub completed_at_ms: Option<u64>,
    pub pinned: bool,
    pub audio_expired: bool,
    pub is_test: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioBlock {
    pub recording_id: String,
    pub sequence: u64,
    pub start_sample: u64,
    pub pcm: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveReceipt {
    #[serde(default)]
    pub transcript_preview: String,
    pub recording_id: String,
    pub revision: u64,
    pub next_sequence: u64,
    pub saved_through_sample: u64,
    pub capture_stopped: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    RetrySegmented,
    SealSegment { through_sample: u64 },
    ConfirmTranscript { parts: Vec<String> },
    CleanTranscriptPrefix { source: Vec<String>, text: String },
    PartialCleanupFailed { error: String },
    Configure {
        cleanup: bool,
        context: Option<String>,
    },
    Stop {
        reason: Option<String>,
    },
    ResumeProcessing,
    CancelProcessing,
    SegmentStarted {
        index: u32,
    },
    SegmentFinished {
        index: u32,
        attempt: u32,
        text: Option<String>,
        error: Option<String>,
    },
    BindContext {
        context: String,
    },
    CleanupStarted,
    CleanupFinished {
        attempt: u64,
        text: Option<String>,
        error: Option<String>,
    },
    UseOriginal,
    PrepareDelivery {
        token: String,
        expected_revision: u64,
    },
    CompleteDelivery {
        token: String,
        applied: Option<bool>,
        now_ms: u64,
    },
    Accept {
        now_ms: u64,
    },
    Pin {
        pinned: bool,
    },
    Recover,
    ExpireAudio {
        now_ms: u64,
    },
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:./".contains(&b))
}
fn bounded_error(error: Option<String>) -> Option<String> {
    error.map(|s| s.chars().take(512).collect())
}
impl Recording {
    pub fn new(id: String, created_at_ms: u64, is_test: bool) -> Result<Self> {
        ensure!(identifier(&id), "dictate.invalid_id");
        Ok(Self {
            live_preview: None,
            confirmed_transcript: Vec::new(),
            cleaned_prefix_parts: 0,
            cleaned_prefix: String::new(),
            transcription_mode: crate::TranscriptionMode::Segmented,
            glossary: String::new(),
            id,
            created_at_ms,
            revision: 1,
            capture: CaptureState::Recording,
            saved_samples: 0,
            next_sequence: 0,
            processing_enabled: true,
            segments: vec![],
            cleaned: None,
            cleanup_error: None,
            cleanup_enabled: true,
            context_enabled: true,
            context: None,
            context_digest: None,
            cleanup_attempt: 0,
            cleanup_running: false,
            cleanup_deferred: false,
            delivery: DeliveryState::Pending,
            delivery_token: None,
            completed_at_ms: None,
            pinned: false,
            audio_expired: false,
            is_test,
            error: None,
        })
    }
    pub fn can_retry_segmented(&self) -> bool {
        self.transcription_mode == crate::TranscriptionMode::Realtime
            && self.capture != CaptureState::Recording && self.saved_samples > 0
            && !self.audio_expired && self.delivery == DeliveryState::Pending && self.original().is_none()
    }
    pub fn original(&self) -> Option<String> {
        if self.capture == CaptureState::Recording
            || self.segments.is_empty()
            || self
                .segments
                .iter()
                .any(|s| s.state != SegmentState::Complete)
        {
            return None;
        }
        Some(
            self.segments
                .iter()
                .filter_map(|s| s.original.as_deref())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
    pub fn text(&self) -> Option<String> {
        self.cleaned.clone().or_else(|| self.original())
    }
    /// Recovery only arms a future explicit editor selection. It cannot replay
    /// an uncertain/completed delivery or move a test into the ordinary router.
    pub fn validate_saved_input(&self) -> Result<()> {
        ensure!(
            !self.is_test,
            "Test recordings cannot enter ordinary inputs"
        );
        ensure!(
            self.delivery == DeliveryState::Pending,
            "This recording has already been delivered or requires manual review"
        );
        ensure!(
            self.original().is_some_and(|text| !text.trim().is_empty())
                && !self.cleanup_running
                && (!self.cleanup_enabled
                    || self.cleanup_deferred
                    || self.cleaned.is_some()
                    || self.cleanup_error.is_some()),
            "Finish transcription and cleanup, or choose Use Original first"
        );
        Ok(())
    }
    pub fn preview_complete(&self) -> bool {
        !self.processing_enabled || self.error.is_some() ||
            (self.capture != CaptureState::Recording && (self.saved_samples == 0 || self.original().is_some()))
    }
    pub fn receipt(&self) -> SaveReceipt {
        SaveReceipt {
            transcript_preview: self.live_preview.clone().unwrap_or_else(|| if self.cleaned_prefix.is_empty() { self.confirmed_transcript.join(" ") }
                else { std::iter::once(self.cleaned_prefix.clone()).chain(self.confirmed_transcript[self.cleaned_prefix_parts..].iter().cloned()).collect::<Vec<_>>().join(" ") }),
            recording_id: self.id.clone(),
            revision: self.revision,
            next_sequence: self.next_sequence,
            saved_through_sample: self.saved_samples,
            capture_stopped: self.capture != CaptureState::Recording,
        }
    }
    pub fn audio_can_expire(&self, now_ms: u64) -> bool {
        !self.pinned
            && !self.audio_expired
            && self
                .completed_at_ms
                .is_some_and(|completed| now_ms.saturating_sub(completed) >= RETENTION_MS)
            && self.delivery != DeliveryState::Unknown
            && self.original().is_some()
    }
    fn advance(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("dictate.revision_exhausted"))?;
        Ok(())
    }
    fn segments_through_saved(&mut self) {
        // Realtime owns one continuous audio range. Provider utterances are text
        // boundaries, not invented audio timestamps. Recovery can replay this range.
        if self.transcription_mode == crate::TranscriptionMode::Realtime {
            if self.capture != CaptureState::Recording && self.saved_samples > 0 && self.segments.is_empty() {
                self.segments.push(Segment { index: 0, start_sample: 0, sample_count: self.saved_samples,
                    attempt: 0, state: SegmentState::Pending, original: None, error: None });
            }
            return;
        }
        let mut start = self
            .segments
            .last()
            .map_or(0, |s| s.start_sample + s.sample_count);
        while start < self.saved_samples {
            let remaining = self.saved_samples - start;
            if remaining < SEGMENT_SAMPLES && self.capture == CaptureState::Recording {
                break;
            }
            let count = remaining.min(SEGMENT_SAMPLES);
            self.segments.push(Segment {
                index: self.segments.len() as u32,
                start_sample: start,
                sample_count: count,
                attempt: 0,
                state: SegmentState::Pending,
                original: None,
                error: None,
            });
            start += count;
        }
    }
    pub fn append(&mut self, block: &AudioBlock) -> Result<()> {
        ensure!(
            self.capture == CaptureState::Recording && !self.audio_expired,
            "dictate.capture_closed"
        );
        ensure!(
            block.recording_id == self.id
                && block.sequence == self.next_sequence
                && block.start_sample == self.saved_samples,
            "dictate.block_out_of_order"
        );
        ensure!(
            !block.pcm.is_empty()
                && block.pcm.len().is_multiple_of(2)
                && block.pcm.len() as u64 <= MAX_BLOCK_SAMPLES * 2,
            "dictate.invalid_audio_block"
        );
        let samples = block.pcm.len() as u64 / 2;
        ensure!(
            self.saved_samples + samples <= MAX_RECORDING_SAMPLES,
            "dictate.recording_limit"
        );
        self.saved_samples += samples;
        self.next_sequence += 1;
        if self.saved_samples == MAX_RECORDING_SAMPLES {
            self.capture = CaptureState::Stopped;
        }
        self.segments_through_saved();
        self.advance()
    }
    pub fn apply(&mut self, change: Change) -> Result<()> {
        match change {
            Change::RetrySegmented => {
                ensure!(self.can_retry_segmented(), "dictate.segmented_retry_unavailable");
                self.transcription_mode = crate::TranscriptionMode::Segmented;
                self.segments.clear();
                self.segments_through_saved();
                self.cleaned = None;
                self.cleaned_prefix.clear();
                self.cleaned_prefix_parts = 0;
                self.cleanup_running = false;
                self.cleanup_error = None;
                self.error = None;
                self.processing_enabled = true;
            }
            Change::SealSegment { through_sample } => {
                ensure!(self.transcription_mode == crate::TranscriptionMode::Segmented && through_sample <= self.saved_samples,
                    "dictate.invalid_segment_boundary");
                let start = self.segments.last().map_or(0, |s| s.start_sample + s.sample_count);
                // A hard-limit segment or Stop may already cover this boundary.
                if through_sample > start {
                    ensure!(through_sample - start <= SEGMENT_SAMPLES, "dictate.segment_limit");
                    self.segments.push(Segment { index: self.segments.len() as u32, start_sample: start,
                        sample_count: through_sample - start, attempt: 0, state: SegmentState::Pending, original: None, error: None });
                }
            }
            Change::ConfirmTranscript { parts } => {
                ensure!(self.processing_enabled, "dictate.processing_cancelled");
                ensure!(parts.iter().map(String::len).sum::<usize>() + parts.len() <= MAX_TEXT_BYTES
                    && parts.iter().all(|s| !s.contains('\0')), "dictate.transcript_limit");
                if parts.get(..self.cleaned_prefix_parts) != self.confirmed_transcript.get(..self.cleaned_prefix_parts) {
                    self.cleaned_prefix_parts = 0;
                    self.cleaned_prefix.clear();
                }
                self.confirmed_transcript = parts;
            }
            Change::CleanTranscriptPrefix { source, text } => {
                ensure!(self.processing_enabled && self.cleanup_enabled && self.delivery == DeliveryState::Pending,
                    "dictate.processing_cancelled");
                ensure!(!source.is_empty() && source.len() >= self.cleaned_prefix_parts
                    && self.confirmed_transcript.get(..source.len()) == Some(source.as_slice()), "dictate.stale_cleanup_result");
                ensure!(!text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES && !text.contains('\0'), "dictate.invalid_cleanup");
                self.cleaned_prefix_parts = source.len();
                self.cleaned_prefix = text;
            }
            Change::PartialCleanupFailed { error } => {
                ensure!(self.processing_enabled, "dictate.processing_cancelled");
                self.cleanup_error = bounded_error(Some(error));
            }
            Change::Configure { cleanup, context } => {
                ensure!(
                    self.saved_samples == 0 && self.capture == CaptureState::Recording,
                    "dictate.already_started"
                );
                ensure!(
                    context
                        .as_ref()
                        .is_none_or(|s| s.len() <= 16 * 1024 && !s.contains('\0')),
                    "dictate.context_limit"
                );
                self.cleanup_enabled = cleanup;
                self.cleanup_deferred = cleanup && context.is_none() && !self.is_test;
                self.context_enabled = context.is_some();
                self.context_digest = context
                    .as_ref()
                    .map(|value| format!("{:x}", Sha256::digest(value.as_bytes())));
                self.context = context;
            }
            Change::Stop { reason } => {
                if self.capture != CaptureState::Recording {
                    return Ok(());
                }
                self.capture = if reason.is_some() {
                    CaptureState::Interrupted
                } else {
                    CaptureState::Stopped
                };
                self.error = bounded_error(reason);
                self.segments_through_saved();
            }
            Change::ResumeProcessing => {
                ensure!(!self.audio_expired, "dictate.audio_expired");
                self.processing_enabled = true;
                self.error = None;
                self.cleanup_error = None;
                for segment in &mut self.segments {
                    if segment.state == SegmentState::Failed {
                        segment.state = SegmentState::Pending;
                        segment.error = None;
                    }
                }
            }
            Change::CancelProcessing => {
                self.processing_enabled = false;
                self.cleanup_running = false;
                self.cleanup_attempt = self
                    .cleanup_attempt
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                for segment in &mut self.segments {
                    if segment.state == SegmentState::Running {
                        segment.state = SegmentState::Pending;
                        segment.attempt = segment
                            .attempt
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                    }
                }
            }
            Change::SegmentStarted { index } => {
                ensure!(self.processing_enabled, "dictate.processing_paused");
                let segment = self
                    .segments
                    .get_mut(index as usize)
                    .ok_or_else(|| anyhow::anyhow!("dictate.segment_missing"))?;
                ensure!(
                    segment.state == SegmentState::Pending,
                    "dictate.segment_not_pending"
                );
                segment.attempt = segment
                    .attempt
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                segment.state = SegmentState::Running;
            }
            Change::SegmentFinished {
                index,
                attempt,
                text,
                error,
            } => {
                let separators = self.segments.len().saturating_sub(1);
                let total: usize = self
                    .segments
                    .iter()
                    .filter_map(|s| s.original.as_ref())
                    .map(String::len)
                    .sum();
                let segment = self
                    .segments
                    .get_mut(index as usize)
                    .ok_or_else(|| anyhow::anyhow!("dictate.segment_missing"))?;
                ensure!(
                    segment.state == SegmentState::Running && segment.attempt == attempt,
                    "dictate.stale_segment_result"
                );
                match text.filter(|s| {
                    !s.trim().is_empty()
                        && !s.contains('\0')
                        && total + separators + s.len() <= MAX_TEXT_BYTES
                }) {
                    Some(text) if error.is_none() => {
                        segment.original = Some(text);
                        segment.state = SegmentState::Complete;
                        segment.error = None;
                    }
                    _ => {
                        segment.state = SegmentState::Failed;
                        segment.error = bounded_error(Some(
                            error.unwrap_or_else(|| "No usable transcription".into()),
                        ));
                        self.processing_enabled = false;
                    }
                }
            }
            Change::BindContext { context } => {
                ensure!(
                    self.cleanup_deferred
                        && self.original().is_some()
                        && self.delivery == DeliveryState::Pending
                        && context.len() <= 16 * 1024
                        && !context.contains('\0'),
                    "dictate.context_already_bound_or_invalid"
                );
                self.context_digest = Some(format!("{:x}", Sha256::digest(context.as_bytes())));
                self.context_enabled = !context.is_empty();
                self.cleanup_deferred = false;
                self.processing_enabled = true;
            }
            Change::CleanupStarted => {
                ensure!(
                    self.processing_enabled
                        && self.cleanup_enabled
                        && !self.cleanup_deferred
                        && self.original().is_some()
                        && !self.cleanup_running
                        && self.cleaned.is_none()
                        && self.delivery == DeliveryState::Pending,
                    "dictate.cleanup_not_ready"
                );
                self.cleanup_attempt = self
                    .cleanup_attempt
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                self.cleanup_running = true;
            }
            Change::CleanupFinished {
                attempt,
                text,
                error,
            } => {
                ensure!(
                    self.cleanup_running && self.cleanup_attempt == attempt,
                    "dictate.stale_cleanup_result"
                );
                self.cleanup_running = false;
                self.cleanup_error = bounded_error(error);
                self.cleaned = text.filter(|_| self.cleanup_error.is_none()).filter(|s| {
                    !s.trim().is_empty() && s.len() <= MAX_TEXT_BYTES && !s.contains('\0')
                });
                if self.cleaned.is_none() && self.cleanup_error.is_none() {
                    self.cleanup_error =
                        Some("Cleanup returned no usable text; original retained".into());
                }
            }
            Change::UseOriginal => {
                ensure!(
                    self.original().is_some() && self.delivery == DeliveryState::Pending,
                    "dictate.original_not_ready"
                );
                self.cleanup_running = false;
                self.cleanup_attempt = self
                    .cleanup_attempt
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                self.cleaned = None;
                self.cleanup_enabled = false;
                self.cleanup_deferred = false;
            }
            Change::PrepareDelivery {
                token,
                expected_revision,
            } => {
                ensure!(
                    identifier(&token) && self.revision == expected_revision,
                    "dictate.delivery_revision_conflict"
                );
                ensure!(
                    self.original().is_some()
                        && !self.cleanup_running
                        && (!self.cleanup_enabled
                            || self.cleaned.is_some()
                            || self.cleanup_error.is_some())
                        && self.delivery == DeliveryState::Pending,
                    "dictate.delivery_not_ready"
                );
                self.delivery = DeliveryState::Prepared;
                self.delivery_token = Some(token);
            }
            Change::CompleteDelivery {
                token,
                applied,
                now_ms,
            } => {
                ensure!(
                    self.delivery == DeliveryState::Prepared
                        && self.delivery_token.as_deref() == Some(&token),
                    "dictate.delivery_conflict"
                );
                self.delivery = match applied {
                    Some(true) => DeliveryState::Delivered,
                    Some(false) => DeliveryState::Pending,
                    None => DeliveryState::Unknown,
                };
                if applied == Some(true) {
                    self.completed_at_ms = Some(now_ms);
                }
                if applied == Some(false) {
                    self.delivery_token = None;
                }
            }
            Change::Accept { now_ms } => {
                ensure!(
                    self.original().is_some()
                        && !self.cleanup_running
                        && (!self.cleanup_enabled
                            || self.cleaned.is_some()
                            || self.cleanup_error.is_some())
                        && self.delivery == DeliveryState::Pending,
                    "dictate.accept_not_ready"
                );
                self.completed_at_ms.get_or_insert(now_ms);
            }
            Change::Pin { pinned } => self.pinned = pinned,
            Change::Recover => {
                if self.capture == CaptureState::Recording {
                    self.capture = CaptureState::Interrupted;
                    self.error = Some("Recording interrupted; saved audio is recoverable".into());
                }
                self.processing_enabled = false;
                self.cleanup_running = false;
                self.cleanup_attempt = self
                    .cleanup_attempt
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                for segment in &mut self.segments {
                    if segment.state == SegmentState::Running {
                        segment.state = SegmentState::Pending;
                        segment.attempt = segment
                            .attempt
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("dictate.attempt_exhausted"))?;
                    }
                }
                self.segments_through_saved();
                if self.delivery == DeliveryState::Prepared {
                    self.delivery = DeliveryState::Unknown;
                }
            }
            Change::ExpireAudio { now_ms } => {
                if !self.audio_can_expire(now_ms) {
                    bail!("dictate.audio_retained");
                }
                self.audio_expired = true;
            }
        }
        self.advance()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingSummary {
    #[serde(default)]
    pub retry_segmented_label: Option<String>,
    pub id: String,
    pub created_at_ms: u64,
    pub revision: u64,
    pub capture: CaptureState,
    pub saved_samples: u64,
    pub segments_complete: usize,
    pub segments_total: usize,
    pub delivery: DeliveryState,
    pub pinned: bool,
    pub audio_expired: bool,
    pub is_test: bool,
    pub completed_at_ms: Option<u64>,
    pub needs_attention: bool,
}
impl From<Recording> for RecordingSummary {
    fn from(r: Recording) -> Self {
        Self {
            retry_segmented_label: r.can_retry_segmented().then(|| "Retry using segmented transcription".into()),
            id: r.id,
            created_at_ms: r.created_at_ms,
            revision: r.revision,
            capture: r.capture,
            saved_samples: r.saved_samples,
            segments_complete: r
                .segments
                .iter()
                .filter(|s| s.state == SegmentState::Complete)
                .count(),
            segments_total: r.segments.len(),
            delivery: r.delivery,
            pinned: r.pinned,
            audio_expired: r.audio_expired,
            is_test: r.is_test,
            completed_at_ms: r.completed_at_ms,
            needs_attention: r.error.is_some()
                || r.cleanup_error.is_some()
                || r.delivery == DeliveryState::Unknown
                || r.segments.iter().any(|s| s.state == SegmentState::Failed),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageUsage {
    pub recordings: u64,
    pub audio_bytes: u64,
}


