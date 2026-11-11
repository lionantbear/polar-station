//! Durable text results and history contracts. A text edit is never a recording.
use crate::{CaptureState, DeliveryState, Recording, RetentionPolicy};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextEditRecord {
    pub id: String,
    pub created_at_ms: Option<u64>,
    pub revision: u64,
    pub original: String,
    pub result: Option<String>,
    pub draft_text: Option<String>,
    pub mode: polar_transcreation::RefineMode,
    #[serde(default)]
    pub output_mode: Option<crate::DictateOutputMode>,
    pub running: bool,
    pub error: Option<String>,
    pub delivery: DeliveryState,
    pub delivery_token: Option<String>,
    pub delivery_text: Option<String>,
    #[serde(default)]
    pub copy_receipt: Option<CopyReceipt>,
    pub completed_at_ms: Option<u64>,
    pub pinned: bool,
    pub dismissed: bool,
    #[serde(default)]
    pub import_digest: Option<String>,
}
impl TextEditRecord {
    pub fn new(
        id: String,
        original: String,
        mode: polar_transcreation::RefineMode,
        now_ms: u64,
    ) -> Result<Self> {
        ensure!(
            crate::identifier(&id) && valid_text(&original),
            "dictate.invalid_text_edit"
        );
        Ok(Self {
            id,
            original,
            mode,
            output_mode: Some(if mode == polar_transcreation::RefineMode::English {
                crate::DictateOutputMode::Transcreate
            } else {
                crate::DictateOutputMode::Refine
            }),
            created_at_ms: Some(now_ms),
            revision: 1,
            result: None,
            draft_text: None,
            running: true,
            error: None,
            delivery: DeliveryState::Pending,
            delivery_token: None,
            delivery_text: None,
            copy_receipt: None,
            completed_at_ms: None,
            pinned: false,
            dismissed: false,
            import_digest: None,
        })
    }
    pub fn text(&self) -> &str {
        self.draft_text
            .as_deref()
            .or(self.result.as_deref())
            .unwrap_or(&self.original)
    }
    pub fn protected(&self) -> bool {
        self.pinned
            || self.running
            || self.completed_at_ms.is_none()
            || matches!(
                self.delivery,
                DeliveryState::Unknown | DeliveryState::Prepared
            )
    }
    pub fn apply(&mut self, change: ItemChange) -> Result<()> {
        match change {
            ItemChange::UseAvailable => {
                ensure!(self.delivery == DeliveryState::Pending, "dictate.delivery_conflict");
                self.draft_text = Some(self.text().into());
                self.running = false;
                self.error = None;
            }
            ItemChange::Edit { text } => {
                ensure!(
                    !self.running
                        && self.delivery != DeliveryState::Prepared
                        && text.len() <= crate::MAX_TEXT_BYTES
                        && !text.contains('\0'),
                    "dictate.text_edit_not_ready"
                );
                self.draft_text = Some(text);
                if self.delivery != DeliveryState::Unknown {
                    self.delivery = DeliveryState::Pending;
                }
                self.completed_at_ms = None;
                self.dismissed = false;
            }
            ItemChange::Prepare {
                token,
                text,
                edited,
            } => {
                ensure!(
                    !self.running
                        && self.delivery == DeliveryState::Pending
                        && crate::identifier(&token)
                        && valid_text(&text),
                    "dictate.delivery_not_ready"
                );
                ensure!(
                    edited || self.text() == text,
                    "dictate.delivery_text_changed"
                );
                if edited {
                    self.draft_text = Some(text.clone());
                }
                self.delivery_text = Some(text);
                self.delivery_token = Some(token);
                self.delivery = DeliveryState::Prepared;
            }
            ItemChange::Complete {
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
            ItemChange::Copied { now_ms, text } => {
                ensure!(
                    self.delivery != DeliveryState::Prepared,
                    "dictate.delivery_conflict"
                );
                self.running = false;
                if let Some(text) = text {
                    ensure!(
                        !text.trim().is_empty()
                            && text.len() <= 2 * crate::MAX_TEXT_BYTES
                            && !text.contains('\0'),
                        "dictate.invalid_copy_text"
                    );
                    self.copy_receipt = Some(CopyReceipt {
                        text,
                        at_ms: now_ms,
                    });
                }
                if self.delivery != DeliveryState::Delivered {
                    self.delivery = DeliveryState::Copied;
                    self.completed_at_ms.get_or_insert(now_ms);
                }
            }
            ItemChange::Reopen => {
                ensure!(
                    self.delivery == DeliveryState::Copied,
                    "dictate.delivery_conflict"
                );
                self.delivery = DeliveryState::Pending;
                self.completed_at_ms = None;
            }
            ItemChange::Pin { pinned } => self.pinned = pinned,
            ItemChange::Dismiss => {
                ensure!(
                    !self.running && self.delivery != DeliveryState::Prepared,
                    "dictate.stop_before_dismiss"
                );
                self.dismissed = true;
            }
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("dictate.revision_exhausted"))?;
        Ok(())
    }
}
pub fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= crate::MAX_TEXT_BYTES && !text.contains('\0')
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum HistoryItem {
    Recording(Box<Recording>),
    Text(Box<TextEditRecord>),
}
impl HistoryItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Recording(r) => &r.id,
            Self::Text(r) => &r.id,
        }
    }
    pub fn original(&self) -> Option<String> {
        match self {
            Self::Recording(r) => r.original(),
            Self::Text(r) => Some(r.original.clone()),
        }
    }
    pub fn text(&self) -> Option<String> {
        match self {
            Self::Recording(r) => r.text(),
            Self::Text(r) => Some(r.text().into()),
        }
    }
    pub fn delivery(&self) -> DeliveryState {
        match self {
            Self::Recording(r) => r.delivery,
            Self::Text(r) => r.delivery,
        }
    }
    pub fn active(&self) -> bool {
        match self {
            Self::Recording(r) => {
                r.capture == CaptureState::Recording
                    || r.cleanup_running
                    || r.segments
                        .iter()
                        .any(|s| s.state == crate::SegmentState::Running)
            }
            Self::Text(r) => r.running,
        }
    }
    pub fn protected(&self) -> bool {
        match self {
            Self::Recording(r) => r.retention_protected(),
            Self::Text(r) => r.protected(),
        }
    }
    pub fn completed_at_ms(&self) -> Option<u64> {
        match self {
            Self::Recording(r) => r.completed_at_ms,
            Self::Text(r) => r.completed_at_ms,
        }
    }
    pub fn audio_bytes(&self) -> u64 {
        match self {
            Self::Recording(r) if !r.audio_expired => r.saved_samples * 2,
            _ => 0,
        }
    }
    pub fn due_at(&self, policy: RetentionPolicy) -> Option<u64> {
        if !policy.enabled || self.protected() {
            return None;
        }
        self.completed_at_ms()
            .map(|t| t.saturating_add(policy.duration_ms()))
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::Recording(r) => r.revision,
            Self::Text(r) => r.revision,
        }
    }
    pub fn summary(&self) -> HistorySummary {
        let current = self.text().unwrap_or_default();
        let text = if current.trim().is_empty() {
            self.original().unwrap_or(current)
        } else {
            current
        };
        let (created_at_ms, pinned, saved_samples, is_test, has_audio, refined, dismissed, error) =
            match self {
                Self::Recording(r) => (
                    Some(r.created_at_ms),
                    r.pinned,
                    r.saved_samples,
                    r.is_test,
                    !r.audio_expired && r.saved_samples > 0,
                    r.cleaned.is_some(),
                    r.dismissed,
                    r.error.clone().or_else(|| r.cleanup_error.clone()),
                ),
                Self::Text(r) => (
                    r.created_at_ms,
                    r.pinned,
                    0,
                    false,
                    false,
                    r.result.is_some(),
                    r.dismissed,
                    r.error.clone(),
                ),
            };
        let output_mode = match self {
            Self::Recording(r) => r.output_mode,
            Self::Text(r) => r.output_mode,
        };
        HistorySummary {
            revision: self.revision(),
            output_mode,
            id: self.id().into(),
            kind: if matches!(self, Self::Recording(_)) {
                HistoryKind::Recording
            } else {
                HistoryKind::Text
            },
            created_at_ms,
            preview: text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(160)
                .collect(),
            has_text: !text.trim().is_empty(),
            pinned,
            saved_samples,
            is_test,
            has_audio,
            refined,
            dismissed,
            error,
            delivery: self.delivery(),
            active: self.active(),
            completed_at_ms: self.completed_at_ms(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    #[default]
    Recording,
    Text,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistorySummary {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub output_mode: Option<crate::DictateOutputMode>,
    pub id: String,
    pub kind: HistoryKind,
    pub created_at_ms: Option<u64>,
    pub preview: String,
    pub has_text: bool,
    pub has_audio: bool,
    pub saved_samples: u64,
    pub pinned: bool,
    pub is_test: bool,
    pub refined: bool,
    pub dismissed: bool,
    pub error: Option<String>,
    pub delivery: DeliveryState,
    pub active: bool,
    pub completed_at_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryCursor {
    pub created_ms: u64,
    pub kind: u8,
    pub id: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryPage {
    pub items: Vec<HistorySummary>,
    pub next: Option<HistoryCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemChange {
    UseAvailable,
    Edit {
        text: String,
    },
    Prepare {
        token: String,
        text: String,
        edited: bool,
    },
    Complete {
        token: String,
        applied: Option<bool>,
        now_ms: u64,
    },
    Copied {
        now_ms: u64,
        #[serde(default)]
        text: Option<String>,
    },
    Reopen,
    Pin {
        pinned: bool,
    },
    Dismiss,
}

/// One event bus for recording progress and durable history invalidation.
#[derive(Clone, Debug)]
pub enum DictateEvent {
    Recording(Box<Recording>),
    Item(Box<HistoryItem>),
    Removed(String),
    HistoryInvalidated,
    SettingsChanged,
}

/// Read-only compatibility boundary for the old Station v1 snapshot. This is
/// never used as the live operation model or as a provider request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyInput {
    pub operation_id: String,
    pub phase: String,
    pub raw: String,
    pub text: String,
    #[serde(default)]
    pub host_recording: bool,
    #[serde(default)]
    pub created_at_ms: Option<u64>,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub refined: bool,
    #[serde(default)]
    pub refine_baseline: Option<String>,
    #[serde(default)]
    pub output_mode: Option<crate::DictateOutputMode>,
    #[serde(default)]
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: u64,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopyReceipt {
    pub text: String,
    pub at_ms: u64,
}
