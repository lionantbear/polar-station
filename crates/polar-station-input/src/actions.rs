//! Common user actions; native adapters never choose a domain submission path.
use crate::{input::Effect, terminal::present, InputEngine};
use polar_station_lgp::Input as ContentInput;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputAction {
    Edit {
        text: String,
    },
    Submit {
        created_at_secs: u64,
        #[serde(default)]
        text: Option<String>,
    },
    Refresh,
}

impl InputEngine {
    pub(crate) fn active_action(
        &mut self,
        window: String,
        pane: String,
        action: InputAction,
    ) -> Result<Vec<Effect>, String> {
        // One synchronous Rust action captures the editor before preparing the
        // immutable request. Native adapters never split edit and send callbacks.
        if let InputAction::Submit {
            created_at_secs,
            text: Some(text),
        } = action
        {
            let mut effects =
                self.active_action(window.clone(), pane.clone(), InputAction::Edit { text })?;
            match self.active_action(
                window,
                pane,
                InputAction::Submit {
                    created_at_secs,
                    text: None,
                },
            ) {
                Ok(sent) => effects.extend(sent),
                Err(message) => effects.push(Effect::Content(Box::new(
                    polar_station_lgp::Effect::Error { message },
                ))),
            }
            return Ok(effects);
        }
        // Selection is read from the existing owner, never reconstructed from
        // native labels, optional JSON fields or cached connection status.
        if let Some(view) = self.generic.view(&window) {
            if view.lease.pane != pane {
                return Err("Input Pane changed".into());
            }
            return match action {
                InputAction::Edit { text } => {
                    Ok(vec![present(self.generic.edit(&view.lease, text)?)])
                }
                InputAction::Submit { .. } => self.submit_terminal(view.lease, true),
                InputAction::Refresh => self.refresh_input(view.lease),
            };
        }
        if self.content.selected_input_pane(&window) != Some(pane.as_str()) {
            return Err("Input Pane changed".into());
        }
        let input = match action {
            InputAction::Edit { text } => ContentInput::Draft {
                window_id: window,
                text,
            },
            InputAction::Submit {
                created_at_secs, ..
            } => ContentInput::Send {
                window_id: window,
                created_at_secs,
            },
            InputAction::Refresh => ContentInput::Retry { pane_id: pane },
        };
        self.dispatch_content(input).map(|effects| {
            effects
                .into_iter()
                .map(|e| Effect::Content(Box::new(e)))
                .collect()
        })
    }
}
