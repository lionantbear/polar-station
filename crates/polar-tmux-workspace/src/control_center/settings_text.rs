//! A single Voice prompt draft shared by its sidebar and native editor Pane.
//! Selection, IME, undo and physical Pane ownership remain native.
use polar_dictate::VoiceSettings;
use serde::{Deserialize, Serialize};

/// Settings destinations; refinement semantics remain in Transcreation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptField { Cleanup, English, Realtime }
impl PromptField {
    fn get(self, settings: &VoiceSettings) -> Option<&String> {
        match self {
            Self::Cleanup => settings.prompts.get(polar_transcreation::RefineMode::Cleanup),
            Self::English => settings.prompts.get(polar_transcreation::RefineMode::English),
            Self::Realtime => settings.realtime_instruction.as_ref(),
        }
    }
    fn set(self, settings: &mut VoiceSettings, value: Option<String>) {
        match self {
            Self::Cleanup => settings.prompts.set(polar_transcreation::RefineMode::Cleanup, value),
            Self::English => settings.prompts.set(polar_transcreation::RefineMode::English, value),
            Self::Realtime => settings.realtime_instruction = value,
        }
    }
    fn default_prompt(self) -> &'static str {
        match self {
            Self::Cleanup => polar_transcreation::default_prompt(polar_transcreation::RefineMode::Cleanup),
            Self::English => polar_transcreation::default_prompt(polar_transcreation::RefineMode::English),
            Self::Realtime => polar_dictate::DEFAULT_REALTIME_INSTRUCTION,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsTextInput {
    Inspect,
    Snapshot {
        settings: VoiceSettings,
    },
    LoadFailed {
        error: String,
    },
    Edit {
        field: PromptField,
    },
    Change {
        text: String,
    },
    RestoreDefault,
    Refresh,
    Rebase,
    Expand,
    Collapse,
    Save,
    Close,
    Discard,
    KeepEditing,
    Complete {
        settings: Option<VoiceSettings>,
        error: Option<String>,
    },
}
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsTextEffect {
    ReadSettings,
    Save { settings: VoiceSettings },
    Expand { field: PromptField },
    Collapse,
    Close,
}
#[derive(Debug, Serialize)]
pub struct SettingsTextFieldView {
    pub field: PromptField,
    pub title: &'static str,
}
fn title(field: PromptField) -> &'static str {
    match field {
        PromptField::Cleanup => "Refine instruction",
        PromptField::English => "English rewrite instruction",
        PromptField::Realtime => "Realtime transcription instruction",
    }
}
#[derive(Debug, Serialize)]
pub struct SettingsTextActions {
    pub loading: &'static str,
    pub save: &'static str,
    pub restore: &'static str,
    pub expand: &'static str,
    pub collapse: &'static str,
    pub focus: &'static str,
    pub cancel: &'static str,
    pub discard: &'static str,
    pub keep: &'static str,
    pub unsaved: &'static str,
    pub refresh: &'static str,
    pub rebase: &'static str,
}
#[derive(Debug, Serialize)]
pub struct SettingsTextView {
    pub fields: [SettingsTextFieldView; 3],
    pub actions: SettingsTextActions,
    pub field: Option<PromptField>,
    pub title: &'static str,
    pub text: String,
    pub dirty: bool,
    pub can_edit: bool,
    pub can_save: bool,
    pub can_rebase: bool,
    pub pending: bool,
    pub loading: bool,
    pub expanded: bool,
    pub confirm_close: bool,
    pub confirmation: &'static str,
    pub error: Option<String>,
}
#[derive(Debug, Default)]
pub struct SettingsTextEngine {
    saved: Option<VoiceSettings>,
    baseline: Option<VoiceSettings>,
    field: Option<PromptField>,
    text: String,
    use_default: bool,
    expanded: bool,
    pending: bool,
    loading: bool,
    close_after_save: bool,
    next_field: Option<PromptField>,
    confirm_close: bool,
    error: Option<String>,
}
impl SettingsTextEngine {
    fn original(&self) -> Option<&String> {
        self.field
            .and_then(|f| self.baseline.as_ref().and_then(|s| f.get(s)))
    }
    fn value(&self) -> Option<String> {
        if self.use_default {
            None
        } else {
            Some(self.text.clone())
        }
    }
    fn dirty(&self) -> bool {
        self.field.is_some() && self.value().as_ref() != self.original()
    }
    fn load(&mut self, field: PromptField) {
        self.field = Some(field);
        self.baseline.clone_from(&self.saved);
        let original = self.original().cloned();
        self.use_default = original.is_none();
        self.text = if self.saved.is_none() {
            String::new()
        } else {
            original.unwrap_or_else(|| field.default_prompt().into())
        };
        self.error = None;
    }
    fn finish_leave(&mut self, effects: &mut Vec<SettingsTextEffect>) {
        self.confirm_close = false;
        self.close_after_save = false;
        if let Some(field) = self.next_field.take() {
            self.load(field);
            if self.expanded {
                effects.push(SettingsTextEffect::Expand { field })
            }
        } else {
            self.field = None;
            self.saved = None;
            self.loading = false;
            self.baseline = None;
            self.expanded = false;
            effects.push(SettingsTextEffect::Close);
        }
    }
    pub fn view(&self) -> SettingsTextView {
        SettingsTextView {
            actions: SettingsTextActions {
                loading: "Loading instruction…",
                save: if self.pending { "Saving…" } else { "Save" },
                restore: "Restore default",
                expand: "Expand editor",
                collapse: "Return to sidebar",
                focus: "Focus editor",
                cancel: "Cancel",
                discard: "Discard",
                keep: "Keep editing",
                unsaved: "Unsaved",
                refresh: "Refresh saved settings",
                rebase: "Keep my instruction using the refreshed settings",
            },
            fields: [
                SettingsTextFieldView {
                    field: PromptField::Cleanup,
                    title: title(PromptField::Cleanup),
                },
                SettingsTextFieldView {
                    field: PromptField::English,
                    title: title(PromptField::English),
                },
                SettingsTextFieldView {
                    field: PromptField::Realtime,
                    title: title(PromptField::Realtime),
                },
            ],
            field: self.field,
            title: self.field.map(title).unwrap_or("Text editor"),
            text: self.text.clone(),
            dirty: self.dirty(),
            can_rebase: !self.pending
                && !self.loading
                && self
                    .saved
                    .as_ref()
                    .zip(self.baseline.as_ref())
                    .is_some_and(|(saved, baseline)| saved.revision > baseline.revision),
            can_edit: self.baseline.is_some() && !self.pending && !self.loading,
            can_save: self.baseline.is_some() && self.dirty() && !self.pending && !self.loading,
            pending: self.pending,
            loading: self.loading,
            expanded: self.expanded,
            confirm_close: self.confirm_close,
            confirmation: if self.next_field.is_some() {
                "Save changes before switching instructions?"
            } else {
                "Save changes before closing?"
            },
            error: self.error.clone(),
        }
    }
    pub fn dispatch(
        &mut self,
        input: SettingsTextInput,
    ) -> (SettingsTextView, Vec<SettingsTextEffect>) {
        let mut effects = vec![];
        match input {
            SettingsTextInput::Inspect => {}
            SettingsTextInput::Snapshot { settings } => {
                if self
                    .saved
                    .as_ref()
                    .is_some_and(|saved| settings.revision < saved.revision)
                {
                    return (self.view(), effects);
                }
                self.saved = Some(settings);
                self.loading = false;
                if !self.dirty() && !self.pending {
                    if let Some(field) = self.field {
                        self.load(field)
                    }
                }
            }
            SettingsTextInput::Edit { field } if !self.pending && !self.loading => {
                if self.field == Some(field) {
                    if self.saved.is_none() {
                        self.loading = true;
                        effects.push(SettingsTextEffect::ReadSettings);
                    } else if self.expanded {
                        effects.push(SettingsTextEffect::Expand { field })
                    }
                } else if self.dirty() {
                    self.next_field = Some(field);
                    self.confirm_close = true;
                } else {
                    self.load(field);
                    if self.saved.is_none() {
                        self.loading = true;
                        effects.push(SettingsTextEffect::ReadSettings)
                    } else if self.expanded {
                        effects.push(SettingsTextEffect::Expand { field })
                    }
                }
            }
            SettingsTextInput::LoadFailed { error } => {
                self.loading = false;
                self.error = Some(error)
            }
            SettingsTextInput::Change { text } if self.view().can_edit => {
                self.text = text;
                self.use_default = false;
                self.error = None;
            }
            SettingsTextInput::Refresh
                if !self.pending && !self.loading && self.field.is_some() =>
            {
                self.loading = true;
                effects.push(SettingsTextEffect::ReadSettings);
            }
            SettingsTextInput::Rebase if self.view().can_rebase => {
                self.baseline.clone_from(&self.saved);
                self.error = None;
            }
            SettingsTextInput::RestoreDefault if self.view().can_edit => {
                self.use_default = true;
                if let Some(field) = self.field {
                    self.text = field.default_prompt().into()
                }
            }
            SettingsTextInput::Expand => {
                if let Some(field) = self.field {
                    self.expanded = true;
                    effects.push(SettingsTextEffect::Expand { field })
                }
            }
            SettingsTextInput::Collapse => {
                self.expanded = false;
                effects.push(SettingsTextEffect::Collapse)
            }
            SettingsTextInput::Save if self.view().can_save => {
                if let (Some(field), Some(mut candidate)) = (self.field, self.baseline.clone()) {
                    field.set(&mut candidate, self.value());
                    match candidate.validate() {
                        Ok(()) => {
                            self.pending = true;
                            self.error = None;
                            self.close_after_save = self.confirm_close;
                            effects.push(SettingsTextEffect::Save {
                                settings: candidate,
                            })
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                } else {
                    self.error = Some("Voice settings have not been loaded.".into())
                }
            }
            SettingsTextInput::Complete { settings, error } if self.pending => {
                self.pending = false;
                if let Some(error) = error {
                    self.error = Some(error);
                    self.close_after_save = false;
                } else if let Some(settings) = settings {
                    self.saved = Some(settings);
                    if let Some(field) = self.field {
                        self.load(field)
                    }
                    self.confirm_close = false;
                    if self.close_after_save {
                        self.finish_leave(&mut effects)
                    }
                } else {
                    self.error = Some("Save was not confirmed; the draft is retained.".into());
                    self.close_after_save = false;
                }
            }
            SettingsTextInput::Close if self.pending => {
                self.next_field = None;
                self.confirm_close = true;
                self.close_after_save = true;
            }
            SettingsTextInput::Close => {
                self.next_field = None;
                if self.dirty() {
                    self.confirm_close = true
                } else {
                    self.finish_leave(&mut effects)
                }
            }
            SettingsTextInput::Discard if !self.pending => self.finish_leave(&mut effects),
            SettingsTextInput::KeepEditing => {
                self.confirm_close = false;
                self.close_after_save = false;
                self.next_field = None;
            }
            _ => {}
        }
        (self.view(), effects)
    }
}

