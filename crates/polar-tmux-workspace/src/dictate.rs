//! Shared Dictate policy. Platform adapters own audio and editor handles, never routing.
pub mod application;
mod history;
use serde::{Deserialize, Serialize};

pub const MAX_RECORDING_MS: u32 = 600_000;
pub const SHORTCUT_LABEL: &std::ffi::CStr = c"Hold to Dictate";
pub const SHORTCUT_MAC_LABEL: &std::ffi::CStr = c"Hold Fn to dictate";
pub const SHORTCUT_LINUX_LABEL: &std::ffi::CStr = c"Hold Ctrl + Super to dictate";
pub const SHORTCUT_TRIGGER: &std::ffi::CStr = c"CTRL+Super_L";
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutStatus {
    #[default]
    Unavailable,
    Ready,
    PermissionRequired,
    AccessDenied,
    CaptureUnavailable,
    Conflict,
    FnSetupRequired,
}
impl ShortcutStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "Dictate shortcut is not registered",
            Self::Ready => "Dictate shortcut ready",
            Self::CaptureUnavailable => "GNOME shortcut capture needs extension activation; sign out and back in once after installation",
            Self::AccessDenied => "Desktop denied Dictate keyboard access; use the recording button",
            Self::PermissionRequired => "Allow Accessibility access to use the Dictate shortcut",
            Self::Conflict => "Dictate shortcut conflicts with an existing binding",
            Self::FnSetupRequired => "Fn also invokes macOS: in Keyboard settings, set Press Globe key to Do Nothing",
        }
    }
}
const MAX_TEXT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Editor,
    IdeaNode,
    DefaultEditor,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub kind: TargetKind,
    pub identity: String,
    /// Adapter version includes the original selection and editor incarnation.
    pub revision: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub protected: bool,
}
impl Target {
    fn usable(&self) -> bool {
        !self.protected
            && !self.identity.is_empty()
            && self.identity.len() <= 1024
            && !self.revision.is_empty()
            && self.revision.len() <= 1024
            && self.context.len() <= 16 * 1024
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Recording,
    Transcribing,
    Rewriting,
    Pending,
    Delivering,
    Delivered,
    Copying,
    Copied,
    Unknown,
    Cancelled,
    Failed,
    Review,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Waiting for microphone permission…",
            Self::Recording => "Recording…",
            Self::Transcribing => "Transcribing…",
            Self::Rewriting => "Refining text…",
            Self::Pending => "Saved to Dictate. Activate an input to insert.",
            Self::Delivering => "Inserting…",
            Self::Delivered => "Inserted",
            Self::Copying => "Copying…",
            Self::Copied => "Copied",
            Self::Unknown => "Check the target before pasting again.",
            Self::Cancelled => "Cancelled",
            Self::Failed => "Dictate failed",
            Self::Review => "Target changed. Text saved to Dictate.",
        }
    }
    fn active(self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Recording
                | Self::Transcribing
                | Self::Rewriting
                | Self::Delivering
        )
    }
}

