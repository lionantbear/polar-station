//! Original Codex application protocol adapter. No terminal-write fallback.
use crate::{
    generic::Destination,
    input::Effect,
    terminal::{present, Selection},
    InputEngine,
};
use polar_tmux_protocol::{
    ClientApplicationMessage, CodexApiV1, CodexCommandV1, CodexRequestEnvelopeV1, CodexResultV1,
    CodexSubmissionStatusV1, HostApplicationMessage, LocalClientFrame, LocalHostFrame,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) enum Pending {
    Bindings(Selection),
    Send(String),
}
#[derive(Default)]
pub(crate) struct CodexState {
    generation: u64,
    revision: u64,
    selected: BTreeMap<String, (String, Selection)>,
    dirty: BTreeSet<String>,
    pending: BTreeMap<String, (String, Pending)>,
}
impl InputEngine {
    pub(crate) fn has_codex_selection(&self, window: &str) -> bool {
        self.codex.selected.contains_key(window)
    }
    pub(crate) fn forget_codex(&mut self, window: &str) {
        self.codex.selected.remove(window);
        self.codex.dirty.remove(window);
        self.codex.pending.retain(|_, (_, p)| !matches!(p, Pending::Bindings(s) if s.lease.window == window));
    }
    fn refresh_codex(&mut self) -> Result<Vec<Effect>, String> {
        let selected = self.codex.selected.values().cloned().collect::<Vec<_>>();
        let mut effects = Vec::new();
        for (owner, mut selection) in selected {
            let Some(view) = self.generic.view(&selection.lease.window) else { continue };
            if !view.lease.same_context(&selection.lease) { continue }
            if self.codex.pending.values().any(|(_, p)| matches!(p, Pending::Bindings(s) if s.lease.window == selection.lease.window)) {
                self.codex.dirty.insert(selection.lease.window.clone());
            } else {
                selection.lease = view.lease;
                effects.extend(self.resolve_codex(selection, owner)?);
            }
        }
        Ok(effects)
    }

