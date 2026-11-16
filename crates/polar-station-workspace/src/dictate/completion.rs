//! One completion workflow for all four preview choices and automatic delivery.
//! Capture and text processing remain owned by polar-dictate; adapters execute primitives.
use super::{Effect, Engine, Entry, Phase, PreviewAction, valid_text};

#[derive(Debug, Clone, Copy)]
pub(super) struct CompletionIntent {
    pub action: PreviewAction,
    pub refine_mode: polar_dictate::DictateOutputMode,
}
impl CompletionIntent {
    pub fn sends(self) -> bool {
        self.action.sends()
    }
    pub fn refines(self) -> bool {
        self.action.refines()
    }
}
pub(super) fn available(entry: &Entry) -> bool {
    !entry.inserted
        && (matches!(
            entry.phase,
            Phase::Starting
                | Phase::Recording
                | Phase::Transcribing
                | Phase::Rewriting
                | Phase::Pending
                | Phase::Review
        ) || (entry.phase == Phase::Failed && valid_text(&entry.raw)))
}
impl Engine {
    pub(super) fn completion_available(&self, entry: &Entry) -> bool {
        available(entry)
            && !self
                .entries
                .iter()
                .any(|e| e.operation_id != entry.operation_id && e.phase.active())
    }
    pub(super) fn select_completion(
        &mut self,
        operation_id: String,
        action: PreviewAction,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();

        if !self
            .entries
            .iter()
            .find(|e| e.operation_id == operation_id)
            .is_some_and(|e| self.completion_available(e))
        {
            return effects;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.operation_id == operation_id)
        {
            if self
                .preview_actions
                .get(&operation_id)
                .is_some_and(|choice| choice.action == action)
                && entry.error.is_none()
                && matches!(entry.phase, Phase::Transcribing | Phase::Rewriting)
            {
                return effects;
            }
            let refine_mode = self
                .preview_actions
                .get(&operation_id)
                .map(|choice| choice.refine_mode)
                .unwrap_or_else(|| {
                    if entry.output_mode == Some(polar_dictate::DictateOutputMode::Transcreate) {
                        polar_dictate::DictateOutputMode::Transcreate
                    } else {
                        polar_dictate::DictateOutputMode::Refine
                    }
                });
            self.preview_actions.insert(
                operation_id.clone(),
                CompletionIntent {
                    action,
                    refine_mode,
                },
            );
            let mode = if action.refines() {
                refine_mode
            } else {
                polar_dictate::DictateOutputMode::NoRefine
            };
            entry.error = None;
            if entry.host_recording
                && entry.raw.is_empty()
                && entry.edited
                && valid_text(&entry.text)
                && !entry.phase.active()
                && !action.refines()
            {
                Self::insert(
                    entry,
                    self.preview_actions.get(&entry.operation_id).copied(),
                    &mut effects,
                );
            } else if entry.host_recording {
                let stop_capture = matches!(entry.phase, Phase::Starting | Phase::Recording);
                entry.phase = Phase::Transcribing;
                effects.push(Effect::SetOutputMode {
                    operation_id: operation_id.clone(),
                    mode,
                });
                if stop_capture {
                    effects.push(Effect::FinishCapture { operation_id });
                }
            } else if !action.refines() {
                effects.push(Effect::SetOutputMode {
                    operation_id: operation_id.clone(),
                    mode,
                });
                if !entry.edited {
                    entry.text = entry.raw.clone();
                }
                entry.refined = false;
                Self::insert(
                    entry,
                    self.preview_actions.get(&entry.operation_id).copied(),
                    &mut effects,
                );
            } else if entry.phase != Phase::Rewriting {
                if entry.refine_baseline.is_some() && !entry.edited {
                    effects.push(Effect::SetOutputMode {
                        operation_id: operation_id.clone(),
                        mode,
                    });
                    entry.text = entry.refine_baseline.clone().unwrap_or_default();
                    entry.refined = true;
                    Self::insert(
                        entry,
                        self.preview_actions.get(&entry.operation_id).copied(),
                        &mut effects,
                    );
                } else {
                    if entry.edited {
                        entry.raw = entry.text.clone();
                        entry.edited = false;
                    }
                    effects.push(Effect::SetOutputMode {
                        operation_id: operation_id.clone(),
                        mode,
                    });
                    Self::rewrite(entry, &mut effects);
                    if let Some(Effect::Rewrite { english, .. }) = effects.last_mut() {
                        *english = mode == polar_dictate::DictateOutputMode::Transcreate;
                    }
                }
            }
        }

        effects
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
                entry.error =
                    Some("Processing stopped; saved audio is available for retry.".into());
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
        Self::complete_output(
            entry,
            self.preview_actions.get(&entry.operation_id).copied(),
            &mut effects,
        );
        effects
    }
    pub(super) fn observe_text(&mut self, record: &polar_dictate::TextEditRecord) -> Vec<Effect> {
        let mut effects = Vec::new();
        let Some(entry) = self.entries.iter_mut().find(|e| {
            e.operation_id == record.id && !e.host_recording && e.phase == Phase::Rewriting
        }) else {
            return effects;
        };
        if record.running {
            return effects;
        }
        if record.delivery != polar_dictate::DeliveryState::Pending {
            return effects;
        }
        entry.raw = record.original.clone();
        entry.text = record.text().into();
        entry.error = record.error.clone();
        entry.output_mode = record.output_mode;
        entry.refined = record.result.is_some() && entry.error.is_none();
        entry.refine_baseline = record.result.clone();
        Self::complete_output(
            entry,
            self.preview_actions.get(&entry.operation_id).copied(),
            &mut effects,
        );
        effects
    }
    pub(super) fn complete_output(
        entry: &mut Entry,
        intent: Option<CompletionIntent>,
        effects: &mut Vec<Effect>,
    ) {
        if entry.error.is_some() && intent.is_some_and(CompletionIntent::refines) {
            entry.phase = Phase::Review;
        } else {
            Self::insert(entry, intent, effects);
        }
    }
    /// The sole builder of insertion effects, shared by automatic, preview and draft delivery.
    pub(super) fn insert(
        entry: &mut Entry,
        intent: Option<CompletionIntent>,
        effects: &mut Vec<Effect>,
    ) {
        entry.phase = Phase::Delivering;
        if intent.is_some() || entry.host_recording || entry.target.is_none() {
            effects.push(Effect::InsertCurrent {
                operation_id: entry.operation_id.clone(),
                text: entry.delivery_text(),
                send: intent.is_some_and(CompletionIntent::sends),
                preserve_selection: !entry.reference.is_empty(),
                source: entry.learning_source(),
            });
        } else if let Some(target) = entry.target.clone() {
            effects.push(Effect::Insert {
                operation_id: entry.operation_id.clone(),
                target,
                text: entry.delivery_text(),
            });
        }
    }
}