#[derive(Debug, Serialize)]
pub struct Presentation<'a> {
    pub action: &'static str,
    pub button_label: &'static str,
    pub enabled: bool,
    pub operation_id: Option<&'a str>,
    pub cancel_id: Option<&'a str>,
    pub use_original_id: Option<&'a str>,
    pub use_available_id: Option<&'a str>,
    pub status: std::borrow::Cow<'a, str>,
    pub saved_samples: u64,
    pub setup_required: bool,
    pub shortcut_status: &'static str,
    pub shortcut_hint: String,
    pub history_title: &'static str,
    pub review_label: &'static str,
    pub copy_label: &'static str,
    pub original_label: &'static str,
    pub dismiss_label: &'static str,
    pub cancel_label: &'static str,
}
#[derive(Debug, Serialize)]
pub struct HistoryEntry<'a> {
    pub operation_id: &'a str,
    pub text: &'a str,
    pub raw: &'a str,
    pub status: std::borrow::Cow<'a, str>,
    pub can_dismiss: bool,
    pub can_copy: bool,
    pub can_copy_original: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub transcription_mode: Option<polar_dictate::TranscriptionMode>,
    #[serde(default)]
    pub output_mode: Option<polar_dictate::DictateOutputMode>,
    #[serde(default)]
    pub created_at_ms: Option<u64>,
    pub operation_id: String,
    pub phase: Phase,
    pub target: Option<Target>,
    pub raw: String,
    pub text: String,
    pub error: Option<String>,
    #[serde(default)]
    pub saved_samples: u64,
    #[serde(default)]
    pub host_recording: bool,
    #[serde(default)]
    pub refined: bool,
    #[serde(default)]
    pub refine_baseline: Option<String>,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub unsaved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_error: Option<String>,
}
impl Entry {
    fn learning_source(&self) -> Option<polar_transcreation::learning::Refinement> {
        self.refine_baseline
            .as_ref()
            .filter(|_| self.refined)
            .map(|text| polar_transcreation::learning::Refinement {
                original: self.raw.clone(),
                refined: text.clone(),
                english: self.output_mode == Some(polar_dictate::DictateOutputMode::Transcreate),
                project: None,
            })
    }
    fn delivery_text(&self) -> String {
        polar_dictate::quoted_transcript(&self.reference, &self.text)
    }
    fn status(&self) -> std::borrow::Cow<'_, str> {
        if let Some(error) = self.copy_error.as_deref() {
            return error.into();
        }
        if self.phase == Phase::Copied {
            return "Copied".into();
        }
        if let Some(error) = self.error.as_deref() {
            if !self.raw.is_empty() {
                format!("Refinement failed; original retained · {error}").into()
            } else {
                error.into()
            }
        } else if self.refined {
            format!("Refinement succeeded · {}", self.phase.label()).into()
        } else {
            self.phase.label().into()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    TranscriptPreview {
        operation_id: String,
        text: String,
        #[serde(default)]
        saved_samples: u64,
    },
    ShowPanel {
        operation_id: String,
    },
    CopyDraft {
        operation_id: String,
        text: String,
    },
    Clipboard {
        operation_id: String,
        copied: bool,
    },
    HidePanel {
        operation_id: String,
    },
    EditDraft {
        operation_id: String,
        text: String,
    },
    InsertDraft {
        operation_id: String,
    },
    RetrySaved {
        operation_id: String,
    },
    ShortcutBinding {
        trigger: String,
    },
    ShortcutMode {
        toggle: bool,
    },
    ShortcutStatus {
        status: ShortcutStatus,
    },
    SetRefineMode {
        context_id: String,
        english: bool,
    },
    RefineContext {
        #[serde(default)]
        project: Option<String>,
        context_id: String,
        operation_id: String,
        target: Target,
        text: String,
    },
    RefineText {
        operation_id: String,
        target: Target,
        text: String,
    },
    StartContext {
        #[serde(default)]
        conversation_context: Option<String>,
        project: Option<String>,
        reference: String,
        operation_id: String,
        targets: Vec<Target>,
    },
    Start {
        #[serde(default)]
        reference: String,
        operation_id: String,
        targets: Vec<Target>,
    },
    AudioSaved {
        operation_id: String,
        saved_samples: u64,
    },
    CaptureReady {
        operation_id: String,
    },
    Stop {
        operation_id: String,
        duration_ms: u32,
    },
    Cancel {
        operation_id: String,
    },
    Transcribed {
        operation_id: String,
        text: String,
    },
    Rewritten {
        operation_id: String,
        text: Option<String>,
        error: Option<String>,
    },
    Failed {
        operation_id: String,
        error: String,
    },
    Focus {
        target: Target,
    },
    Delivery {
        operation_id: String,
        outcome: DeliveryOutcome,
    },
    UseOriginal {
        operation_id: String,
    },
    UseAvailable {
        operation_id: String,
    },
    Dismiss {
        operation_id: String,
    },
    StorageFailed,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    NoTarget,
    Applied,
    Unchanged,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    FinishCapture {
        operation_id: String,
    },
    Copy {
        operation_id: String,
        text: String,
        stop_capture: bool,
    },
    InsertCurrent {
        operation_id: String,
        text: String,
        preserve_selection: bool,
        source: Option<polar_transcreation::learning::Refinement>,
    },
    RetryTranscribe {
        operation_id: String,
    },
    Capture {
        project: Option<String>,
        reference: String,
        operation_id: String,
        maximum_ms: u32,
        context: Option<String>,
    },
    Transcribe {
        operation_id: String,
    },
    Cancel {
        operation_id: String,
    },
    Rewrite {
        #[serde(default)]
        english: bool,
        #[serde(default)]
        project: Option<String>,
        scope: Option<String>,
        saved_recording: bool,
        operation_id: String,
        raw: String,
        context: String,
    },
    Insert {
        operation_id: String,
        target: Target,
        text: String,
    },
}
#[derive(Debug, Clone, Default)]
pub struct Engine {
    host_events: bool,
    entries: Vec<Entry>,
    refine_modes: std::collections::BTreeMap<String, bool>,
    shortcut_status: ShortcutStatus,
    shortcut_trigger: String,
    hidden_panel: Option<String>,
    selected_panel: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_ref: Option<String>,
    #[serde(default)]
    legacy_pending: bool,
}
impl Engine {
    /// Host processing is observed, never scheduled by native phase callbacks.
    pub fn use_host_events(&mut self) {
        self.host_events = true;
    }
    pub fn observe_recording(&mut self, record: &polar_dictate::Recording) -> Vec<Effect> {
        let mut effects = Vec::new();
        if !self.host_events {
            return effects;
        }
        let Some(entry) = self.entries.iter_mut().find(|e| {
            e.operation_id == record.id
                && e.host_recording
                && matches!(e.phase, Phase::Transcribing | Phase::Rewriting)
        }) else {
            return effects;
        };
        entry.saved_samples = entry.saved_samples.max(record.saved_samples);
        entry.transcription_mode = Some(record.transcription_mode);
        entry.output_mode = Some(record.output_mode());
        if record.capture == polar_dictate::CaptureState::Recording {
            return effects;
        }
        if record.delivery != polar_dictate::DeliveryState::Pending {
            entry.phase = match record.delivery {
                polar_dictate::DeliveryState::Delivered => Phase::Delivered,
                polar_dictate::DeliveryState::Copied => Phase::Copied,
                _ => Phase::Unknown,
            };
            return effects;
        }
        let Some(original) = record.original() else {
            if let Some(error) = record
                .error
                .as_ref()
                .or_else(|| record.segments.iter().find_map(|s| s.error.as_ref()))
            {
                entry.error = Some(error.chars().take(512).collect());
                entry.phase = Phase::Failed;
            } else if !record.processing_enabled {
                entry.error = Some("Processing stopped; saved audio is available for retry.".into());
                entry.phase = Phase::Failed;
            }
            return effects;
        };
        entry.raw = original.clone();
        if record.cleanup_enabled
            && (record.cleanup_deferred
                || record.cleanup_running
                || (record.cleaned.is_none() && record.cleanup_error.is_none()))
        {
            entry.text = original;
            entry.phase = Phase::Rewriting;
            return effects;
        }
        entry.error = record.cleanup_error.clone();
        entry.refined = record.cleaned.is_some() && entry.error.is_none();
        entry.refine_baseline = if entry.refined {
            record.cleaned.clone()
        } else {
            None
        };
        entry.text = record.cleaned.clone().unwrap_or(original);
        entry.phase = Phase::Pending;
        Self::insert(entry, &mut effects);
        effects
    }
    pub fn history(&self) -> Vec<HistoryEntry<'_>> {
        self.entries
            .iter()
            .rev()
            .filter(|entry| {
                !entry.text.trim().is_empty()
                    || !entry.raw.trim().is_empty()
                    || entry.saved_samples > 0
            })
            .map(|entry| HistoryEntry {
                operation_id: &entry.operation_id,
                text: &entry.text,
                raw: &entry.raw,
                status: entry.status(),
                can_dismiss: !entry.phase.active(),
                can_copy: !entry.text.is_empty(),
                can_copy_original: !entry.raw.trim().is_empty() && entry.raw != entry.text,
            })
            .collect()
    }
    pub fn presentation(&self) -> Presentation<'_> {
        let active = self.entries.iter().find(|e| e.phase.active());
        let recording =
            active.is_some_and(|e| matches!(e.phase, Phase::Starting | Phase::Recording));
        let latest = active.or_else(|| self.entries.last());
        Presentation {
            action: if recording {
                "stop"
            } else if active.is_some() {
                "wait"
            } else {
                "start"
            },
            button_label: if recording {
                "Stop Dictate"
            } else {
                "Start Dictate"
            },
            enabled: recording || active.is_none(),
            operation_id: active.map(|e| e.operation_id.as_str()),
            cancel_id: active
                .filter(|e| e.phase != Phase::Delivering)
                .map(|e| e.operation_id.as_str()),
            setup_required: active
                .and_then(|entry| entry.error.as_deref())
                .is_some_and(|error| {
                    let error = error.to_ascii_lowercase();
                    [
                        "voice.auth",
                        "voice.provider_unavailable",
                        "microphone",
                        "permission",
                        "credentials",
                        "voice settings are not ready",
                        "voice capture could not start",
                    ]
                    .iter()
                    .any(|cause| error.contains(cause))
                }),
            use_original_id: active
                .filter(|e| e.phase == Phase::Rewriting && !e.raw.is_empty())
                .map(|e| e.operation_id.as_str()),
            use_available_id: active
                .filter(|e| matches!(e.phase, Phase::Transcribing | Phase::Rewriting))
                .map(|e| e.operation_id.as_str()),
            status: if let Some(entry) = latest {
                if entry.phase == Phase::Recording {
                    format!(
                        "Recording {:02}:{:02} / 10:00 · Saved {:.1}s",
                        entry.saved_samples / 16000 / 60,
                        entry.saved_samples / 16000 % 60,
                        entry.saved_samples as f64 / 16000.0
                    )
                    .into()
                } else {
                    entry.status()
                }
            } else {
                "Ready".into()
            },
            saved_samples: latest.map_or(0, |entry| entry.saved_samples),
            shortcut_status: self.shortcut_status.label(),
            shortcut_hint: if active.is_some_and(|e| matches!(e.phase, Phase::Transcribing | Phase::Rewriting)) {
                "Press the Dictate shortcut to stop processing and insert available text".into()
            } else if self.shortcut_trigger.is_empty() {
                "Configure Dictate shortcut in Voice settings".into()
            } else {
                format!("Tap {} to keep recording; hold and release to stop; press again to stop",self.shortcut_trigger)
            },
            history_title: "Dictate drafts",
            review_label: "Review result",
            copy_label: "Copy",
            original_label: "Copy Original",
            dismiss_label: "Dismiss",
            cancel_label: "Cancel",
        }
    }
    pub fn set_recording_mode(&mut self, id: &str, mode: polar_dictate::TranscriptionMode) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == id) {
            entry.transcription_mode = Some(mode);
        }
    }
    pub fn panel(&self) -> serde_json::Value {
        let Some(entry) = self
            .selected_panel
            .as_ref()
            .and_then(|id| self.entries.iter().find(|entry| &entry.operation_id == id))
            .or_else(|| self.entries.iter().find(|e| e.phase.active()))
            .or_else(|| self.entries.last())
        else {
            return serde_json::Value::Null;
        };
        if self.hidden_panel.as_deref() == Some(&entry.operation_id)
            || matches!(entry.phase, Phase::Cancelled | Phase::Copied)
            || (entry.phase == Phase::Failed && entry.saved_samples == 0 && entry.text.is_empty())
        {
            return serde_json::Value::Null;
        }
        let active = entry.phase.active();
        let mode = match entry.transcription_mode {
            Some(polar_dictate::TranscriptionMode::Realtime) => "Realtime",
            Some(polar_dictate::TranscriptionMode::Segmented) => "Dictate",
            None => "Voice",
        };
        let seconds = entry.saved_samples / polar_dictate::SAMPLE_RATE;
        let heading = if entry.phase == Phase::Recording {
            format!("● {mode} · {:02}:{:02}", seconds / 60, seconds % 60)
        } else if matches!(entry.phase, Phase::Pending | Phase::Review) {
            format!("Draft · {mode}")
        } else {
            format!("{} · {mode}", entry.phase.label())
        };
        let heading = if let Some(output) = entry.output_mode {
            format!("{heading} · {}", output.label())
        } else {
            heading
        };
        serde_json::json!({
            "heading":heading,"output_mode":entry.output_mode,"transcription_mode":entry.transcription_mode,"reference":entry.reference,"copy_text":entry.delivery_text(),
            "operation_id":entry.operation_id,"phase":entry.phase,"text":entry.text,
            "status":if entry.phase == Phase::Recording { self.presentation().status } else { entry.status() },
            "can_stop":matches!(entry.phase,Phase::Starting|Phase::Recording),
            "can_cancel":active && entry.phase != Phase::Delivering,
            "can_copy":!entry.text.is_empty(),
            "can_edit":!active && entry.phase != Phase::Delivered && !entry.raw.is_empty(),
            "can_insert":!active && !matches!(entry.phase,Phase::Delivered|Phase::Unknown|Phase::Failed) && valid_text(&entry.text),
            "can_retry":entry.phase == Phase::Failed && entry.host_recording && entry.saved_samples > 0,
            "can_available":matches!(entry.phase,Phase::Transcribing|Phase::Rewriting),
            "hide_after_ms":if entry.phase == Phase::Delivered { 1600 } else { 0 },
            "title":"Dictate","stop_label":"Stop","cancel_label":"Cancel",
            "copy_label":"Copy","edit_label":"Edit","insert_label":"Insert",
            "retry_label":"Retry transcription","close_label":"Close","available_label":if valid_text(&entry.text) { "Use text now" } else { "Stop waiting" }
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    /// Called only after the Host confirms deletion. Late capture/cleanup input
    /// cannot recreate a removed entry; legacy text-only snapshots stay separate.
    pub fn discard_saved(&mut self, id: &str) -> Vec<Effect> {
        let effects = self
            .entries
            .iter()
            .filter(|entry| entry.operation_id == id && entry.phase.active())
            .map(|entry| Effect::Cancel {
                operation_id: entry.operation_id.clone(),
            })
            .collect();
        self.entries.retain(|entry| entry.operation_id != id);
        effects
    }
    /// Project an authoritative saved recording without running any effect.
    /// The next explicit Focus reuses the normal rewrite/delivery path.
    pub fn stage_saved(&mut self, recording: &polar_dictate::Recording) -> Result<(), String> {
        recording
            .validate_saved_input()
            .map_err(|error| error.to_string())?;
        if self.entries.iter().any(|entry| entry.phase.active()) {
            return Err("Finish or cancel the active Dictate operation first".into());
        }
        if self.entries.iter().any(|entry| {
            entry.operation_id == recording.id
                && matches!(entry.phase, Phase::Delivered | Phase::Unknown)
        }) {
            return Err("This recording requires manual review before reuse".into());
        }
        let raw = recording.original().unwrap_or_default();
        let accepted_draft = !recording.processing_enabled
            && recording.draft_text.as_deref().is_some_and(valid_text);
        let edited = self
            .entries
            .iter()
            .find(|entry| entry.operation_id == recording.id && entry.edited);
        let text = edited
            .map(|entry| entry.text.clone())
            .or_else(|| recording.text())
            .ok_or("Saved transcript is incomplete")?;
        let was_edited = edited.is_some() || accepted_draft;
        if (!valid_text(&raw) && !accepted_draft) || !valid_text(&text) {
            return Err("Invalid saved recording text".into());
        }
        // An explicit selection takes precedence over older unbound drafts;
        // keep those drafts reviewable instead of silently inserting the wrong one.
        for entry in &mut self.entries {
            if entry.phase == Phase::Pending && entry.target.is_none() {
                entry.phase = Phase::Review;
            }
        }
        self.entries
            .retain(|entry| entry.operation_id != recording.id);
        self.entries.push(Entry {
            unsaved: false,
            output_mode: None,
            reference: recording.reference.clone(),
            transcription_mode: Some(recording.transcription_mode),
            created_at_ms: Some(recording.created_at_ms),
            host_recording: true,
            edited: was_edited,
            copy_error: None,
            refine_baseline: recording.cleaned.clone(),
            refined: recording.cleaned.is_some() && recording.cleanup_error.is_none(),
            operation_id: recording.id.clone(),
            phase: Phase::Pending,
            target: None,
            raw,
            text,
            error: None,
            saved_samples: recording.saved_samples,
        });
        Ok(())
    }
    pub fn copy_saved(
        &mut self,
        record: &polar_dictate::Recording,
        original: bool,
    ) -> Result<Vec<Effect>, String> {
        let local = self.entries.iter().find(|e| e.operation_id == record.id);
        let body = if original {
            record.original().or_else(|| local.map(|e| e.raw.clone()))
        } else {
            local
                .filter(|e| e.edited)
                .map(|e| e.text.clone())
                .or_else(|| record.text())
                .or_else(|| local.map(|e| e.text.clone()))
        }
        .ok_or("No text to copy")?;
        if !valid_text(&body) {
            return Err("No text to copy".into());
        }
        if local.is_none() {
            self.entries.push(Entry {
                unsaved: false,
                output_mode: None,
                reference: record.reference.clone(),
                transcription_mode: Some(record.transcription_mode),
                created_at_ms: Some(record.created_at_ms),
                operation_id: record.id.clone(),
                phase: Phase::Review,
                target: None,
                raw: record.original().unwrap_or_else(|| body.clone()),
                text: body.clone(),
                error: None,
                saved_samples: record.saved_samples,
                host_recording: true,
                refined: record.cleaned.is_some(),
                refine_baseline: record.cleaned.clone(),
                edited: false,
                copy_error: None,
            });
        }
        self.dispatch(Input::CopyDraft {
            operation_id: record.id.clone(),
            text: body,
        })
    }
    pub fn stage_item(&mut self, item: &polar_dictate::HistoryItem) -> Result<(), String> {
        match item {
            polar_dictate::HistoryItem::Recording(record) => self.stage_saved(record),
            polar_dictate::HistoryItem::Text(record) => {
                if record.running {
                    return Err("Refinement is still running".into());
                }
                self.entries.retain(|e| e.operation_id != record.id);
                self.entries.push(Entry {
                    unsaved: false,
                    operation_id: record.id.clone(),
                    created_at_ms: record.created_at_ms,
                    reference: String::new(),
                    transcription_mode: None,
                    output_mode: Some(if record.mode == polar_transcreation::RefineMode::English {
                        polar_dictate::DictateOutputMode::Transcreate
                    } else {
                        polar_dictate::DictateOutputMode::Refine
                    }),
                    raw: record.original.clone(),
                    text: record.text().into(),
                    phase: match record.delivery {
                        polar_dictate::DeliveryState::Delivered => Phase::Delivered,
                        polar_dictate::DeliveryState::Copied => Phase::Copied,
                        polar_dictate::DeliveryState::Unknown
                        | polar_dictate::DeliveryState::Prepared => Phase::Unknown,
                        _ => Phase::Review,
                    },
                    target: None,
                    error: record.error.clone(),
                    saved_samples: 0,
                    host_recording: false,
                    refined: record.result.is_some(),
                    refine_baseline: record.result.clone(),
                    edited: record.draft_text.is_some(),
                    copy_error: None,
                });
                Ok(())
            }
        }
    }
    pub fn stage_retry(&mut self, record: &polar_dictate::Recording) -> Result<(), String> {
        if record.is_test
            || record.saved_samples == 0
            || record.audio_expired
            || record.delivery != polar_dictate::DeliveryState::Pending
            || self.entries.iter().any(|e| e.phase.active())
        {
            return Err(
                "This recording cannot be retried while capture or insertion is active".into(),
            );
        }
        self.entries.retain(|e| e.operation_id != record.id);
        self.entries.push(Entry {
            unsaved: false,
            output_mode: None,
            reference: record.reference.clone(),
            transcription_mode: Some(record.transcription_mode),
            operation_id: record.id.clone(),
            phase: Phase::Failed,
            target: None,
            created_at_ms: Some(record.created_at_ms),
            raw: String::new(),
            text: String::new(),
            error: record.error.clone(),
            saved_samples: record.saved_samples,
            host_recording: true,
            refined: false,
            refine_baseline: None,
            edited: false,
            copy_error: None,
        });
        self.selected_panel = None;
        Ok(())
    }
    pub fn dispatch(&mut self, input: Input) -> Result<Vec<Effect>, String> {
        let mut effects = Vec::new();
        match input {
            Input::UseAvailable { operation_id } => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == operation_id
                    && matches!(e.phase, Phase::Transcribing | Phase::Rewriting)) {
                    entry.edited = true;
                    entry.refined = false;
                    entry.refine_baseline = None;
                    if valid_text(&entry.text) {
                        entry.error = None;
                        entry.phase = Phase::Pending;
                        Self::insert(entry, &mut effects);
                    } else {
                        entry.phase = Phase::Failed;
                        entry.error = Some("Processing stopped; no text yet. Saved audio is available for retry.".into());
                    }
                }
            }
            Input::CopyDraft { operation_id, text } => {
                if !valid_text(&text) {
                    return Err("Invalid or oversized clipboard text".into());
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.operation_id == operation_id
                        && !matches!(e.phase, Phase::Delivering | Phase::Copying)
                }) {
                    let stop_capture = matches!(entry.phase, Phase::Starting | Phase::Recording);
                    entry.text = text;
                    entry.edited = true;
                    entry.phase = Phase::Copying;
                    entry.error = None;
                    entry.copy_error = None;
                    effects.push(Effect::Copy {
                        operation_id: operation_id.clone(),
                        text: entry.delivery_text(),
                        stop_capture,
                    });
                }
            }
            Input::Clipboard {
                operation_id,
                copied,
            } => {
                if let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.operation_id == operation_id && e.phase == Phase::Copying)
                {
                    entry.target = None;
                    if copied {
                        entry.phase = Phase::Copied;
                        self.hidden_panel = Some(operation_id);
                    } else {
                        entry.phase = if valid_text(&entry.raw) {
                            Phase::Review
                        } else {
                            Phase::Failed
                        };
                        entry.copy_error =
                            Some("Copy failed; draft retained. Try Copy again.".into());
                    }
                }
            }
            Input::ShowPanel { operation_id } => {
                if self
                    .entries
                    .iter()
                    .any(|entry| entry.operation_id == operation_id)
                {
                    self.selected_panel = Some(operation_id);
                    self.hidden_panel = None;
                }
            }
            Input::HidePanel { operation_id } => {
                if let Some(entry) = self.entries.iter_mut().find(|entry| {
                    entry.operation_id == operation_id
                        && entry.phase.active()
                        && entry.phase != Phase::Delivering
                }) {
                    entry.phase = Phase::Cancelled;
                    effects.push(Effect::Cancel {
                        operation_id: operation_id.clone(),
                    });
                }
                self.hidden_panel = Some(operation_id);
            }
            Input::EditDraft { operation_id, text } => {
                if text.len() > MAX_TEXT || text.contains('\0') {
                    return Err("Dictate draft exceeds text limit".into());
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.operation_id == operation_id
                        && !e.phase.active()
                        && !matches!(e.phase, Phase::Delivered | Phase::Copying | Phase::Copied)
                        && !e.raw.is_empty()
                }) {
                    entry.text = text;
                    entry.edited = true;
                    entry.target = None;
                    entry.phase = Phase::Pending;
                }
            }
            Input::InsertDraft { operation_id } => {
                if self.entries.iter().any(|e| e.phase.active()) {
                    return Ok(effects);
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.operation_id == operation_id
                        && !matches!(e.phase, Phase::Delivered | Phase::Unknown | Phase::Failed)
                        && valid_text(&e.text)
                }) {
                    entry.target = None;
                    entry.phase = Phase::Delivering;
                    effects.push(Effect::InsertCurrent {
                        operation_id: entry.operation_id.clone(),
                        text: entry.delivery_text(),
                        preserve_selection: !entry.reference.is_empty(),
                        source: entry.learning_source(),
                    });
                    self.hidden_panel = None;
                }
            }
            Input::RetrySaved { operation_id } => {
                if self.entries.iter().any(|e| e.phase.active()) {
                    return Ok(effects);
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.operation_id == operation_id
                        && e.phase == Phase::Failed
                        && e.saved_samples > 0
                        && e.host_recording
                }) {
                    entry.phase = Phase::Transcribing;
                    entry.error = None;
                    self.hidden_panel = None;
                    effects.push(Effect::RetryTranscribe { operation_id });
                }
            }
            Input::TranscriptPreview {
                operation_id,
                text,
                saved_samples,
            } => {
                if text.len() > 64 * 1024
                    || text.contains('\0')
                    || saved_samples > polar_dictate::MAX_RECORDING_SAMPLES
                {
                    return Err("Invalid transcript preview".into());
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.operation_id == operation_id
                        && matches!(
                            e.phase,
                            Phase::Starting | Phase::Recording | Phase::Transcribing
                        )
                }) {
                    entry.text = text;
                    entry.saved_samples = entry.saved_samples.max(saved_samples);
                }
            }
            Input::AudioSaved {
                operation_id,
                saved_samples,
            } => {
                if let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|entry| entry.operation_id == operation_id)
                {
                    if saved_samples < entry.saved_samples || saved_samples > 9_600_000 {
                        return Err("Invalid durable audio position".into());
                    }
                    entry.saved_samples = saved_samples;
                    entry.host_recording = true;
                }
            }
            Input::ShortcutMode { toggle: _ } => {}
            Input::ShortcutBinding { trigger } => {
                if trigger.len() > 256 || trigger.chars().any(char::is_control) {
                    return Err("Invalid shortcut description".into());
                }
                self.shortcut_trigger = trigger;
            }
            Input::ShortcutStatus { status } => {
                self.shortcut_status = status;
            }
            Input::StorageFailed => {
                for entry in &mut self.entries {
                    if entry.phase.active() {
                        effects.push(Effect::Cancel {
                            operation_id: entry.operation_id.clone(),
                        });
                        entry.phase = if entry.raw.is_empty() {
                            Phase::Failed
                        } else {
                            Phase::Review
                        };
                    }
                    if entry.phase != Phase::Delivered {
                        entry.error = Some(
                            "Dictate could not save. Copy any text before closing the app.".into(),
                        );
                    }
                }
            }
            Input::SetRefineMode {
                context_id,
                english,
            } => {
                if context_id.is_empty() || context_id.len() > 1024 {
                    return Err("Refine context unavailable".into());
                }
                if english {
                    self.refine_modes.insert(context_id, true);
                } else {
                    self.refine_modes.remove(&context_id);
                }
            }
            Input::RefineContext {
                context_id,
                operation_id,
                target,
                text,
                project,
            } => {
                let english = self.refine_modes.get(&context_id).copied().unwrap_or(false);
                effects = self.dispatch(Input::RefineText {
                    operation_id,
                    target,
                    text,
                })?;
                for effect in &mut effects {
                    if let Effect::Rewrite {
                        english: value,
                        scope,
                        project: effect_project,
                        ..
                    } = effect
                    {
                        *value = english;
                        *scope = Some(context_id.clone());
                        *effect_project = project.clone();
                    }
                }
            }
            Input::RefineText {
                operation_id,
                target,
                text,
            } => {
                if operation_id.is_empty()
                    || operation_id.len() > 128
                    || !target.usable()
                    || !valid_text(&text)
                {
                    return Err("Select a nonempty editable draft to refine".into());
                }
                if self.entries.iter().any(|e| e.operation_id == operation_id) {
                    return Ok(effects);
                }
                if !self.presentation().enabled || self.entries.iter().any(|e| e.phase.active()) {
                    return Err("Finish the active Dictate operation first".into());
                }
                self.entries.retain(|e| {
                    e.phase.active() || !e.raw.trim().is_empty() || !e.text.trim().is_empty()
                });
                let mut entry = Entry {
                    unsaved: false,
                    output_mode: None,
                    reference: String::new(),
                    transcription_mode: None,
                    created_at_ms: None,
                    operation_id,
                    target: Some(target),
                    raw: text.clone(),
                    text,
                    phase: Phase::Rewriting,
                    error: None,
                    saved_samples: 0,
                    host_recording: false,
                    refined: false,
                    refine_baseline: None,
                    edited: false,
                    copy_error: None,
                };
                Self::rewrite(&mut entry, &mut effects);
                self.entries.push(entry);
            }
            Input::StartContext {
                conversation_context,
                project,
                reference,
                operation_id,
                targets,
            } => {
                if project.as_ref().is_some_and(|p| {
                    !p.starts_with('/') || p.len() > 4096 || p.chars().any(char::is_control)
                }) {
                    return Err("Invalid Dictate project".into());
                }
                if conversation_context.as_ref().is_some_and(|s| s.len()>8192 || s.contains('\0')) {
                    return Err("Invalid conversation reference".into());
                }
                effects = self.dispatch(Input::Start {
                    reference,
                    operation_id,
                    targets,
                })?;
                for effect in &mut effects {
                    if let Effect::Capture { project: value, context, .. } = effect {
                        *value = project.clone();
                        if let Some(conversation) = conversation_context.as_ref().filter(|s| !s.trim().is_empty()) {
                            let nearby=context.as_deref().unwrap_or_default();
                            let nearby=&nearby[..nearby.floor_char_boundary(4096.min(nearby.len()))];
                            *context=Some(format!("Conversation reference:\n{conversation}\n\nCurrent input:\n{nearby}"));
                            if let Some(target)=self.entries.last_mut().and_then(|e|e.target.as_mut()) {target.context=context.clone().unwrap_or_default();}
                        }
                    }
                }
            }
            Input::Start {
                reference,
                operation_id,
                targets,
            } => {
                if self.entries.iter().any(|e| e.operation_id == operation_id) {
                    return Ok(effects);
                }
                if operation_id.is_empty() || operation_id.len() > 128 {
                    return Err("Invalid Dictate operation".into());
                }
                // Empty terminal attempts are not text drafts. Their audio remains
                // owned by the Host recording store and recoverable in Voice settings.
                self.entries.retain(|e| {
                    e.phase.active() || !e.raw.trim().is_empty() || !e.text.trim().is_empty()
                });
                if targets.len() > 16 {
                    return Err("Too many Dictate targets".into());
                }
                if self.entries.iter().any(|e| {
                    matches!(
                        e.phase,
                        Phase::Starting
                            | Phase::Recording
                            | Phase::Transcribing
                            | Phase::Rewriting
                            | Phase::Delivering
                    )
                }) {
                    return Err("Dictate is already active".into());
                }
                let protected_focus = targets
                    .iter()
                    .any(|t| t.kind == TargetKind::Editor && t.protected);
                let target = if protected_focus {
                    None
                } else {
                    targets
                        .into_iter()
                        .filter(Target::usable)
                        .min_by_key(|t| match t.kind {
                            TargetKind::Editor => 0,
                            TargetKind::IdeaNode => 1,
                            TargetKind::DefaultEditor => 2,
                        })
                };
                if reference.len() > 16 * 1024 || reference.contains('\0') {
                    return Err("Selected quote is too large; recording was not started".into());
                }
                self.selected_panel = None;
                effects.push(Effect::Capture {
                    project: None,
                    reference: reference.clone(),
                    operation_id: operation_id.clone(),
                    maximum_ms: MAX_RECORDING_MS,
                    context: target.as_ref().map(|t| t.context.clone()),
                });
                self.entries.push(Entry {
                    unsaved: false,
                    output_mode: None,
                    reference,
                    transcription_mode: None,
                    created_at_ms: None,
                    host_recording: true,
                    edited: false,
                    copy_error: None,
                    refined: false,
                    refine_baseline: None,
                    saved_samples: 0,
                    operation_id,
                    phase: Phase::Starting,
                    target,
                    raw: String::new(),
                    text: String::new(),
                    error: None,
                });
            }
            Input::Focus { target } => {
                if !target.usable() || target.kind != TargetKind::Editor {
                    return Ok(effects);
                }
                // Do not insert while another recording or delivery owns the editor.
                if self.entries.iter().any(|e| {
                    matches!(
                        e.phase,
                        Phase::Starting
                            | Phase::Recording
                            | Phase::Transcribing
                            | Phase::Rewriting
                            | Phase::Delivering
                    )
                }) {
                    return Ok(effects);
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| {
                    e.phase == Phase::Pending && e.target.is_none() && valid_text(&e.text)
                }) {
                    entry.target = Some(target);
                    if entry.refined || entry.edited {
                        Self::insert(entry, &mut effects);
                    } else {
                        Self::rewrite(entry, &mut effects);
                    }
                }
            }
            Input::Dismiss { operation_id } => {
                if let Some(e) = self.entries.iter().find(|e| e.operation_id == operation_id) {
                    if matches!(
                        e.phase,
                        Phase::Starting
                            | Phase::Recording
                            | Phase::Transcribing
                            | Phase::Rewriting
                            | Phase::Delivering
                    ) {
                        return Err("Cancel the active operation first".into());
                    }
                }
                self.entries.retain(|e| e.operation_id != operation_id);
            }
            other => {
                let id = match &other {
                    Input::CaptureReady { operation_id }
                    | Input::Stop { operation_id, .. }
                    | Input::Cancel { operation_id }
                    | Input::Transcribed { operation_id, .. }
                    | Input::Rewritten { operation_id, .. }
                    | Input::Failed { operation_id, .. }
                    | Input::Delivery { operation_id, .. }
                    | Input::UseOriginal { operation_id } => operation_id,
                    _ => unreachable!(),
                };
                let Some(entry) = self.entries.iter_mut().find(|e| &e.operation_id == id) else {
                    return Ok(effects);
                };
                match other {
                    Input::CaptureReady { .. } if entry.phase == Phase::Starting => {
                        entry.phase = Phase::Recording;
                    }
                    Input::Stop { .. } if entry.phase == Phase::Starting => {
                        entry.phase = Phase::Cancelled;
                        effects.push(Effect::Cancel {
                            operation_id: entry.operation_id.clone(),
                        });
                    }
                    Input::Stop { duration_ms, .. } if entry.phase == Phase::Recording => {
                        if !(300..=MAX_RECORDING_MS).contains(&duration_ms) {
                            entry.phase = Phase::Failed;
                            entry.error = Some("Record between 0.3 seconds and 10 minutes".into());
                            effects.push(Effect::Cancel {
                                operation_id: entry.operation_id.clone(),
                            });
                        } else {
                            entry.phase = Phase::Transcribing;
                            effects.push(if self.host_events {
                                Effect::FinishCapture {
                                    operation_id: entry.operation_id.clone(),
                                }
                            } else {
                                Effect::Transcribe {
                                    operation_id: entry.operation_id.clone(),
                                }
                            });
                        }
                    }
                    Input::Cancel { .. }
                        if matches!(
                            entry.phase,
                            Phase::Starting
                                | Phase::Recording
                                | Phase::Transcribing
                                | Phase::Rewriting
                        ) =>
                    {
                        entry.phase = Phase::Cancelled;
                        effects.push(Effect::Cancel {
                            operation_id: entry.operation_id.clone(),
                        });
                    }
                    Input::Transcribed { text, .. } if entry.phase == Phase::Transcribing => {
                        if !valid_text(&text) {
                            entry.phase = Phase::Failed;
                            entry.error = Some("No usable transcription".into());
                        } else {
                            entry.raw = text.clone();
                            entry.text = text.clone();
                            entry.phase = Phase::Pending;
                            Self::rewrite(entry, &mut effects);
                        }
                    }
                    Input::Rewritten { text, error, .. } if entry.phase == Phase::Rewriting => {
                        let text = text.filter(|t| valid_text(t));
                        entry.error = error.or_else(|| text.is_none().then(|| "Rewrite returned no usable text; original transcription retained".into()))
                            .map(|s| s.chars().take(512).collect());
                        entry.refined = entry.error.is_none();
                        entry.refine_baseline = if entry.refined { text.clone() } else { None };
                        if entry.refined {
                            if let Some(text) = text {
                                entry.text = text;
                            }
                        } else {
                            entry.text = entry.raw.clone();
                        }
                        entry.phase = Phase::Pending;
                        Self::insert(entry, &mut effects);
                    }
                    Input::Failed { error, .. }
                        if matches!(
                            entry.phase,
                            Phase::Starting
                                | Phase::Recording
                                | Phase::Transcribing
                                | Phase::Rewriting
                        ) =>
                    {
                        entry.error = Some(error.chars().take(512).collect());
                        if entry.raw.is_empty() {
                            entry.phase = Phase::Failed;
                        } else {
                            entry.text = entry.raw.clone();
                            entry.phase = Phase::Pending;
                            Self::insert(entry, &mut effects);
                        }
                    }
                    Input::Delivery { outcome, .. } if entry.phase == Phase::Delivering => {
                        entry.phase = match outcome {
                            DeliveryOutcome::NoTarget => {
                                entry.target = None;
                                Phase::Pending
                            }
                            DeliveryOutcome::Applied => Phase::Delivered,
                            DeliveryOutcome::Unchanged => Phase::Review,
                            DeliveryOutcome::Unknown => Phase::Unknown,
                        };
                    }
                    Input::UseOriginal { .. }
                        if matches!(entry.phase, Phase::Pending | Phase::Review) =>
                    {
                        entry.text = entry.raw.clone();
                    }
                    _ => {}
                }
            }
        }
        Ok(effects)
    }
    fn rewrite(entry: &mut Entry, effects: &mut Vec<Effect>) {
        entry.phase = Phase::Rewriting;
        effects.push(Effect::Rewrite {
            english: false,
            project: None,
            scope: None,
            saved_recording: entry.host_recording,
            operation_id: entry.operation_id.clone(),
            raw: entry.raw.clone(),
            context: entry
                .target
                .as_ref()
                .map(|t| t.context.clone())
                .unwrap_or_default(),
        });
    }
    fn insert(entry: &mut Entry, effects: &mut Vec<Effect>) {
        if entry.host_recording {
            entry.phase = Phase::Delivering;
            effects.push(Effect::InsertCurrent {
                operation_id: entry.operation_id.clone(),
                text: entry.delivery_text(),
                preserve_selection: !entry.reference.is_empty(),
                source: entry.learning_source(),
            });
            return;
        }
        if let Some(target) = entry.target.clone() {
            entry.phase = Phase::Delivering;
            effects.push(Effect::Insert {
                operation_id: entry.operation_id.clone(),
                target,
                text: entry.delivery_text(),
            });
        }
    }
    /// A restart never replays an uncertain insertion or resumes microphone capture.
    pub fn restore(mut saved: Snapshot) -> Result<Self, String> {
        if !matches!(saved.version, 1 | 2) {
            return Err("Unsupported Dictate history version".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for entry in &mut saved.entries {
            // Older clients could turn a failed partial preview into Review.
            // Keep its audio/text, but never treat it as a complete transcript.
            if entry.phase == Phase::Review
                && entry.host_recording
                && entry.saved_samples > 0
                && entry.raw.is_empty()
                && valid_text(&entry.text)
                && entry.error.is_some()
            {
                entry.phase = Phase::Failed;
            }
            if entry.operation_id.is_empty()
                || entry.operation_id.len() > 128
                || !ids.insert(entry.operation_id.clone())
                || entry.reference.len() > 16 * 1024
                || entry.reference.contains('\0')
                || entry.raw.len() > MAX_TEXT
                || entry.text.len() > MAX_TEXT
                || entry.raw.contains('\0')
                || entry.text.contains('\0')
                || entry
                    .error
                    .as_ref()
                    .is_some_and(|e| e.chars().count() > 512)
                || entry.target.as_ref().is_some_and(|t| !t.usable())
                || (matches!(
                    entry.phase,
                    Phase::Pending
                        | Phase::Rewriting
                        | Phase::Delivering
                        | Phase::Delivered
                        | Phase::Review
                        | Phase::Unknown
                ) && (!(valid_text(&entry.raw)
                    || entry.host_recording && entry.edited && valid_text(&entry.text))
                    || (!valid_text(&entry.text) && !entry.edited)))
            {
                return Err("Invalid Dictate history".into());
            }
            if let Some(target) = &mut entry.target {
                target.context.clear();
            }
            entry.phase = match entry.phase {
                Phase::Delivering => Phase::Unknown,
                Phase::Copying => {
                    if valid_text(&entry.raw) {
                        Phase::Review
                    } else {
                        Phase::Failed
                    }
                }
                Phase::Starting | Phase::Recording | Phase::Transcribing => Phase::Cancelled,
                Phase::Rewriting => {
                    entry.text = entry.raw.clone();
                    Phase::Review
                }
                p => p,
            };
        }
        Ok(Self {
            entries: saved.entries,
            ..Self::default()
        })
    }
    pub fn persistent_snapshot(&self) -> Snapshot {
        if self.host_events {
            return Snapshot {
                version: 2,
                entries: self
                    .entries
                    .iter()
                    .filter(|e| e.unsaved)
                    .cloned()
                    .map(|mut e| {
                        if let Some(t) = &mut e.target {
                            t.context.clear();
                        }
                        e
                    })
                    .collect(),
                current_ref: self
                    .selected_panel
                    .clone()
                    .or_else(|| self.entries.last().map(|e| e.operation_id.clone())),
                legacy_pending: false,
            };
        }
        let mut saved = Snapshot {
            version: 1,
            entries: self.entries.clone(),
            current_ref: None,
            legacy_pending: false,
        };
        for entry in &mut saved.entries {
            if let Some(target) = &mut entry.target {
                target.context.clear();
            }
        }
        saved
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_TEXT && !text.contains('\0')
}






