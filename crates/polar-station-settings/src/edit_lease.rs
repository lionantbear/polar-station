//! Transient edit protection for the original native Settings controls.
//! Raw text, IME and undo stay in those controls; persistence stays in Repository.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsEditSection {
    Appearance,
    Attention,
    Monitoring,
    SessionSummary,
    Voice,
    VoiceRetention,
    Imessage,
    TmuxStatusBar,
}
impl SettingsEditSection {
    const ALL: [Self; 8] = [
        Self::Appearance,
        Self::Attention,
        Self::Monitoring,
        Self::SessionSummary,
        Self::Voice,
        Self::VoiceRetention,
        Self::Imessage,
        Self::TmuxStatusBar,
    ];
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsEditInput {
    Observe {
        revision: u64,
    },
    Disconnected,
    Rebase,
    Edit {
        section: SettingsEditSection,
    },
    BeginSave {
        section: SettingsEditSection,
        request_id: String,
    },
    CompleteSave {
        request_id: String,
        accepted: bool,
        revision: Option<u64>,
    },
}
#[derive(Debug, Serialize)]
pub struct SettingsEditSectionView {
    pub section: SettingsEditSection,
    pub replace: bool,
    pub revision: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct SettingsEditView {
    pub revision: Option<u64>,
    pub can_rebase: bool,
    pub rebase_label: &'static str,
    pub sections: Vec<SettingsEditSectionView>,
    pub save_revision: Option<u64>,
}
#[derive(Debug, Clone, Copy)]
struct Edit {
    revision: u64,
    generation: u64,
}
#[derive(Debug)]
struct Pending {
    section: SettingsEditSection,
    request_id: String,
    generation: u64,
}
#[derive(Debug, Default)]
pub struct SettingsEditLease {
    revision: Option<u64>,
    edits: BTreeMap<SettingsEditSection, Edit>,
    pending: Option<Pending>,
}
impl SettingsEditLease {
    pub fn dispatch(&mut self, input: SettingsEditInput) -> SettingsEditView {
        let mut save_revision = None;
        match input {
            SettingsEditInput::Observe { revision } => self.observe(revision),
            SettingsEditInput::Disconnected => self.pending = None,
            SettingsEditInput::Rebase => {
                if self.pending.is_none() {
                    if let Some(revision) = self.revision {
                        for edit in self.edits.values_mut() {
                            edit.revision = revision;
                        }
                    }
                }
            }
            SettingsEditInput::Edit { section } => {
                if let Some(revision) = self.revision {
                    let edit = self.edits.entry(section).or_insert(Edit {
                        revision,
                        generation: 0,
                    });
                    edit.generation = edit.generation.saturating_add(1);
                }
            }
            SettingsEditInput::BeginSave {
                section,
                request_id,
            } => {
                if self.pending.is_none() && !request_id.is_empty() {
                    if let Some(revision) = self.revision {
                        let edit = *self.edits.entry(section).or_insert(Edit {
                            revision,
                            generation: 0,
                        });
                        self.pending = Some(Pending {
                            section,
                            request_id,
                            generation: edit.generation,
                        });
                        save_revision = Some(edit.revision);
                    }
                }
            }
            SettingsEditInput::CompleteSave {
                request_id,
                accepted,
                revision,
            } => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.request_id == request_id)
                {
                    let pending = self.pending.take().expect("matching pending edit");
                    if accepted {
                        if let Some(revision) = revision {
                            if let Some(edit) = self.edits.get_mut(&pending.section) {
                                if edit.generation == pending.generation {
                                    self.edits.remove(&pending.section);
                                } else {
                                    // Later input is based on the just-acknowledged save.
                                    edit.revision = revision;
                                }
                            }
                            self.observe(revision);
                        }
                    }
                }
            }
        }
        SettingsEditView {
            revision: self.revision,
            can_rebase: self.pending.is_none()
                && self.revision.is_some_and(|revision| {
                    self.edits.values().any(|edit| edit.revision < revision)
                }),
            rebase_label: "Keep edits with current settings",
            save_revision,
            sections: SettingsEditSection::ALL
                .into_iter()
                .map(|section| {
                    let edit = self.edits.get(&section);
                    SettingsEditSectionView {
                        section,
                        replace: edit.is_none(),
                        revision: edit.map(|e| e.revision).or(self.revision),
                    }
                })
                .collect(),
        }
    }
    fn observe(&mut self, revision: u64) {
        self.revision = Some(
            self.revision
                .map_or(revision, |current| current.max(revision)),
        );
    }
}

