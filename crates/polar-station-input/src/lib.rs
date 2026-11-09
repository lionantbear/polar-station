//! Shared Station input use cases. Content behavior stays in its existing owner.
//!
//! This first migration step routes the legacy content ABI through one input
//! owner. Generic terminal input and native editor migration follow separately.
use polar_station_lgp::{Effect, Engine, Input};
mod codex;
pub mod generic;
pub mod input;
pub mod terminal;

/// Application-scoped owner. Native windows borrow the same instance.
#[derive(Default)]
pub struct InputEngine {
    content: Engine,
    terminal: terminal::TerminalState,
    codex: codex::CodexState,
    snapshot_revision: u64,
    content_windows: std::collections::BTreeMap<String, String>,
    pub generic: generic::GenericDrafts,
}

impl InputEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared native entrypoint; old content commands retain their wire shape.
    pub fn dispatch(&mut self, input: input::Input) -> Result<Vec<input::Effect>, String> {
        use input::{Effect, GenericEffect, GenericInput, Input};
        match input {
            Input::Content(value) => {
                let mut out = self.codex_input(&value)?;
                out.extend(
                    self.dispatch_content(value)?
                        .into_iter()
                        .map(|effect| Effect::Content(Box::new(effect))),
                );
                Ok(out)
            }
            Input::Generic(value) => {
                let view = match value {
                    GenericInput::SelectTerminal {
                        window_id,
                        pane_id,
                        target_id,
                        session_id,
                    } => return self.select_terminal(window_id, pane_id, target_id, session_id),
                    GenericInput::SessionCompleted {
                        request_id,
                        value,
                        error,
                    } => return self.session_completed(request_id, value, error),
                    GenericInput::QuoteSelection { lease, text } => {
                        self.generic.pending_submission(&lease)?; // exact live context and draft revision
                        if text.is_empty() || text.len() > 1024 * 1024 {
                            return Err("Invalid terminal selection".into());
                        }
                        return Ok(vec![Effect::Generic(GenericEffect::InsertText {
                            lease,
                            text,
                        })]);
                    }
                    GenericInput::RefreshInput { lease } => return self.refresh_input(lease),
                    GenericInput::SubmitInput { lease, press_enter } => {
                        return self.submit_terminal(lease, press_enter)
                    }
                    GenericInput::SnapshotStored { revision, success } => {
                        return self.snapshot_stored(revision, success)
                    }
                    GenericInput::ObserveInput {
                        window_id,
                        pane_id,
                        draft_key,
                        destination,
                    } => self
                        .generic
                        .observe(window_id, pane_id, draft_key, destination)?,
                    GenericInput::EditInput { lease, text } => self.generic.edit(&lease, text)?,
                };
                Ok(vec![Effect::Generic(GenericEffect::InputBar { view })])
            }
        }
    }

    /// Compatibility boundary for existing Chat, Idea and Inquiry adapters.
    /// No second content engine, draft copy or request queue is created.
    pub fn dispatch_content(&mut self, input: Input) -> Result<Vec<Effect>, String> {
        match input {
            Input::ExportDrafts => {
                self.snapshot_revision = self
                    .snapshot_revision
                    .checked_add(1)
                    .ok_or("Snapshot revision exhausted")?;
                let mut effects = self.content.dispatch(Input::ExportDrafts)?;
                for effect in &mut effects {
                    if let Effect::Drafts { value } = effect {
                        *value = serde_json::json!({"schema":"polar.station.input.v1", "snapshot_revision":self.snapshot_revision, "content_drafts":value, "generic_drafts":self.generic.snapshot()?});
                        if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 1024 * 1024
                        {
                            return Err("Combined input drafts exceed storage budget".into());
                        }
                    }
                }
                Ok(effects)
            }
            Input::RestoreDrafts { drafts } if drafts["schema"] == "polar.station.input.v1" => {
                let mut generic = generic::GenericDrafts::default();
                generic.restore_drafts(drafts["generic_drafts"].clone())?;
                // Validate all domains before replacing the live content owner.
                let mut content = Engine::new();
                content.dispatch(Input::RestoreDrafts {
                    drafts: drafts["content_drafts"].clone(),
                })?;
                let effects = self.content.dispatch(Input::RestoreDrafts {
                    drafts: drafts["content_drafts"].clone(),
                })?;
                self.generic = generic;
                Ok(effects)
            }
            Input::WindowClosed { window_id } => {
                self.leave_terminal(&window_id);
                self.content_windows
                    .retain(|_, window| window != &window_id);
                self.content.dispatch(Input::WindowClosed { window_id })
            }
            value => {
                let opened = match &value {
                    Input::Open {
                        pane_id, window_id, ..
                    }
                    | Input::OpenIdea {
                        pane_id, window_id, ..
                    } => Some((pane_id.clone(), window_id.clone())),
                    _ => None,
                };
                let selected = if let Input::SelectionChanged { pane_id } = &value {
                    self.content_windows.get(pane_id).cloned()
                } else {
                    None
                };
                let effects = self.content.dispatch(value)?;
                if let Some((pane, window)) = opened {
                    self.content_windows.insert(pane, window);
                }
                if let Some(window) = selected {
                    self.leave_terminal(&window);
                }
                Ok(effects)
            }
        }
    }
}
