//! Shared Dictate policy. Platform adapters own audio and editor handles, never routing.
pub mod application;
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
        }
    }
}
const MAX_TEXT: usize = 64 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_HISTORY_TEXT: usize = 512 * 1024;

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
    pub edited: bool,
}
impl Entry {
    fn status(&self) -> std::borrow::Cow<'_, str> {
        if let Some(error) = self.error.as_deref() {
            if !self.raw.is_empty() {
                format!("Refinement failed; original retained · {error}").into()
            } else { error.into() }
        } else if self.refined {
            format!("Refinement succeeded · {}", self.phase.label()).into()
        } else { self.phase.label().into() }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    TranscriptPreview { operation_id: String, text: String },
    ShowPanel { operation_id: String },
    HidePanel { operation_id: String },
    EditDraft { operation_id: String, text: String },
    InsertDraft { operation_id: String },
    RetrySaved { operation_id: String },
    ShortcutBinding { trigger: String },
    ShortcutMode { toggle: bool },
    ShortcutStatus {
        status: ShortcutStatus,
    },
    RefineText {
        operation_id: String,
        target: Target,
        text: String,
    },
    Start {
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
    InsertCurrent { operation_id: String, text: String },
    RetryTranscribe { operation_id: String },
    Capture {
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
    entries: Vec<Entry>,
    shortcut_status: ShortcutStatus,
    shortcut_trigger: String,
    shortcut_toggle: bool,
    hidden_panel: Option<String>,
    selected_panel: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    version: u32,
    entries: Vec<Entry>,
}
impl Engine {
    pub fn history(&self) -> Vec<HistoryEntry<'_>> {
        self.entries
            .iter()
            .rev()
            .filter(|entry| !entry.text.trim().is_empty() || !entry.raw.trim().is_empty())
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
        let has_room = self.entries.iter().filter(|e| e.phase.active() || !e.raw.trim().is_empty() || !e.text.trim().is_empty()).count() < MAX_ENTRIES
            && self
                .entries
                .iter()
                .map(|e| e.raw.len() + e.text.len())
                .sum::<usize>()
                <= MAX_HISTORY_TEXT - 2 * MAX_TEXT;
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
            enabled: recording || (active.is_none() && has_room),
            operation_id: active.map(|e| e.operation_id.as_str()),
            cancel_id: active
                .filter(|e| e.phase != Phase::Delivering)
                .map(|e| e.operation_id.as_str()),
            setup_required: latest
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
            status: if !has_room && active.is_none() {
                "Review or dismiss Dictate history before recording".into()
            } else if let Some(entry) = latest {
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
            shortcut_hint: if self.shortcut_trigger.is_empty() { "Configure Dictate shortcut in Voice settings".into() } else if self.shortcut_toggle { format!("Press {} to record; press again to transcribe", self.shortcut_trigger) } else { format!("Hold {} to dictate", self.shortcut_trigger) },
            history_title: "Dictate drafts",
            review_label: "Review result",
            copy_label: "Copy",
            original_label: "Copy Original",
            dismiss_label: "Dismiss",
            cancel_label: "Cancel",
        }
    }
    pub fn panel(&self) -> serde_json::Value {
        let Some(entry) = self.entries.iter().find(|e| e.phase.active()).or_else(|| self.selected_panel.as_ref().and_then(|id| self.entries.iter().find(|entry| &entry.operation_id == id))).or_else(|| self.entries.last()) else { return serde_json::Value::Null; };
        if self.hidden_panel.as_deref() == Some(&entry.operation_id)
            || entry.phase == Phase::Cancelled
            || (entry.phase == Phase::Failed && entry.saved_samples == 0 && entry.text.is_empty()) {
            return serde_json::Value::Null;
        }
        let active = entry.phase.active();
        serde_json::json!({
            "operation_id":entry.operation_id,"phase":entry.phase,"text":entry.text,
            "status":if entry.phase == Phase::Recording { self.presentation().status } else { entry.status() },
            "can_stop":matches!(entry.phase,Phase::Starting|Phase::Recording),
            "can_cancel":active && entry.phase != Phase::Delivering,
            "can_copy":!entry.text.is_empty(),
            "can_edit":!active && entry.phase != Phase::Delivered && !entry.raw.is_empty(),
            "can_insert":!active && !matches!(entry.phase,Phase::Delivered|Phase::Unknown) && valid_text(&entry.text),
            "can_retry":entry.phase == Phase::Failed && entry.host_recording && entry.saved_samples > 0,
            "can_original":entry.phase == Phase::Rewriting && !entry.raw.is_empty(),
            "hide_after_ms":if entry.phase == Phase::Delivered { 1600 } else { 0 },
            "title":"Dictate","stop_label":"Stop & Save","cancel_label":"Cancel",
            "copy_label":"Copy","edit_label":"Edit","insert_label":"Insert in current input",
            "retry_label":"Retry transcription","close_label":"Close","original_label":"Use Original"
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
        let raw = recording
            .original()
            .ok_or("Saved transcript is incomplete")?;
        let text = recording.text().ok_or("Saved transcript is incomplete")?;
        let retained = self
            .entries
            .iter()
            .filter(|entry| entry.operation_id != recording.id);
        if raw.len() > MAX_TEXT
            || text.len() > MAX_TEXT
            || retained.clone().count() >= MAX_ENTRIES
            || retained
                .map(|entry| entry.raw.len() + entry.text.len())
                .sum::<usize>()
                + raw.len()
                + text.len()
                > MAX_HISTORY_TEXT
        {
            return Err("Review or dismiss Dictate history before recovering a recording".into());
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
            host_recording: true,
            edited: false,
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
    pub fn dispatch(&mut self, input: Input) -> Result<Vec<Effect>, String> {
        let mut effects = Vec::new();
        match input {
            Input::ShowPanel { operation_id } => {
                if self.entries.iter().any(|entry| entry.operation_id == operation_id) {
                    self.selected_panel = Some(operation_id);
                    self.hidden_panel = None;
                }
            }
            Input::HidePanel { operation_id } => {
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.operation_id == operation_id && entry.phase.active() && entry.phase != Phase::Delivering) {
                    entry.phase = Phase::Cancelled;
                    effects.push(Effect::Cancel { operation_id: operation_id.clone() });
                }
                self.hidden_panel = Some(operation_id);
            },
            Input::EditDraft { operation_id, text } => {
                let retained = self.entries.iter().map(|entry| entry.raw.len() + if entry.operation_id == operation_id { 0 } else { entry.text.len() }).sum::<usize>();
                if retained + text.len() > MAX_HISTORY_TEXT || text.len() > MAX_TEXT || text.contains('\0') { return Err("Dictate draft exceeds text limit".into()); }
                if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == operation_id && !e.phase.active() && e.phase != Phase::Delivered && !e.raw.is_empty()) {
                    entry.text = text;
                    entry.edited = true;
                    entry.target = None;
                    entry.phase = Phase::Pending;
                }
            }
            Input::InsertDraft { operation_id } => {
                if self.entries.iter().any(|e| e.phase.active()) { return Ok(effects); }
                if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == operation_id && !matches!(e.phase,Phase::Delivered|Phase::Unknown) && valid_text(&e.text)) {
                    entry.target = None;
                    entry.phase = Phase::Delivering;
                    effects.push(Effect::InsertCurrent { operation_id: entry.operation_id.clone(), text: entry.text.clone() });
                    self.hidden_panel = None;
                }
            }
            Input::RetrySaved { operation_id } => {
                if self.entries.iter().any(|e| e.phase.active()) { return Ok(effects); }
                if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == operation_id && e.phase == Phase::Failed && e.saved_samples > 0 && e.host_recording) {
                    entry.phase = Phase::Transcribing;
                    entry.error = None;
                    self.hidden_panel = None;
                    effects.push(Effect::RetryTranscribe { operation_id });
                }
            }
            Input::TranscriptPreview { operation_id, text } => {
                if text.len() > 64 * 1024 || text.contains('\0') { return Err("Invalid transcript preview".into()); }
                if let Some(entry) = self.entries.iter_mut().find(|e| e.operation_id == operation_id
                    && matches!(e.phase, Phase::Starting | Phase::Recording | Phase::Transcribing)) {
                    entry.text = text;
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
            Input::ShortcutMode { toggle } => { self.shortcut_toggle = toggle; }
            Input::ShortcutBinding { trigger } => {
                if trigger.len() > 256 || trigger.chars().any(char::is_control) { return Err("Invalid shortcut description".into()); }
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
            Input::RefineText { operation_id, target, text } => {
                if operation_id.is_empty() || operation_id.len() > 128 || !target.usable() || !valid_text(&text) {
                    return Err("Select a nonempty editable draft to refine".into());
                }
                if self.entries.iter().any(|e| e.operation_id == operation_id) { return Ok(effects); }
                if !self.presentation().enabled || self.entries.iter().any(|e| e.phase.active()) {
                    return Err("Finish the active Dictate operation first".into());
                }
                self.entries.retain(|e| e.phase.active() || !e.raw.trim().is_empty() || !e.text.trim().is_empty());
                let mut entry = Entry { operation_id, target: Some(target), raw: text.clone(), text,
                    phase: Phase::Rewriting, error: None, saved_samples: 0, host_recording: false, refined: false, edited: false };
                Self::rewrite(&mut entry, &mut effects);
                self.entries.push(entry);
            }
            Input::Start {
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
                self.entries.retain(|e| e.phase.active() || !e.raw.trim().is_empty() || !e.text.trim().is_empty());
                if self.entries.len() >= MAX_ENTRIES {
                    return Err("Review or dismiss Dictate history before recording".into());
                }
                if self
                    .entries
                    .iter()
                    .map(|e| e.raw.len() + e.text.len())
                    .sum::<usize>()
                    > MAX_HISTORY_TEXT - 2 * MAX_TEXT
                {
                    return Err("Review or dismiss Dictate history before recording".into());
                }
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
                self.selected_panel = None;
                effects.push(Effect::Capture {
                    operation_id: operation_id.clone(),
                    maximum_ms: MAX_RECORDING_MS,
                    context: target.as_ref().map(|t| t.context.clone()),
                });
                self.entries.push(Entry {
                    host_recording: true,
                    edited: false,
                    refined: false,
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
                if let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.phase == Phase::Pending && e.target.is_none() && valid_text(&e.text))
                {
                    entry.target = Some(target);
                    if entry.refined || entry.edited { Self::insert(entry, &mut effects); }
                    else { Self::rewrite(entry, &mut effects); }
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
                            effects.push(Effect::Transcribe {
                                operation_id: entry.operation_id.clone(),
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
                        if entry.refined {
                            if let Some(text) = text { entry.text = text; }
                        } else { entry.text = entry.raw.clone(); }
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
                            DeliveryOutcome::NoTarget => { entry.target = None; Phase::Pending },
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
            effects.push(Effect::InsertCurrent { operation_id: entry.operation_id.clone(), text: entry.text.clone() });
            return;
        }
        if let Some(target) = entry.target.clone() {
            entry.phase = Phase::Delivering;
            effects.push(Effect::Insert {
                operation_id: entry.operation_id.clone(),
                target,
                text: entry.text.clone(),
            });
        }
    }
    /// A restart never replays an uncertain insertion or resumes microphone capture.
    pub fn restore(mut saved: Snapshot) -> Result<Self, String> {
        if saved.version != 1 {
            return Err("Unsupported Dictate history version".into());
        }
        if saved.entries.len() > MAX_ENTRIES
            || saved
                .entries
                .iter()
                .map(|e| e.raw.len() + e.text.len())
                .sum::<usize>()
                > MAX_HISTORY_TEXT
        {
            return Err("Dictate history is too large".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for entry in &mut saved.entries {
            if entry.operation_id.is_empty()
                || entry.operation_id.len() > 128
                || !ids.insert(entry.operation_id.clone())
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
                ) && (!valid_text(&entry.raw) || (!valid_text(&entry.text) && !entry.edited)))
            {
                return Err("Invalid Dictate history".into());
            }
            if let Some(target) = &mut entry.target {
                target.context.clear();
            }
            entry.phase = match entry.phase {
                Phase::Delivering => Phase::Unknown,
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
        let mut saved = Snapshot {
            version: 1,
            entries: self.entries.clone(),
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




