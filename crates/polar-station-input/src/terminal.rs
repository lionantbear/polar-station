//! Terminal selection and submission use cases; execution remains in the Host adapter.
use crate::{
    generic::{Destination, Lease, Submission},
    input::{Effect, GenericEffect},
    InputEngine,
};
use polar_station_session::{Catalog, DeliveryStatus, ListRequest, LocalRequest, Receipt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionJob {
    pub request_id: String,

    pub request: LocalRequest,
}
#[derive(Clone)]
pub(crate) struct Selection {
    pub(crate) lease: Lease,
    pub(crate) target: String,
    pub(crate) session: String,
    pub(crate) managed_reference: Option<String>,
    pub(crate) original_agent_id: Option<String>,
}
enum Pending {
    Selection(Selection),
    Send {
        intent: String,
        reference: polar_station_session::SessionReference,
    },
}
#[derive(Default)]
pub(crate) struct TerminalState {
    selected: BTreeMap<String, Selection>,
    pending: BTreeMap<String, Pending>,
    saving: BTreeMap<String, u64>,
}
fn id() -> String {
    polar_identity::generation::new_uuid_v7().to_string()
}
pub(crate) fn present(view: crate::generic::GenericView) -> Effect {
    Effect::Generic(GenericEffect::InputBar { view })
}
fn request(job: SessionJob) -> Effect {
    Effect::Generic(GenericEffect::SessionRequest {
        request: Box::new(job),
    })
}

impl InputEngine {
    pub(crate) fn refresh_terminal_catalogs(&mut self) -> Result<Vec<Effect>, String> {
        let leases = self.terminal.selected.keys()
            .filter(|window| !self.has_codex_selection(window))
            .filter_map(|window| self.generic.view(window).map(|view| view.lease))
            .collect::<Vec<_>>();
        let mut effects = Vec::new();
        for lease in leases {
            effects.extend(self.refresh_input(lease)?);
        }
        Ok(effects)
    }

    pub(crate) fn leave_terminal(&mut self, window: &str) {
        self.forget_codex(window);
        self.generic.close_window(window);
        self.terminal.selected.remove(window);
        self.terminal.pending.retain(|_,pending|!matches!(pending,Pending::Selection(selection) if selection.lease.window==window));
    }

    pub(crate) fn select_terminal(
        &mut self,
        window: String,
        pane: String,
        target: String,
        session: String,
    ) -> Result<Vec<Effect>, String> {
        for value in [&target, &session] {
            polar_tmux_protocol::validate_stable_identifier(value, "terminal target")
                .map_err(|e| e.to_string())?;
        }
        if let Some(current) = self.terminal.selected.get(&window) {
            if current.lease.pane == pane && current.target == target && current.session == session
            {
                if let Some(view) = self.generic.view(&window) {
                    return Ok(vec![present(view)]);
                }
            }
        }
        self.forget_codex(&window);
        let key = serde_json::to_string(&(&pane, &target, &session)).map_err(|e| e.to_string())?;
        let view = self.generic.observe(
            window.clone(),
            pane,
            key,
            Destination::Unavailable("Resolving terminal input…".into()),
        )?;
        let selection = Selection {
            lease: view.lease.clone(),
            target: target.clone(),
            session,
            managed_reference: None,
            original_agent_id: None,
        };
        self.terminal.pending.retain(
            |_, pending| !matches!(pending,Pending::Selection(old) if old.lease.window==window),
        );
        self.terminal
            .selected
            .insert(window.clone(), selection.clone());
        let request_id = id();
        self.terminal
            .pending
            .insert(request_id.clone(), Pending::Selection(selection));
        let mut out = self
            .content
            .dispatch(polar_station_lgp::Input::Deselect { window_id: window })?
            .into_iter()
            .map(|e| Effect::Content(Box::new(e)))
            .collect::<Vec<_>>();
        out.push(present(view));
        out.push(request(SessionJob {
            request_id,

            request: LocalRequest::List {
                input: ListRequest {
                    target_id: Some(target),
                },
            },
        }));
        Ok(out)
    }

    pub(crate) fn session_completed(
        &mut self,
        request_id: String,
        value: Option<serde_json::Value>,
        error: Option<String>,
    ) -> Result<Vec<Effect>, String> {
        let Some(pending) = self.terminal.pending.remove(&request_id) else {
            return Ok(vec![]);
        };
        match pending {
            Pending::Selection(selection) => {
                let Some(current) = self.generic.view(&selection.lease.window) else {
                    return Ok(vec![]);
                };
                if !current.lease.same_context(&selection.lease) {
                    return Ok(vec![]);
                }
                let destination = if let Some(error) = error {
                    Destination::Unavailable(error)
                } else {
                    let catalog: Catalog =
                        serde_json::from_value(value.ok_or("Missing Session catalog")?)
                            .map_err(|e| e.to_string())?;
                    let mut matches = catalog.sessions.iter().filter(|s| {
                        s.target_id == selection.target && (s.session_id == selection.session
                            || s.managed_reference.as_deref() == Some(selection.session.as_str()))
                    });
                    let session = matches.next();
                    if matches.next().is_some() {
                        Destination::Unavailable("Original Session reference is ambiguous".into())
                    } else if let Some(session) = session {
                        self.generic.label(
                            &selection.lease.window,
                            format!("{} · {}", session.name, selection.target),
                        );
                        if session.kind == "codex_plane" {
                            let mut selection = selection;
                            selection.session = session.session_id.clone();
                            selection.managed_reference = session.managed_reference.clone();
                            selection.original_agent_id = session.original_agent_id.clone();
                            return self.resolve_codex(selection, catalog.source_host_id);
                        } else if let Some(pane) = session
                            .panes
                            .iter()
                            .find(|p| p.active && p.available)
                            .or_else(|| {
                                (session.panes.len() == 1)
                                    .then(|| &session.panes[0])
                                    .filter(|p| p.available)
                            })
                        {
                            Destination::Terminal(pane.reference.clone())
                        } else {
                            Destination::Unavailable("No active terminal input endpoint".into())
                        }
                    } else {
                        Destination::Unavailable("Original Session is unavailable".into())
                    }
                };
                let view = self.generic.observe(
                    selection.lease.window,
                    selection.lease.pane,
                    selection.lease.draft_key,
                    destination,
                )?;
                Ok(vec![present(view)])
            }
            Pending::Send { intent, reference } => {
                let mut refresh = Vec::new();
                if error.is_none() {
                    let receipt: Receipt =
                        serde_json::from_value(value.ok_or("Missing Session receipt")?)
                            .map_err(|e| e.to_string())?;
                    if receipt.intent_id != intent || receipt.reference != reference {
                        return Err("Session receipt identity mismatch".into());
                    }
                    // This owner dispatches each frozen intent once, including across
                    // snapshot restore. Unlike the general Session API, it cannot have
                    // an earlier write followed by a replay rejected after cache eviction.
                    let rejected_once = receipt.state == DeliveryStatus::DeliveryUnknown
                        && receipt.error.as_deref() == Some("input_reference_changed_no_retry");
                    if rejected_once || matches!(receipt.state, DeliveryStatus::Delivered | DeliveryStatus::FailedBeforeWrite) {
                        // Match before clearing the immutable pending intent. A receipt for
                        // an old Pane must not refresh a newly selected Pane/window.
                        refresh = self.terminal.selected.keys().filter_map(|window| {
                            let view = self.generic.view(window)?;
                            let (pending, _) = self.generic.pending_submission(&view.lease).ok()??;
                            (pending == intent).then_some(view.lease)
                        }).collect();
                    }
                    match receipt.state {
                        DeliveryStatus::Delivered => self.generic.resolve(&intent, true)?,
                        DeliveryStatus::FailedBeforeWrite => {
                            self.generic.resolve(&intent, false)?
                        }
                        _ if rejected_once => self.generic.resolve(&intent, false)?,
                        _ => self.generic.submission_error(&intent, receipt.error.unwrap_or_else(|| "Receipt unavailable".into())),
                    }
                } else if let Some(error) = error {
                    self.generic.submission_error(&intent, error);
                }
                let mut out = self
                    .terminal
                    .selected
                    .keys()
                    .filter_map(|window| self.generic.view(window))
                    .map(present)
                    .collect::<Vec<_>>();
                for lease in refresh {
                    if let Some(view) = self.generic.view(&lease.window) {
                        out.extend(self.refresh_input(view.lease)?);
                    }
                }
                out.extend(
                    self.dispatch_content(polar_station_lgp::Input::ExportDrafts)?
                        .into_iter()
                        .map(|e| Effect::Content(Box::new(e))),
                );
                Ok(out)
            }
        }
    }

    pub(crate) fn refresh_input(&mut self, lease: Lease) -> Result<Vec<Effect>, String> {
        let pending = self.generic.pending_submission(&lease)?;
        let selection = self
            .terminal
            .selected
            .get(&lease.window)
            .ok_or("Input selection missing")?
            .clone();
        if let Some((intent, submission)) = pending {
            return match submission {
                Submission::Terminal(send) => {
                    let request_id = id();
                    self.terminal.pending.insert(
                        request_id.clone(),
                        Pending::Send {
                            intent: intent.clone(),
                            reference: send.reference.clone(),
                        },
                    );
                    Ok(vec![request(SessionJob {
                        request_id,

                        request: LocalRequest::Receipt {
                            principal: "station-input".into(),
                            input: polar_station_session::ReceiptRequest {
                                intent_id: intent,
                                reference: send.reference,
                            },
                        },
                    })])
                }
                Submission::Codex { owner_host_id, .. } => Ok(vec![self.codex_request(
                    owner_host_id,
                    polar_tmux_protocol::CodexCommandV1::GetReceipt {
                        intent_id: intent.clone(),
                    },
                    crate::codex::Pending::Send(intent),
                )?]),
            };
        }
        self.terminal.selected.remove(&lease.window);
        self.select_terminal(
            lease.window,
            lease.pane,
            selection.target,
            selection.session,
        )
    }

    pub(crate) fn submit_terminal(
        &mut self,
        lease: Lease,
        press_enter: bool,
    ) -> Result<Vec<Effect>, String> {
        let selection = self
            .terminal
            .selected
            .get(&lease.window)
            .ok_or("Select a terminal before sending")?;
        if selection.lease.pane != lease.pane {
            return Err("Terminal selection changed".into());
        }
        let intent = id();
        self.generic.stage(&lease, intent.clone(), press_enter)?;
        let effects = match self.dispatch_content(polar_station_lgp::Input::ExportDrafts) {
            Ok(effects) => effects,
            Err(error) => {
                self.generic.resolve(&intent, false)?;
                return Err(error);
            }
        };
        self.terminal.saving.insert(intent, self.snapshot_revision);
        let mut out = effects
            .into_iter()
            .map(|e| Effect::Content(Box::new(e)))
            .collect::<Vec<_>>();
        if let Some(view) = self.generic.view(&lease.window) {
            out.push(present(view));
        }
        Ok(out)
    }

    pub(crate) fn snapshot_stored(
        &mut self,
        revision: u64,
        success: bool,
    ) -> Result<Vec<Effect>, String> {
        if revision > self.snapshot_revision {
            return Err("Unknown snapshot receipt".into());
        }
        let intents = self
            .terminal
            .saving
            .iter()
            .filter(|(_, saved)| **saved <= revision)
            .map(|(intent, _)| intent.clone())
            .collect::<Vec<_>>();
        let mut out = vec![];
        let mut restored_retry = false;
        for intent in intents {
            let Some(_) = self.terminal.saving.remove(&intent) else {
                continue;
            };
            if !success {
                self.generic.resolve(&intent, false)?;
                restored_retry = true;
                continue;
            }
            if let Some(submission) = self.generic.saved_submission(&intent) {
                match submission {
                    Submission::Terminal(input) => {
                        let request_id = id();
                        self.terminal.pending.insert(
                            request_id.clone(),
                            Pending::Send {
                                intent,
                                reference: input.reference.clone(),
                            },
                        );
                        out.push(request(SessionJob {
                            request_id,

                            request: LocalRequest::Send {
                                principal: "station-input".into(),
                                input,
                            },
                        }));
                    }
                    Submission::Codex {
                        owner_host_id,
                        command,
                    } => {
                        out.push(self.codex_request(
                            owner_host_id,
                            command,
                            crate::codex::Pending::Send(intent),
                        )?);
                    }
                }
            }
        }
        if restored_retry {
            out.extend(
                self.terminal
                    .selected
                    .keys()
                    .filter_map(|window| self.generic.view(window))
                    .map(present),
            );
        }
        Ok(out)
    }
}
