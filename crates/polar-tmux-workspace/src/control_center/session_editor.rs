//! Window-local New/Resume interaction, shared by Swift and GTK. No I/O or layout.
use polar_station_settings::{CodexControlModeV1, CodexEndpointSettingsV1};
use polar_tmux_protocol::{
    AgentProviderV1, AgentStartModeV1, CodexHistoryDetailV1, CodexHistoryThreadV1,
    ControlCenterIntentV1, SessionEditorRequestV1, SessionEditorResultV1, SessionEndPreviewV1,
    SessionIdentityV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionEditorDraft {
    pub bind_only: bool,
    pub target: String,
    pub endpoint: String,
    pub session_type: String,
    pub mode: String,
    pub name: String,
    pub directory: String,
    pub command: String,
    pub auto_restart: bool,
    pub yolo: bool,
    pub open_local_cli: bool,
    pub original_id: String,
    pub search: String,
    pub archived: bool,
    pub subagents: bool,
    pub rename: Option<SessionIdentityV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEditorInput {
    Open {
        draft: SessionEditorDraft,
        endpoints: Vec<CodexEndpointSettingsV1>,
        source: Option<SessionIdentityV1>,
    },
    OpenEnd {
        request: SessionEditorRequestV1,
    },
    RefreshEnd,
    Endpoints {
        endpoints: Vec<CodexEndpointSettingsV1>,
    },
    Edit {
        draft: SessionEditorDraft,
    },
    Select {
        id: String,
        checked: Option<bool>,
    },
    Previous,
    Next,
    ToggleSelected,
    Manual {
        id: String,
    },
    More,
    RetryHistory,
    RetryDetail,
    Complete {
        token: u64,
        result: Option<SessionEditorResultV1>,
        error: Option<String>,
    },
    Submit,
    Submitted {
        token: u64,
        error: Option<String>,
        completed: Vec<String>,
    },
    Close,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEditorEffect {
    Picked { token: u64, owner_host_id: String, endpoint_id: String, thread_id: String, title: String },
    Read {
        token: u64,
        request: SessionEditorRequestV1,
    },
    Cancel {
        token: u64,
    },
    Submit {
        token: u64,
        operation: ControlCenterIntentV1,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEditorView {
    pub end_preview: Option<SessionEndPreviewV1>,
    pub end_loading: bool,
    pub end_completed: bool,
    pub draft: SessionEditorDraft,
    pub history: Vec<CodexHistoryThreadV1>,
    pub selected: Vec<String>,
    pub manual: bool,
    pub detail: Option<CodexHistoryDetailV1>,
    pub history_loading: bool,
    pub detail_loading: bool,
    pub directory_loading: bool,
    pub history_error: Option<String>,
    pub detail_error: Option<String>,
    pub message: Option<String>,
    pub can_more: bool,
    pub can_submit: bool,
    pub pending: bool,
    pub submit_label: String,
    pub local_cli_supported: bool,
}
#[derive(Debug)]
struct Read {
    token: u64,
    epoch: u64,
    request: SessionEditorRequestV1,
}
#[derive(Debug, Default)]
pub struct SessionEditorEngine {
    end_target: Option<SessionEditorRequestV1>,
    end_preview: Option<SessionEndPreviewV1>,
    end_read: Option<Read>,
    end_completed: bool,
    draft: SessionEditorDraft,
    endpoints: Vec<CodexEndpointSettingsV1>,
    source: Option<SessionIdentityV1>,
    history: Vec<CodexHistoryThreadV1>,
    selected: Vec<String>,
    manual: bool,
    cursor: Option<String>,
    consumed: BTreeSet<String>,
    detail: Option<CodexHistoryDetailV1>,
    history_error: Option<String>,
    detail_error: Option<String>,
    message: Option<String>,
    history_read: Option<Read>,
    detail_read: Option<Read>,
    directory_read: Option<Read>,
    epoch: u64,
    detail_epoch: u64,
    next_token: u64,
    pending: Option<u64>,
    open: bool,
}
impl SessionEditorEngine {
    fn token(&mut self) -> u64 {
        self.next_token = self
            .next_token
            .checked_add(1)
            .expect("editor token exhausted");
        self.next_token
    }
    fn browsing(&self) -> bool {
        self.open
            && self.draft.rename.is_none()
            && self.draft.session_type == "codex"
            && self.draft.mode == "resume"
    }
    fn endpoint(&self) -> Option<&CodexEndpointSettingsV1> {
        self.endpoints
            .iter()
            .find(|e| e.id == self.draft.endpoint && e.execution_target_id == self.draft.target)
    }
    fn local_cli_supported(&self) -> bool {
        self.endpoint()
            .is_some_and(|e| e.control_mode != CodexControlModeV1::RemoteHost)
    }
    fn normalize(&mut self) {
        if self.draft.bind_only {
            self.draft.session_type = "codex".into();
            self.draft.mode = "resume".into();
            self.draft.open_local_cli = false;
            self.draft.rename = None;
        }
        if self.endpoint().is_none() {
            self.draft.endpoint = self
                .endpoints
                .iter()
                .find(|e| e.execution_target_id == self.draft.target)
                .map(|e| e.id.clone())
                .unwrap_or_default();
        }
        if self.draft.session_type != "codex" || !self.local_cli_supported() {
            self.draft.open_local_cli = false;
        }
    }
    fn picked(&self) -> Option<(String, String, String, String)> {
        if !self.draft.bind_only || !self.browsing() || self.history_read.is_some() { return None; }
        let endpoint = self.endpoint()?;
        let id = &self.draft.original_id;
        let thread = if self.manual { self.detail.as_ref().map(|d| &d.thread).filter(|t| t.thread_id == *id) }
            else { self.history.iter().find(|t| t.thread_id == *id) }?;
        Some((endpoint.owner_host_id.clone(), endpoint.id.clone(), id.clone(), thread.display_name.clone()))
    }
    fn operation(&self) -> Option<ControlCenterIntentV1> {
        if self.draft.bind_only { return None; }
        if let Some(request) = &self.end_target {
            let confirmation_id = self.end_preview.as_ref()?.confirmation.id.clone();
            return Some(match request {
                SessionEditorRequestV1::PreviewEnd {
                    execution_target_id,
                    tmux_session_id,
                } => ControlCenterIntentV1::KillSession {
                    execution_target_id: execution_target_id.clone(),
                    tmux_session_id: tmux_session_id.clone(),
                    confirmation_id,
                },
                SessionEditorRequestV1::PreviewEndAgent {
                    owner_host_id,
                    endpoint_id,
                    thread_id,
                    ..
                } => ControlCenterIntentV1::EndAgentSession {
                    owner_host_id: owner_host_id.clone(),
                    endpoint_id: endpoint_id.clone(),
                    thread_id: thread_id.clone(),
                    confirmation_id,
                },
                _ => return None,
            });
        }
        let d = &self.draft;
        let optional = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        let op = if let Some(identity) = &d.rename {
            ControlCenterIntentV1::RenameSession {
                execution_target_id: identity.execution_target_id.clone(),
                tmux_session_id: identity.tmux_session_id.clone(),
                name: d.name.trim().into(),
            }
        } else {
            match d.session_type.as_str() {
                "codex" | "claude" => {
                    let codex = d.session_type == "codex";
                    let resume = d.mode == "resume";
                    if codex
                        && (self.endpoint().is_none()
                            || (d.open_local_cli && !self.local_cli_supported()))
                    {
                        return None;
                    }
                    ControlCenterIntentV1::CreateAgentSession {
                        execution_target_id: d.target.clone(),
                        provider: if codex {
                            AgentProviderV1::Codex
                        } else {
                            AgentProviderV1::Claude
                        },
                        mode: if resume {
                            AgentStartModeV1::Resume
                        } else {
                            AgentStartModeV1::New
                        },
                        name: if codex && resume {
                            String::new()
                        } else {
                            d.name.trim().into()
                        },
                        working_directory: if codex && resume {
                            None
                        } else {
                            optional(&d.directory)
                        },
                        endpoint_id: codex.then(|| d.endpoint.clone()),
                        session_id: if resume {
                            optional(&d.original_id)
                        } else {
                            None
                        },
                        session_ids: if codex && resume && !self.manual {
                            self.selected.clone()
                        } else {
                            vec![]
                        },
                        yolo: codex && !resume && d.yolo,
                        open_local_cli: codex && d.open_local_cli,
                        open_terminal: false,
                    }
                }
                "codex_terminal" => {
                    self.endpoint()?;
                    ControlCenterIntentV1::CreateCodexSession {
                        execution_target_id: d.target.clone(),
                        endpoint_id: d.endpoint.clone(),
                        name: d.name.trim().into(),
                        working_directory: optional(&d.directory),
                        yolo: d.yolo,
                    }
                }
                "shell" | "command" => ControlCenterIntentV1::CreateSession {
                    execution_target_id: d.target.clone(),
                    name: d.name.trim().into(),
                    working_directory: optional(&d.directory),
                    command: if d.session_type == "command" {
                        Some(optional(&d.command)?)
                    } else {
                        None
                    },
                    auto_restart: d.session_type == "command" && d.auto_restart,
                },
                _ => return None,
            }
        };
        op.validate().ok().map(|()| op)
    }
    pub fn view(&self) -> SessionEditorView {
        let resume = matches!(self.draft.session_type.as_str(), "codex" | "claude")
            && self.draft.mode == "resume";
        SessionEditorView {
            end_preview: self.end_preview.clone(),
            end_loading: self.end_read.is_some(),
            end_completed: self.end_completed,
            draft: self.draft.clone(),
            history: self.history.clone(),
            selected: self.selected.clone(),
            manual: self.manual,
            detail: self.detail.clone(),
            history_loading: self.history_read.is_some(),
            detail_loading: self.detail_read.is_some(),
            directory_loading: self.directory_read.is_some(),
            history_error: self.history_error.clone(),
            detail_error: self.detail_error.clone(),
            message: self.message.clone(),
            can_more: self.browsing()
                && self.cursor.is_some()
                && self.history_read.is_none()
                && self.history_error.is_none(),
            can_submit: self.open
                && self.pending.is_none()
                && (resume || self.directory_read.is_none())
                && (self.operation().is_some() || self.picked().is_some()),
            pending: self.pending.is_some(),
            submit_label: if self.end_target.is_some() {
                self.end_preview
                    .as_ref()
                    .map(|p| p.confirmation.confirm_label.clone())
                    .unwrap_or_else(|| "End Session".into())
            } else if self.draft.bind_only {
                "Bind session".into()
            } else if self.draft.rename.is_some() {
                "Rename".into()
            } else if resume {
                format!("Add {} to Tab", self.selected.len().max(1))
            } else {
                "New & Open".into()
            },
            local_cli_supported: self.local_cli_supported(),
        }
    }
    fn reset_history(&mut self) {
        self.epoch += 1;
        self.detail_epoch += 1;
        self.history.clear();
        self.selected.clear();
        if !self.manual {
            self.draft.original_id.clear();
        }
        self.cursor = None;
        self.consumed.clear();
        self.detail = None;
        self.history_error = None;
        self.detail_error = None;
    }
    fn history_request(&mut self, more: bool) -> Vec<SessionEditorEffect> {
        if !self.browsing() || self.history_read.is_some() {
            return vec![];
        }
        if self.endpoint().is_none() {
            self.history_error = Some("Select an app-server to browse its history.".into());
            return vec![];
        }
        if more && self.cursor.is_none() {
            return vec![];
        }
        let request = SessionEditorRequestV1::History {
            execution_target_id: self.draft.target.clone(),
            endpoint_id: self.draft.endpoint.clone(),
            cursor: if more { self.cursor.clone() } else { None },
            search_term: (!self.draft.search.is_empty()).then(|| self.draft.search.clone()),
            archived: self.draft.archived,
            include_subagents: self.draft.subagents,
        };
        let token = self.token();
        self.history_error = None;
        self.history_read = Some(Read {
            token,
            epoch: self.epoch,
            request: request.clone(),
        });
        vec![SessionEditorEffect::Read { token, request }]
    }
    fn detail_request(&mut self) -> Vec<SessionEditorEffect> {
        if !self.browsing()
            || self.draft.original_id.is_empty()
            || self.endpoint().is_none()
            || self.detail_read.is_some()
        {
            return vec![];
        }
        let request = SessionEditorRequestV1::HistoryDetail {
            execution_target_id: self.draft.target.clone(),
            endpoint_id: self.draft.endpoint.clone(),
            thread_id: self.draft.original_id.clone(),
        };
        let token = self.token();
        self.detail_error = None;
        self.detail_read = Some(Read {
            token,
            epoch: self.detail_epoch,
            request: request.clone(),
        });
        vec![SessionEditorEffect::Read { token, request }]
    }
    fn directory_request(&mut self) -> Vec<SessionEditorEffect> {
        if !self.open || self.draft.rename.is_some() || self.directory_read.is_some() {
            return vec![];
        }
        let request = SessionEditorRequestV1::LaunchContext {
            execution_target_id: self.draft.target.clone(),
            source_session_id: self
                .source
                .as_ref()
                .filter(|s| s.execution_target_id == self.draft.target)
                .map(|s| s.tmux_session_id.clone()),
        };
        let token = self.token();
        self.directory_read = Some(Read {
            token,
            epoch: self.epoch,
            request: request.clone(),
        });
        vec![SessionEditorEffect::Read { token, request }]
    }
    pub fn dispatch(&mut self, input: SessionEditorInput) -> Vec<SessionEditorEffect> {
        match input {
            SessionEditorInput::OpenEnd { request }
                if matches!(
                    request,
                    SessionEditorRequestV1::PreviewEnd { .. }
                        | SessionEditorRequestV1::PreviewEndAgent { .. }
                ) =>
            {
                let mut effects = self.dispatch(SessionEditorInput::Close);
                self.open = true;
                self.end_target = Some(request);
                effects.extend(self.dispatch(SessionEditorInput::RefreshEnd));
                effects
            }
            SessionEditorInput::RefreshEnd
                if self.open && self.pending.is_none() && self.end_target.is_some() =>
            {
                let mut effects = vec![];
                if let Some(read) = self.end_read.take() {
                    effects.push(SessionEditorEffect::Cancel { token: read.token });
                }
                self.end_preview = None;
                self.end_completed = false;
                self.message = Some("Checking the original Session…".into());
                let token = self.token();
                let request = self.end_target.clone().expect("end target");
                self.end_read = Some(Read {
                    token,
                    epoch: self.epoch,
                    request: request.clone(),
                });
                effects.push(SessionEditorEffect::Read { token, request });
                effects
            }
            SessionEditorInput::Open {
                draft,
                endpoints,
                source,
            } => {
                let mut effects = self.dispatch(SessionEditorInput::Close);
                self.open = true;
                self.draft = draft;
                self.endpoints = endpoints;
                self.source = source;
                self.normalize();
                self.manual = !self.draft.original_id.is_empty();
                self.message = None;
                effects.extend(self.directory_request());
                effects.extend(self.history_request(false));
                effects.extend(self.detail_request());
                effects
            }
            SessionEditorInput::Close => {
                self.open = false;
                self.end_target = None;
                self.end_preview = None;
                self.end_completed = false;
                self.reset_history();
                self.pending = None;
                [
                    &mut self.history_read,
                    &mut self.detail_read,
                    &mut self.directory_read,
                    &mut self.end_read,
                ]
                .into_iter()
                .filter_map(Option::take)
                .map(|r| SessionEditorEffect::Cancel { token: r.token })
                .collect()
            }
            SessionEditorInput::Edit { draft } if self.pending.is_none() => {
                let old = self.draft.clone();
                self.draft = draft;
                let target_changed = old.target != self.draft.target;
                if target_changed {
                    self.draft.directory.clear();
                    self.draft.yolo = false;
                    self.draft.open_local_cli = false;
                    self.draft.endpoint.clear();
                }
                self.normalize();
                let scope_changed = old.target != self.draft.target
                    || old.endpoint != self.draft.endpoint
                    || old.session_type != self.draft.session_type
                    || old.mode != self.draft.mode
                    || old.search != self.draft.search
                    || old.archived != self.draft.archived
                    || old.subagents != self.draft.subagents;
                let mut effects = vec![];
                if scope_changed {
                    self.reset_history();
                    if old.target != self.draft.target || old.endpoint != self.draft.endpoint {
                        self.draft.original_id.clear();
                        self.manual = false;
                    }
                    effects.extend(self.history_request(false));
                }
                if scope_changed || old.original_id != self.draft.original_id {
                    self.detail_epoch += 1;
                    self.detail = None;
                    effects.extend(self.detail_request());
                }
                if target_changed {
                    if let Some(r) = self.directory_read.take() {
                        effects.push(SessionEditorEffect::Cancel { token: r.token });
                    }
                    effects.extend(self.directory_request());
                }
                effects
            }
            SessionEditorInput::Endpoints { endpoints } => {
                self.endpoints = endpoints;
                self.dispatch(SessionEditorInput::Edit {
                    draft: self.draft.clone(),
                })
            }
            input @ (SessionEditorInput::Previous | SessionEditorInput::Next)
                if self.browsing() && self.pending.is_none() && !self.history.is_empty() =>
            {
                let current = self
                    .history
                    .iter()
                    .position(|t| t.thread_id == self.draft.original_id);
                let index = match (current, input) {
                    (Some(i), SessionEditorInput::Previous) => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(self.history.len() - 1),
                    (None, _) => 0,
                };
                let id = self.history[index].thread_id.clone();
                if id == self.draft.original_id {
                    return vec![];
                }
                self.dispatch(SessionEditorInput::Select { id, checked: None })
            }
            SessionEditorInput::ToggleSelected if self.browsing() && self.pending.is_none() => {
                let id = self.draft.original_id.clone();
                let checked = !self.selected.contains(&id);
                self.dispatch(SessionEditorInput::Select {
                    id,
                    checked: Some(checked),
                })
            }
            SessionEditorInput::Select { id, checked }
                if self.pending.is_none() && self.history.iter().any(|t| t.thread_id == id) =>
            {
                self.manual = false;
                if self.draft.bind_only { self.selected.clear(); self.selected.push(id.clone()); }
                else if let Some(checked) = checked {
                    self.selected.retain(|v| v != &id);
                    if checked && self.selected.len() < 9 {
                        self.selected.push(id.clone());
                    }
                }
                self.draft.original_id = id;
                self.detail_epoch += 1;
                self.detail = None;
                self.detail_request()
            }
            SessionEditorInput::Manual { id } if self.pending.is_none() => {
                self.manual = true;
                self.selected.clear();
                self.draft.original_id = id.trim().into();
                self.detail_epoch += 1;
                self.detail = None;
                self.detail_request()
            }
            SessionEditorInput::More if self.history_error.is_none() => self.history_request(true),
            SessionEditorInput::RetryHistory => self.history_request(!self.history.is_empty()),
            SessionEditorInput::RetryDetail => self.detail_request(),
            SessionEditorInput::Submit if self.view().can_submit => {
                if let Some((owner_host_id, endpoint_id, thread_id, title)) = self.picked() {
                    let token = self.token(); self.pending = Some(token);
                    return vec![SessionEditorEffect::Picked {token, owner_host_id, endpoint_id, thread_id, title}];
                }
                let operation = self.operation().expect("validated editor operation");
                let token = self.token();
                self.pending = Some(token);
                self.message = self.end_target.as_ref().map(|_| "Ending Session…".into());
                vec![SessionEditorEffect::Submit { token, operation }]
            }
            SessionEditorInput::Submitted {
                token,
                error,
                completed,
            } if self.pending == Some(token) => {
                self.pending = None;
                if self.end_target.is_some() {
                    self.end_completed = error.is_none();
                    // A failed or unknown mutation cannot reuse its preview/confirmation.
                    self.end_preview = None;
                }
                self.message = error;
                self.selected.retain(|id| !completed.contains(id));
                if let Some(id) = self.selected.first() {
                    self.draft.original_id = id.clone();
                }
                vec![]
            }
            SessionEditorInput::Complete {
                token,
                result,
                error,
            } => self.complete(token, result, error),
            _ => vec![],
        }
    }
    fn complete(
        &mut self,
        token: u64,
        result: Option<SessionEditorResultV1>,
        error: Option<String>,
    ) -> Vec<SessionEditorEffect> {
        if self.end_read.as_ref().is_some_and(|r| r.token == token) {
            self.end_read = None;
            match (result, error) {
                (Some(SessionEditorResultV1::EndPreview { preview }), None) => {
                    self.end_preview = Some(*preview);
                    self.message = None;
                }
                (_, error) => {
                    self.message = Some(error.unwrap_or_else(|| {
                        "This Host did not return a Session end preview.".into()
                    }))
                }
            }
            return vec![];
        }
        if self.history_read.as_ref().is_some_and(|r| r.token == token) {
            let read = self.history_read.take().unwrap();
            if read.epoch != self.epoch {
                return self.history_request(false);
            }
            match (result, error) {
                (
                    Some(SessionEditorResultV1::History {
                        threads,
                        next_cursor,
                    }),
                    None,
                ) => {
                    let mut seen: BTreeSet<_> =
                        self.history.iter().map(|t| t.thread_id.clone()).collect();
                    self.history.extend(
                        threads
                            .into_iter()
                            .filter(|t| seen.insert(t.thread_id.clone())),
                    );
                    if let SessionEditorRequestV1::History {
                        cursor: Some(cursor),
                        ..
                    } = read.request
                    {
                        self.consumed.insert(cursor);
                    }
                    self.cursor =
                        next_cursor.filter(|s| !s.is_empty() && !self.consumed.contains(s));
                    if self.draft.original_id.is_empty() {
                        self.draft.original_id = self
                            .history
                            .first()
                            .map(|t| t.thread_id.clone())
                            .unwrap_or_default();
                        self.detail_epoch += 1;
                    }
                    return self.detail_request();
                }
                (_, error) => {
                    self.history_error = Some(
                        error.unwrap_or_else(|| "This Host did not return session history.".into()),
                    )
                }
            }
        } else if self.detail_read.as_ref().is_some_and(|r| r.token == token) {
            let read = self.detail_read.take().unwrap();
            if read.epoch != self.detail_epoch {
                return self.detail_request();
            }
            match (result, error) {
                (Some(SessionEditorResultV1::HistoryDetail { detail }), None)
                    if detail.thread.thread_id == self.draft.original_id =>
                {
                    self.detail = Some(detail)
                }
                (_, error) => {
                    self.detail_error = Some(error.unwrap_or_else(|| {
                        "This Host did not return the selected conversation.".into()
                    }))
                }
            }
        } else if self
            .directory_read
            .as_ref()
            .is_some_and(|r| r.token == token)
        {
            let read = self.directory_read.take().unwrap();
            let current = matches!(&read.request, SessionEditorRequestV1::LaunchContext { execution_target_id, .. } if execution_target_id == &self.draft.target);
            if !current {
                return vec![];
            }
            match (result, error) {
                (Some(SessionEditorResultV1::LaunchContext { directory, notice }), None)
                    if directory.execution_target_id == self.draft.target =>
                {
                    if self.draft.directory.is_empty() {
                        self.draft.directory = directory.path.clone();
                        if self.message.is_none() {
                            self.message = notice;
                        }
                    }
                    if self.draft.name.is_empty() {
                        self.draft.name = directory
                            .path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or("Session")
                            .into();
                    }
                }
                (_, error) => {
                    self.message =
                        Some(error.unwrap_or_else(|| {
                            "This Host did not return a startup directory.".into()
                        }))
                }
            }
        }
        vec![]
    }
}


