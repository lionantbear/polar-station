//! Window-local Connections interaction. No I/O, credentials, or Workspace geometry.
use polar_station_settings::{CodexControlModeV1, CodexControlSettingsV1, CodexEndpointSettingsV1};
use polar_tmux_protocol::{ConnectionChangeV1, ConnectionsProjectionV1};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    #[default]
    Connection,
    Polar,
    Codex,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Selection {
    pub machine: String,
    pub page: Page,
    pub item: Option<String>,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            machine: "local".into(),
            page: Page::Connection,
            item: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectionsInput {
    Snapshot {
        facts: ConnectionsProjectionV1,
    },
    Navigate {
        selection: Selection,
    },
    CodexSnapshot {
        settings: CodexControlSettingsV1,
        owner: String,
    },
    EditCodex {
        machine: String,
        id: Option<String>,
    },
    RemoveCodex {
        machine: String,
        id: String,
    },
    DraftCodex {
        endpoint: CodexEndpointSettingsV1,
        enabled: bool,
    },
    Edit {
        change: ConnectionChangeV1,
    },
    Draft {
        change: ConnectionChangeV1,
    },
    Save,
    Cancel,
    Close,
    Discard,
    KeepEditing,
    Complete {
        token: u64,
        error: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectionsEffect {
    SaveCodex {
        token: u64,
        expected_revision: u64,
        settings: CodexControlSettingsV1,
    },
    Save {
        token: u64,
        expected_revision: String,
        change: ConnectionChangeV1,
    },
    Close,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionsView {
    pub selection: Selection,
    pub editing: bool,
    pub dirty: bool,
    pub pending: bool,
    pub confirm_discard: bool,
    pub message: Option<String>,
    pub draft: Option<ConnectionChangeV1>,
    pub facts: ConnectionsProjectionV1,
    pub codex_settings: Option<CodexControlSettingsV1>,
    pub codex_draft: Option<CodexEndpointSettingsV1>,
    pub codex_enabled: bool,
    pub save_label: String,
    pub discard_label: String,
    pub keep_label: String,
    pub confirmation: String,
}
#[derive(Debug, Clone)]
enum Destination {
    Selection(Selection),
    Close,
    Cancel,
}
#[derive(Debug, Clone)]
struct Draft {
    original: Value,
    change: ConnectionChangeV1,
    revision: String,
}
#[derive(Debug, Clone)]
struct CodexDraft {
    baseline: CodexControlSettingsV1,
    original: CodexEndpointSettingsV1,
    endpoint: CodexEndpointSettingsV1,
    existing: bool,
    remove: bool,
    enabled: bool,
    owner: String,
}
impl CodexDraft {
    fn candidate(&self) -> Result<CodexControlSettingsV1, String> {
        let mut candidate = self.baseline.clone();
        let mut endpoint = self.endpoint.clone();
        if self.remove {
            candidate.endpoints.retain(|e| e.id != self.original.id);
            candidate.validate().map_err(|e| e.to_string())?;
            return Ok(candidate);
        }
        if !self.existing && candidate.endpoints.iter().any(|e| e.id == endpoint.id) {
            return Err("An endpoint with this ID already exists. Choose another ID.".into());
        }
        if endpoint.control_mode != CodexControlModeV1::RemoteHost {
            endpoint.owner_host_id = self.owner.clone();
            endpoint.peer_connection_ref = None;
            endpoint.remote_endpoint_id = None;
            endpoint.remote_target_id = None;
        } else {
            let connection = candidate
                .host_connections
                .iter()
                .find(|c| {
                    Some(&c.id) == endpoint.peer_connection_ref.as_ref()
                        && c.execution_credential_ref.is_some()
                })
                .ok_or("Select an authorized Station Host connection.")?;
            endpoint.owner_host_id = connection.host_id.clone();
            endpoint.socket_path.clear();
        }
        if self.existing {
            let saved = candidate
                .endpoints
                .iter_mut()
                .find(|e| e.id == self.original.id)
                .ok_or("This endpoint is no longer available.")?;
            *saved = endpoint;
        } else {
            candidate.endpoints.push(endpoint);
        }
        candidate.enabled = self.enabled;
        candidate
            .validate()
            .map_err(|e| format!("Settings were not saved: {e}. Your draft is retained."))?;
        Ok(candidate)
    }
}
#[derive(Debug, Default)]
pub struct ConnectionsEngine {
    facts: ConnectionsProjectionV1,
    codex_settings: Option<CodexControlSettingsV1>,
    codex_owner: String,
    codex_draft: Option<CodexDraft>,
    selection: Selection,
    draft: Option<Draft>,
    next_token: u64,
    pending: Option<u64>,
    destination: Option<Destination>,
    message: Option<String>,
}
impl ConnectionsEngine {
    fn editing(&self) -> bool {
        self.draft.is_some() || self.codex_draft.is_some()
    }
    fn dirty(&self) -> bool {
        if let Some(d) = &self.codex_draft {
            return !d.existing || d.endpoint != d.original || d.enabled != d.baseline.enabled;
        }
        self.draft
            .as_ref()
            .is_some_and(|d| serde_json::to_value(&d.change).ok().as_ref() != Some(&d.original))
    }
    pub fn view(&self) -> ConnectionsView {
        ConnectionsView {
            selection: self.selection.clone(),
            editing: self.editing(),
            dirty: self.dirty(),
            pending: self.pending.is_some(),
            confirm_discard: self.destination.is_some() && self.pending.is_none(),
            message: self.message.clone(),
            draft: self.draft.as_ref().map(|d| d.change.clone()),
            facts: self.facts.clone(),
            codex_settings: self.codex_settings.clone(),
            codex_draft: self
                .codex_draft
                .as_ref()
                .filter(|d| !d.remove)
                .map(|d| d.endpoint.clone()),
            codex_enabled: self.codex_draft.as_ref().map_or_else(
                || self.codex_settings.as_ref().is_some_and(|s| s.enabled),
                |d| d.enabled,
            ),
            save_label: if self.pending.is_some() {
                "Saving…"
            } else {
                "Save"
            }
            .into(),
            discard_label: "Discard".into(),
            keep_label: "Keep editing".into(),
            confirmation: "Save changes before leaving this connection?".into(),
        }
    }
    pub fn dispatch(&mut self, input: ConnectionsInput) -> Vec<ConnectionsEffect> {
        // Host refreshes and correlated results remain admissible while saving.
        match input {
            ConnectionsInput::CodexSnapshot { settings, owner } => {
                self.codex_settings = Some(settings);
                self.codex_owner = owner;
                return vec![];
            }
            ConnectionsInput::Snapshot { facts } => {
                self.facts = facts;
                if !self.editing() && self.pending.is_none() {
                    self.normalize_selection();
                }
                return vec![];
            }
            ConnectionsInput::Complete { token, error } => {
                if self.pending != Some(token) {
                    return vec![];
                }
                self.pending = None;
                if let Some(error) = error {
                    if self.codex_draft.as_ref().is_some_and(|d| d.remove) {
                        self.codex_draft = None;
                    }
                    self.message = Some(error);
                    self.destination = None;
                    return vec![];
                }
                if let Some(draft) = self.codex_draft.take() {
                    self.selection.machine = draft.endpoint.execution_target_id;
                    self.selection.page = Page::Codex;
                    self.selection.item = Some(draft.endpoint.id);
                }
                if let Some(draft) = self.draft.take() {
                    match draft.change {
                        ConnectionChangeV1::SaveMachine { machine } if !machine.id.is_empty() => {
                            self.selection.machine = machine.id;
                            self.selection.page = Page::Connection;
                            self.selection.item = None;
                        }
                        ConnectionChangeV1::SavePolar { polar } => {
                            self.selection.machine = polar.execution_target_id;
                            self.selection.page = Page::Polar;
                            self.selection.item = (!polar.id.is_empty()).then_some(polar.id);
                        }
                        _ => {}
                    }
                }
                self.message = Some("Saved. Check the connection to verify access.".into());
                if let Some(next) = self.destination.take() {
                    return self.apply_destination(next);
                }
                self.normalize_selection();
                return vec![];
            }
            _ if self.pending.is_some() => return vec![],
            _ => {}
        }
        match input {
            ConnectionsInput::RemoveCodex { machine, id } => {
                if self.editing() {
                    return vec![];
                }
                self.dispatch(ConnectionsInput::EditCodex {
                    machine,
                    id: Some(id),
                });
                if let Some(draft) = &mut self.codex_draft {
                    draft.remove = true;
                }
                return self.dispatch(ConnectionsInput::Save);
            }
            ConnectionsInput::EditCodex { machine, id } => {
                if self.editing() {
                    return vec![];
                }
                let Some(settings) = &self.codex_settings else {
                    self.message = Some("Load app-server settings before editing.".into());
                    return vec![];
                };
                let endpoint = if let Some(id) = &id {
                    let Some(endpoint) = settings
                        .endpoints
                        .iter()
                        .find(|e| &e.id == id && e.execution_target_id == machine)
                    else {
                        self.message =
                            Some("This endpoint is no longer available on this machine.".into());
                        return vec![];
                    };
                    endpoint.clone()
                } else {
                    CodexEndpointSettingsV1 {
                        id: String::new(),
                        execution_target_id: machine.clone(),
                        control_mode: if machine == "local" {
                            CodexControlModeV1::Local
                        } else {
                            CodexControlModeV1::SshDirect
                        },
                        owner_host_id: self.codex_owner.clone(),
                        socket_path: String::new(),
                        peer_connection_ref: None,
                        remote_endpoint_id: None,
                        remote_target_id: None,
                    }
                };
                self.selection = Selection {
                    machine,
                    page: Page::Codex,
                    item: id.clone(),
                };
                self.codex_draft = Some(CodexDraft {
                    baseline: settings.clone(),
                    original: endpoint.clone(),
                    endpoint,
                    existing: id.is_some(),
                    remove: false,
                    enabled: settings.enabled,
                    owner: self.codex_owner.clone(),
                });
                self.message = None;
            }
            ConnectionsInput::DraftCodex { endpoint, enabled } => {
                if let Some(draft) = &mut self.codex_draft {
                    if endpoint.execution_target_id == draft.original.execution_target_id
                        && (!draft.existing || endpoint.id == draft.original.id)
                    {
                        draft.endpoint = endpoint;
                        draft.enabled = enabled;
                    }
                }
            }
            ConnectionsInput::Edit { change } => {
                if !self.editing() {
                    self.draft = Some(Draft {
                        original: serde_json::to_value(&change).unwrap_or(Value::Null),
                        change,
                        revision: self.facts.revision.clone(),
                    });
                    self.message = None;
                }
            }
            ConnectionsInput::Draft { change } => {
                if let Some(draft) = &mut self.draft {
                    // A field edit may not silently change the edited object's identity or kind.
                    if identity(&draft.change) == identity(&change) {
                        draft.change = change;
                    }
                }
            }
            ConnectionsInput::Navigate { selection } => {
                return self.leave(Destination::Selection(selection))
            }
            ConnectionsInput::Cancel => return self.leave(Destination::Cancel),
            ConnectionsInput::Close => return self.leave(Destination::Close),
            ConnectionsInput::KeepEditing => self.destination = None,
            ConnectionsInput::Discard => {
                if let Some(next) = self.destination.take() {
                    self.draft = None;
                    self.codex_draft = None;
                    return self.apply_destination(next);
                }
            }
            ConnectionsInput::Save => {
                if let Some(draft) = &self.codex_draft {
                    let settings = match draft.candidate() {
                        Ok(settings) => settings,
                        Err(error) => {
                            self.message = Some(error);
                            self.destination = None;
                            return vec![];
                        }
                    };
                    self.next_token += 1;
                    self.pending = Some(self.next_token);
                    self.message = None;
                    return vec![ConnectionsEffect::SaveCodex {
                        token: self.next_token,
                        expected_revision: draft.baseline.revision,
                        settings,
                    }];
                }
                if let Some(draft) = &self.draft {
                    if draft.revision.is_empty() {
                        self.message = Some(
                            "Connection settings have not loaded. Your draft is retained.".into(),
                        );
                        return vec![];
                    }
                    self.next_token += 1;
                    self.pending = Some(self.next_token);
                    self.message = None;
                    return vec![ConnectionsEffect::Save {
                        token: self.next_token,
                        expected_revision: draft.revision.clone(),
                        change: draft.change.clone(),
                    }];
                }
            }
            ConnectionsInput::Snapshot { .. }
            | ConnectionsInput::CodexSnapshot { .. }
            | ConnectionsInput::Complete { .. } => unreachable!(),
        }
        vec![]
    }
    fn leave(&mut self, destination: Destination) -> Vec<ConnectionsEffect> {
        if self.editing() {
            self.destination = Some(destination);
            vec![]
        } else {
            self.draft = None;
            self.apply_destination(destination)
        }
    }
    fn apply_destination(&mut self, destination: Destination) -> Vec<ConnectionsEffect> {
        self.destination = None;
        match destination {
            Destination::Selection(selection) => {
                self.selection = selection;
                self.message = None;
                self.normalize_selection();
                vec![]
            }
            Destination::Cancel => {
                self.message = None;
                self.normalize_selection();
                vec![]
            }
            Destination::Close => vec![ConnectionsEffect::Close],
        }
    }
    fn normalize_selection(&mut self) {
        if self.selection.machine != "local"
            && !self
                .facts
                .machines
                .iter()
                .any(|m| m.id == self.selection.machine)
        {
            self.selection = Selection::default();
        }
        if self.selection.page == Page::Polar
            && self.selection.item.as_ref().is_some_and(|id| {
                !self
                    .facts
                    .polar_instances
                    .iter()
                    .any(|p| &p.id == id && p.execution_target_id == self.selection.machine)
            })
        {
            self.selection.item = None;
        }
    }
}
fn identity(change: &ConnectionChangeV1) -> Value {
    match change {
        ConnectionChangeV1::SaveMachine { machine } => json!(["machine", machine.id]),
        ConnectionChangeV1::SavePolar { polar } => {
            json!(["polar", polar.id, polar.execution_target_id])
        }
        ConnectionChangeV1::RemoveMachine { id } => json!(["remove_machine", id]),
        ConnectionChangeV1::RemovePolar { id } => json!(["remove_polar", id]),
    }
}