    pub(crate) fn codex_request(
        &mut self,
        owner: String,
        command: CodexCommandV1,
        pending: Pending,
    ) -> Result<Effect, String> {
        if self.codex.pending.len() >= 256 {
            return Err("Too many pending Codex requests".into());
        }
        let id = polar_identity::generation::new_uuid_v7().to_string();
        let request = CodexRequestEnvelopeV1 {
            protocol_version: 1,
            api: CodexApiV1::CodexV1,
            request_id: id.clone(),
            owner_host_id: owner.clone(),
            command,
        };
        request.validate().map_err(|e| e.to_string())?;
        let frame = serde_json::to_value(LocalClientFrame::ApplicationRequest {
            request: ClientApplicationMessage::CodexV1(Box::new(request)),
        })
        .map_err(|e| e.to_string())?;
        self.codex.pending.insert(id, (owner, pending));
        Ok(Effect::Content(Box::new(
            polar_station_lgp::Effect::Request { frame },
        )))
    }
    pub(crate) fn resolve_codex(
        &mut self,
        selection: Selection,
        owner: String,
    ) -> Result<Vec<Effect>, String> {
        // Revalidation of the same selected thread is not a target change.
        // Keep the confirmed destination until its authoritative reply changes it;
        // otherwise every activity notification invalidates in-flight Dictate.
        let existing = self.generic.view(&selection.lease.window).filter(|view| {
            view.lease.same_context(&selection.lease)
                && self.codex.selected.get(&selection.lease.window)
                    .is_some_and(|(current_owner, _)| current_owner == &owner)
        });
        let view = match existing {
            Some(view) => view,
            None => self.generic.observe(
                selection.lease.window.clone(),
                selection.lease.pane.clone(),
                selection.lease.draft_key.clone(),
                Destination::Unavailable("Resolving original Codex thread…".into()),
            )?,
        };
        let mut selection = selection;
        selection.lease = view.lease.clone();
        self.codex.selected.insert(selection.lease.window.clone(), (owner.clone(), selection.clone()));
        Ok(vec![
            present(view),
            self.codex_request(
                owner,
                CodexCommandV1::GetBindings,
                Pending::Bindings(selection),
            )?,
        ])
    }
    pub(crate) fn codex_input(
        &mut self,
        input: &polar_station_lgp::Input,
    ) -> Result<Vec<Effect>, String> {
        use polar_station_lgp::Input;
        let response = match input {
            Input::Connected { generation, .. } => {
                self.codex.generation = *generation;
                self.codex.pending.clear();
                self.codex.dirty.clear();
                self.codex.revision = 0;
                // Refresh on the new transport; never replay a submission.
                let mut effects = self.refresh_codex()?;
                effects.extend(self.refresh_terminal_catalogs()?);
                return Ok(effects);
            }
            Input::Disconnected { generation } if *generation == self.codex.generation => {
                self.codex.pending.clear();
                let views = self.generic.invalidate_codex()?;
                for view in &views {
                    if let Some((_, selection)) = self.codex.selected.get_mut(&view.lease.window) {
                        selection.lease = view.lease.clone();
                    }
                }
                return Ok(views.into_iter().map(present).collect());
            }
            Input::RequestRejected {
                generation,
                request_id,
                ..
            } if *generation == self.codex.generation => {
                if let Some((_, pending)) = self.codex.pending.remove(request_id) {
                    return self.codex_result(
                        pending,
                        CodexResultV1::Error {
                            code: "request_not_admitted".into(),
                        },
                    );
                }
                return Ok(vec![]);
            }
            Input::Frame { generation, frame } if *generation == self.codex.generation => {
                match serde_json::from_value::<LocalHostFrame>(frame.clone()) {
                    Ok(LocalHostFrame::ApplicationResponse {
                        response: HostApplicationMessage::CodexV1(response),
                    }) => response,
                    Ok(LocalHostFrame::ApplicationStateChanged { api: polar_tmux_protocol::ApplicationStateApiV1::CodexV1, state_revision }) => {
                        if state_revision <= self.codex.revision { return Ok(vec![]) }
                        self.codex.revision = state_revision;
                        return self.refresh_codex();
                    }
                    _ => return Ok(vec![]),
                }
            }
            _ => return Ok(vec![]),
        };
        response.validate().map_err(|e| e.to_string())?;
        if self
            .codex
            .pending
            .get(&response.request_id)
            .is_none_or(|(owner, _)| owner != &response.owner_host_id)
        {
            return Ok(vec![]);
        }
        let (_, pending) = self
            .codex
            .pending
            .remove(&response.request_id)
            .ok_or("Missing Codex request")?;
        if let CodexResultV1::Bindings { bindings } = &response.result {
            if bindings
                .iter()
                .any(|state| state.binding.owner_host_id != response.owner_host_id)
            {
                return self.codex_result(
                    pending,
                    CodexResultV1::Error {
                        code: "binding_owner_mismatch".into(),
                    },
                );
            }
        }
        self.codex_result(pending, response.result)
    }
    fn codex_result(
        &mut self,
        pending: Pending,
        result: CodexResultV1,
    ) -> Result<Vec<Effect>, String> {
        match pending {
            Pending::Bindings(selection) => {
                let Some(current) = self.generic.view(&selection.lease.window) else {
                    return Ok(vec![]);
                };
                if !current.lease.same_context(&selection.lease) {
                    return Ok(vec![]);
                }
                if self.codex.dirty.remove(&selection.lease.window) {
                    if let Some((owner, _)) = self.codex.selected.get(&selection.lease.window).cloned() {
                        return self.resolve_codex(selection, owner);
                    }
                }
                let destination = match result {
                    CodexResultV1::Bindings { bindings } => {
                        let mut matching = bindings.into_iter().filter(|s| {
                            // The Host catalog owns original Thread identity. Terminal
                            // presentation is optional for a protocol-owned binding.
                            s.binding.session.as_ref().map_or_else(
                                || {
                                    selection.original_agent_id.as_deref()
                                        == Some(s.binding.thread_id.as_str())
                                },
                                |key| {
                                    key.execution_target.as_str() == selection.target
                                        && (key.tmux_session_id == selection.session
                                            || selection.managed_reference.as_deref()
                                                == Some(key.tmux_session_id.as_str()))
                                },
                            )
                        });
                        match (matching.next(), matching.next()) {
                            (Some(state), None) if state.can_queue => {
                                Destination::Codex(state.binding)
                            }
                            (None, _) => Destination::Unavailable("Original Codex thread is not bound".into()),
                            (Some(_), Some(_)) => Destination::Unavailable("Multiple original Codex bindings match this pane".into()),
                            (Some(state), None) => Destination::Unavailable(format!("Codex connection is not ready ({:?})", state.connection)),
                        }
                    }
                    _ => Destination::Unavailable("Unable to resolve original Codex thread".into()),
                };
                let view = self.generic.observe(
                    selection.lease.window.clone(), selection.lease.pane,
                    selection.lease.draft_key, destination,
                )?;
                if let Some((_, selected)) = self.codex.selected.get_mut(&selection.lease.window) {
                    selected.lease = view.lease.clone();
                }
                Ok(vec![present(view)])
            }
            Pending::Send(intent) => {
                if let CodexResultV1::Receipt {
                    receipt: Some(receipt),
                } = result
                {
                    if receipt.intent_id != intent {
                        return Err("Codex receipt identity mismatch".into());
                    }
                    match receipt.status {
                        CodexSubmissionStatusV1::Accepted => {
                            self.generic.resolve(&intent, receipt.clear_input)?
                        }
                        CodexSubmissionStatusV1::Rejected => {
                            self.generic.resolve(&intent, false)?
                        }
                        _ => {}
                    }
                }
                // Ambiguous/error outcomes retain the immutable intent for receipt lookup.
                let mut out = self
                    .generic
                    .active_views()
                    .into_iter()
                    .map(present)
                    .collect::<Vec<_>>();
                out.extend(
                    self.dispatch_content(polar_station_lgp::Input::ExportDrafts)?
                        .into_iter()
                        .map(|e| Effect::Content(Box::new(e))),
                );
                Ok(out)
            }
        }
    }
}
