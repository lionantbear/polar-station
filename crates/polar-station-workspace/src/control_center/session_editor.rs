//! Window-local New/Resume interaction, shared by Swift and GTK. No I/O or layout.
use super::tool_preparation::{ToolPreparation, PreparationOutcome};
use polar_station_settings::{CodexControlModeV1, CodexEndpointSettingsV1};
use polar_station_protocol::{
    AgentProviderV1, AgentStartModeV1, CodexHistoryDetailV1, CodexHistoryThreadV1,
    ControlCenterIntentV1, SessionDirectoryV1, SessionEditorRequestV1, SessionEditorResultV1, SessionEndPreviewV1,
    SessionIdentityV1, SessionKindV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
impl Default for SessionEditorDraft {
    fn default() -> Self {
        Self {
            bind_only: false, target: String::new(), endpoint: String::new(),
            session_type: String::new(), mode: String::new(), name: String::new(),
            directory: String::new(), command: String::new(), auto_restart: false,
            yolo: true, open_local_cli: false, original_id: String::new(),
            search: String::new(), archived: false, subagents: false, rename: None,
        }
    }
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
    Activate { id: String },
    Previous,
    Next,
    ToggleSelected,
    Manual {
        id: String,
    },
    More,
    RetryHistory,
    PrepareTools,
    ReturnedFromTools,
    SetupStarted { token:u64, error:Option<String> },
    SetupExited { token:u64, exit_code:Option<u32> },
    AppServerAction,
    CheckEndpoint,
    RetryDetail,
    ToggleDirectoryBrowser,
    BrowseDirectory { path: String },
    ChooseDirectory { path: String },
    RetryDirectoryBrowse,
    KnownSessions { sessions: Vec<SessionEditorCandidate> },
    ChooseKnownSession { identity: SessionIdentityV1 },
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
    LaunchSetup {token:u64,setup:polar_station_protocol::ToolSetupLaunchV1},
    FocusSetup {token:u64,setup_id:String},
    SavePreference { token: u64, request: SessionEditorRequestV1 },
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
    Attach { token: u64, identity: SessionIdentityV1 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEditorCandidate {
    pub identity: SessionIdentityV1,
    pub display_name: String,
    pub kind: SessionKindV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEditorView {
    pub context_preserves_form:bool,
    pub prepare_label:Option<String>,
    pub diagnostics_section: Option<String>,
    pub can_prepare:bool,
    pub endpoints:Vec<CodexEndpointSettingsV1>,
    pub can_start_local_server:bool,
    pub server_starting:bool,
    pub server_start_label:String,
    pub can_check_endpoint: bool,
    pub endpoint_checking: bool,
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
    pub directory_browser: Option<SessionDirectoryBrowserView>,
    pub known_sessions: Vec<SessionEditorCandidate>,
    pub known_selection: Option<SessionIdentityV1>,
    pub history_error: Option<String>,
    pub detail_error: Option<String>,
    pub message: Option<String>,
    pub can_more: bool,
    pub can_submit: bool,
    pub pending: bool,
    pub submit_label: String,
    pub local_cli_supported: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionDirectoryBrowserView {
    pub listing: Option<SessionDirectoryV1>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct DirectoryBrowser {
    path: String,
    token: Option<u64>,
    view: SessionDirectoryBrowserView,
}

#[derive(Debug)]
struct Read {
    token: u64,
    epoch: u64,
    request: SessionEditorRequestV1,
}
#[derive(Debug, Default)]
pub struct SessionEditorEngine {
    context_preserves_form:bool,
    tools:Option<Vec<polar_station_protocol::SessionTargetToolsV1>>,
    preparation:ToolPreparation,
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
    directory_browser: Option<DirectoryBrowser>,
    directory_edited: bool,
    known_sessions: Vec<SessionEditorCandidate>,
    known_selection: Option<SessionIdentityV1>,
    yolo_edited: bool,
    desired_yolo: Option<bool>,
    yolo_write: Option<(u64, bool)>,
    yolo_save_error: Option<String>,
    epoch: u64,
    detail_epoch: u64,
    next_token: u64,
    pending: Option<u64>,
    open: bool,
    endpoint_check: Option<Read>,
    endpoint_message: Option<String>,
    local_codex: LocalCodexPreparation,
}

#[derive(Debug, Default)]
enum LocalCodexPreparation {
    #[default]
    Idle,
    Pending(u64),
    Ready,
    NeedsLogin,
    Failed(String),
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
    fn uses_local_codex(&self) -> bool {
        self.open && self.draft.target == "local" && self.draft.session_type == "codex"
            && self.draft.rename.is_none() && self.end_target.is_none()
    }
    fn local_default(&self) -> Option<&str> {
        let endpoint = self.endpoint()?;
        if !self.uses_local_codex() || endpoint.control_mode != CodexControlModeV1::Local {
            return None;
        }
        self.tools.as_ref()?.iter().find(|tool|
            tool.execution_target_id == "local"
                && tool.default_local_codex_endpoint.as_deref() == Some(endpoint.id.as_str())
        ).map(|_| endpoint.id.as_str())
    }
    fn local_codex_waiting(&self) -> bool {
        self.uses_local_codex()
            && ((self.directory_read.is_some()
                && self.endpoint().is_none_or(|endpoint| endpoint.control_mode == CodexControlModeV1::Local))
                || (self.local_default().is_some() && !matches!(self.local_codex, LocalCodexPreparation::Ready)))
    }
    fn prepare_local_codex(&mut self) -> Vec<SessionEditorEffect> {
        let Some(endpoint_id) = self.local_default().map(str::to_owned) else { return vec![]; };
        if self.directory_read.is_some() || self.pending.is_some() || self.preparation.is_pending()
            || !matches!(self.local_codex, LocalCodexPreparation::Idle) {
            return vec![];
        }
        let token = self.token();
        self.local_codex = LocalCodexPreparation::Pending(token);
        vec![SessionEditorEffect::Read {
            token,
            request: SessionEditorRequestV1::EnsureLocalCodex { endpoint_id },
        }]
    }
    fn reset_local_codex(&mut self) -> Option<SessionEditorEffect> {
        match std::mem::take(&mut self.local_codex) {
            LocalCodexPreparation::Pending(token) => Some(SessionEditorEffect::Cancel { token }),
            _ => None,
        }
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
    fn tools_missing(&self,id:&str,tmux:bool,codex:bool)->bool {
        use polar_station_protocol::SessionToolStateV1::Available;
        self.tools.as_ref().and_then(|v|v.iter().find(|t|t.execution_target_id==id))
            .is_none_or(|t|(tmux && t.tmux!=Available)||(codex && t.codex!=Available))
    }
    fn tools_blocked(&self)->bool {
        if self.end_target.is_some() || self.draft.rename.is_some() {return false}
        match self.draft.session_type.as_str() {
            "shell"|"command"|"codex_terminal"=>self.tools_missing(&self.draft.target,true,self.draft.session_type=="codex_terminal"),
            "claude"=>self.tools.as_ref().and_then(|v|v.iter().find(|t|t.execution_target_id==self.draft.target)).is_none_or(|t|t.tmux!=polar_station_protocol::SessionToolStateV1::Available || t.claude!=Some(polar_station_protocol::SessionToolStateV1::Available)),
            "codex"=>self.tools.as_ref().and_then(|v|v.iter().find(|t|t.execution_target_id==self.draft.target)).is_some_and(|t|!t.codex_control_enabled) || (self.draft.open_local_cli && self.tools_missing("local",true,true)),
            _=>false,
        }
    }
    fn preparation_request(&self)->Option<SessionEditorRequestV1> {
        if !self.open || self.draft.rename.is_some() || self.end_target.is_some() {return None}
        let tools=self.tools.as_ref()?;
        let cli=self.draft.session_type=="codex" && self.draft.open_local_cli && self.tools_missing("local",true,true);
        let target=if cli {"local"}else{&self.draft.target};
        let facts=tools.iter().find(|t|t.execution_target_id==target)?;
        use polar_station_protocol::SessionToolStateV1::{Missing,Unavailable};
        let codex=cli || matches!(self.draft.session_type.as_str(),"codex"|"codex_terminal");
        if codex && !cli && self.endpoint().is_some_and(|e|e.control_mode==CodexControlModeV1::RemoteHost){return None}
        if self.uses_local_codex() && !cli
            && !matches!(self.local_codex, LocalCodexPreparation::NeedsLogin)
            && (facts.codex != Missing || matches!(self.local_codex, LocalCodexPreparation::Ready)) {
            return None;
        }
        let claude=self.draft.session_type=="claude";
        if claude && facts.claude.is_none(){return None}
        let tmux=cli || matches!(self.draft.session_type.as_str(),"shell"|"command"|"codex_terminal"|"claude");
        if (tmux && facts.tmux==Unavailable)||(codex && facts.codex==Unavailable)||(claude && facts.claude==Some(Unavailable)){return None}
        let tmux=tmux && facts.tmux==Missing;
        let claude=claude && facts.claude==Some(Missing);
        (tmux||codex||claude).then(||SessionEditorRequestV1::PrepareTools{execution_target_id:target.into(),tmux,codex,codex_service:codex && !cli,claude,claude_login:false,delivery:polar_station_protocol::ToolSetupDeliveryV1::NativePane})
    }
    fn operation(&self) -> Option<ControlCenterIntentV1> {
        if self.tools_blocked() { return None; }
        self.requested_operation()
    }
    fn known_candidate_kind(&self) -> Option<SessionKindV1> {
        if !self.open || self.draft.mode != "resume" || self.draft.rename.is_some() || self.end_target.is_some() {
            return None;
        }
        match self.draft.session_type.as_str() {
            "shell" => Some(SessionKindV1::Terminal),
            "command" => Some(SessionKindV1::Command),
            "claude" => Some(SessionKindV1::ClaudePlane),
            _ => None,
        }
    }
    fn matching_known_sessions(&self) -> impl Iterator<Item = &SessionEditorCandidate> {
        let kind = self.known_candidate_kind();
        self.known_sessions.iter().filter(move |session| {
            kind.as_ref() == Some(&session.kind)
                && session.identity.execution_target_id == self.draft.target
        })
    }
    fn requested_operation(&self) -> Option<ControlCenterIntentV1> {
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
        if d.mode == "resume" && matches!(d.session_type.as_str(), "shell" | "command") {
            return None;
        }
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
                        yolo: codex && d.yolo,
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
        let resume = self.draft.mode == "resume";
        let known_sessions: Vec<_> = self.matching_known_sessions().cloned().collect();
        let known_selection = self.known_selection.clone().filter(|identity| {
            known_sessions.iter().any(|session| &session.identity == identity)
        });
        let request=self.preparation_request();
        let diagnostics_section = if self.preparation.is_pending() {
            None
        } else {
            match &request {
                Some(SessionEditorRequestV1::PrepareTools { tmux: true, .. }) => Some("tmux"),
                Some(SessionEditorRequestV1::PrepareTools { claude: true, .. }) => Some("claude"),
                Some(SessionEditorRequestV1::PrepareTools { codex: true, .. }) => Some("codex"),
                _ => None,
            }
        };
        let prepare_label=match &request {
            Some(SessionEditorRequestV1::PrepareTools{execution_target_id,..}) if !self.tools.as_ref().is_some_and(|tools|tools.iter().any(|tool|tool.execution_target_id==*execution_target_id && tool.native_setup))=>Some("Update Host".into()),
            Some(SessionEditorRequestV1::PrepareTools{execution_target_id,codex,codex_service,claude,..})=>Some(
                if *claude {"Install Claude"}
                else if *codex && !codex_service {"Set up local CLI"}
                else if self.uses_local_codex() && matches!(self.local_codex, LocalCodexPreparation::NeedsLogin) {"Sign in to Codex"}
                else if *codex && self.tools.as_ref().and_then(|v|v.iter().find(|t|t.execution_target_id==*execution_target_id)).is_some_and(|t|t.codex==polar_station_protocol::SessionToolStateV1::Missing) {"Install Codex"}
                else if *codex {"Set up Codex"} else {"Install tmux"}.into()),
            _ if self.open && self.directory_read.is_none() && self.draft.session_type=="claude" && self.tools_blocked() && self.tools.as_ref().and_then(|v|v.iter().find(|t|t.execution_target_id==self.draft.target)).is_none_or(|t|t.claude.is_none())=>Some("Update Host".into()),
            _ if self.tools.is_some() && self.tools_blocked()=>Some("Check tools".into()),
            _=>None,
        };
        let server_starting = matches!(self.local_codex, LocalCodexPreparation::Pending(_));
        let can_prepare=prepare_label.as_deref()!=Some("Update Host") && prepare_label.is_some() && (!self.preparation.is_pending() || self.preparation.focus().is_some()) && self.pending.is_none() && !server_starting && self.directory_read.is_none();
        let endpoint = self.endpoint().filter(|_| self.open && self.draft.session_type == "codex");
        let is_default = self.local_default().is_some();
        let endpoint_action_available = self.pending.is_none() && !server_starting
            && self.endpoint_check.is_none() && !self.preparation.is_pending();
        SessionEditorView {
            context_preserves_form:self.context_preserves_form,
            prepare_label:if self.preparation.focus().is_some(){Some("Show setup Pane".into())}else if self.preparation.is_pending(){Some("Opening setup Pane…".into())}else{prepare_label.clone()},
            diagnostics_section: diagnostics_section.map(str::to_owned),
            can_prepare,
            endpoints:self.endpoints.clone(),
            can_start_local_server: is_default && endpoint_action_available
                && matches!(self.local_codex, LocalCodexPreparation::Failed(_)) && request.is_none(),
            server_starting,
            server_start_label:(if server_starting {"Connecting to local Codex…"} else {"Retry Codex connection"}).into(),
            can_check_endpoint: endpoint.is_some() && !is_default && endpoint_action_available,
            endpoint_checking: self.endpoint_check.is_some(),
            end_preview: self.end_preview.clone(),
            end_loading: self.end_read.is_some(),
            end_completed: self.end_completed,
            draft: self.draft.clone(),
            history: self.history.clone(),
            selected: self.selected.clone(),
            manual: self.manual,
            detail: self.detail.clone(),
            history_loading: self.history_read.is_some() || (self.browsing() && server_starting),
            detail_loading: self.detail_read.is_some(),
            directory_loading: self.directory_read.is_some(),
            directory_browser: self.directory_browser.as_ref().map(|browser| browser.view.clone()),
            known_sessions,
            known_selection: known_selection.clone(),
            history_error: self.history_error.clone(),
            detail_error: self.detail_error.clone(),
            message: self.yolo_save_error.clone()
                .or_else(|| match &self.local_codex {
                    LocalCodexPreparation::Pending(_) => Some("Connecting to local Codex…".into()),
                    LocalCodexPreparation::Failed(error) if !self.preparation.is_pending() => Some(error.clone()),
                    LocalCodexPreparation::NeedsLogin if !self.preparation.is_pending() => Some("Sign in to Codex to continue.".into()),
                    _ => None,
                })
                .or_else(|| self.message.clone())
                .or_else(|| self.endpoint_message.clone())
                .or_else(||(prepare_label.as_deref()==Some("Update Host")).then(||"This Host cannot prepare the required tools in an interactive Shell Pane. Update the Host to continue.".into()))
                .or_else(||(self.directory_read.is_none() && self.tools_blocked()).then(||"Required tools are unavailable on the execution machine. Prepare tools or check its connection.".into())),
            can_more: self.browsing()
                && self.cursor.is_some()
                && self.history_read.is_none()
                && self.history_error.is_none(),
            can_submit: self.open
                && !self.local_codex_waiting()
                && !self.preparation.is_pending()
                && self.pending.is_none()
                && (resume || self.directory_read.is_none())
                && (known_selection.is_some() || self.operation().is_some() || self.picked().is_some()),
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
        if !self.browsing() || self.history_read.is_some() || self.local_codex_waiting() {
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
            || self.local_codex_waiting()
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
    fn can_browse_directory(&self) -> bool {
        self.open && self.pending.is_none() && self.end_target.is_none()
            && self.draft.rename.is_none() && self.draft.mode != "resume"
    }
    fn close_directory_browser(&mut self) -> Option<SessionEditorEffect> {
        self.directory_browser.take()?.token.map(|token| SessionEditorEffect::Cancel { token })
    }
    fn browse_directory(&mut self, path: String) -> Vec<SessionEditorEffect> {
        let token = self.token();
        let Some(browser) = &mut self.directory_browser else { return vec![] };
        let mut effects = Vec::new();
        if let Some(token) = browser.token.replace(token) {
            effects.push(SessionEditorEffect::Cancel { token });
        }
        let request = SessionEditorRequestV1::BrowseDirectory {
            execution_target_id: self.draft.target.clone(),
            path: path.clone(),
            endpoint_id: matches!(self.draft.session_type.as_str(), "codex" | "codex_terminal")
                .then(|| self.draft.endpoint.clone()).filter(|id| !id.is_empty()),
        };
        browser.path = path;
        browser.view = SessionDirectoryBrowserView { loading: true, ..Default::default() };
        effects.push(SessionEditorEffect::Read { token, request });
        effects
    }
    fn save_yolo(&mut self) -> Vec<SessionEditorEffect> {
        let Some(yolo) = self.desired_yolo.filter(|_| self.yolo_write.is_none()) else { return vec![]; };
        let token = self.token();
        self.yolo_write = Some((token, yolo));
        self.yolo_save_error = None;
        vec![SessionEditorEffect::SavePreference { token, request: SessionEditorRequestV1::SetCreationYolo { yolo } }]
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
                self.directory_edited = false;
                self.yolo_edited = self.desired_yolo.is_some();
                if let Some(yolo) = self.desired_yolo { self.draft.yolo = yolo; }
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
                self.context_preserves_form=false;self.tools=None;self.preparation=ToolPreparation::default();
                let server_cancel = self.reset_local_codex();
                self.endpoint_message = None;
                self.open = false;
                self.end_target = None;
                self.end_preview = None;
                self.end_completed = false;
                self.reset_history();
                self.pending = None;
                self.known_selection = None;
                let directory_cancel = self.close_directory_browser();
                [
                    &mut self.history_read,
                    &mut self.detail_read,
                    &mut self.directory_read,
                    &mut self.end_read,
                    &mut self.endpoint_check,
                ]
                .into_iter()
                .filter_map(Option::take)
                .map(|r| SessionEditorEffect::Cancel { token: r.token })
                .chain(directory_cancel)
                .chain(server_cancel)
                .collect()
            }
            SessionEditorInput::Edit { draft } if self.pending.is_none() => {
                let old = self.draft.clone();
                self.draft = draft;
                let target_changed = old.target != self.draft.target;
                if target_changed || old.session_type != self.draft.session_type
                    || (self.draft.session_type == "claude" && old.original_id != self.draft.original_id)
                {
                    self.known_selection = None;
                }
                if target_changed {
                    self.context_preserves_form=false;self.tools=None;self.preparation=ToolPreparation::default();
                    self.draft.directory.clear();
                    self.directory_edited = false;
                    self.draft.open_local_cli = false;
                    self.draft.endpoint.clear();
                } else if old.directory != self.draft.directory {
                    self.directory_edited = true;
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
                if target_changed || old.endpoint != self.draft.endpoint
                    || old.session_type != self.draft.session_type || old.mode != self.draft.mode
                {
                    effects.extend(self.close_directory_browser());
                }
                if target_changed || old.endpoint != self.draft.endpoint
                    || old.session_type != self.draft.session_type {
                    effects.extend(self.reset_local_codex());
                    self.endpoint_message = None;
                    if let Some(read) = self.endpoint_check.take() {
                        effects.push(SessionEditorEffect::Cancel { token: read.token });
                    }
                }
                if old.yolo != self.draft.yolo && !self.draft.bind_only && self.draft.rename.is_none() {
                    self.yolo_edited = true;
                    self.desired_yolo = Some(self.draft.yolo);
                    effects.extend(self.save_yolo());
                }
                if target_changed {
                    if let Some(r) = self.directory_read.take() {
                        effects.push(SessionEditorEffect::Cancel { token: r.token });
                    }
                    effects.extend(self.directory_request());
                }
                effects.extend(self.prepare_local_codex());
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
            SessionEditorInput::Activate { id }
                if self.browsing() && self.pending.is_none() && self.history.iter().any(|t|t.thread_id==id) => {
                    self.manual=false;self.selected.clear();self.draft.original_id=id;
                    self.dispatch(SessionEditorInput::Submit)
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
            SessionEditorInput::PrepareTools if self.view().can_prepare => {
                if let Some((token,id))=self.preparation.focus() {return vec![SessionEditorEffect::FocusSetup{token,setup_id:id.into()}];}
                let Some(request)=self.preparation_request() else {
                    self.context_preserves_form=true;self.message=Some("Checking tools on the execution machine…".into());return self.directory_request();
                };
                let SessionEditorRequestV1::PrepareTools { execution_target_id, .. } = &request else { return vec![]; };
                let token=self.token();
                self.preparation.start(token,self.draft.target.clone(),execution_target_id.clone());

                self.message=Some("Complete setup in its Shell Pane. Your form is retained; readiness is checked after the process exits. Then choose New or Resume when ready.".into());
                vec![SessionEditorEffect::Read{token,request}]
            }
            SessionEditorInput::ReturnedFromTools => vec![],
            SessionEditorInput::SetupStarted{token,error}=>{
                let outcome=self.preparation.started(self.open.then_some(self.draft.target.as_str()),token,error);
                self.preparation_outcome(outcome)
            }
            SessionEditorInput::SetupExited{token,exit_code}=>{
                let outcome=self.preparation.exited(self.open.then_some(self.draft.target.as_str()),token,exit_code);
                self.preparation_outcome(outcome)
            }
            SessionEditorInput::More if self.history_error.is_none() => self.history_request(true),
            SessionEditorInput::AppServerAction => {
                if self.view().can_start_local_server {
                    self.local_codex = LocalCodexPreparation::Idle;
                    self.prepare_local_codex()
                } else {
                    self.dispatch(SessionEditorInput::CheckEndpoint)
                }
            }
            SessionEditorInput::CheckEndpoint if self.view().can_check_endpoint => {
                let endpoint = self.endpoint().expect("selected endpoint");
                let request = SessionEditorRequestV1::CheckEndpoint {
                    execution_target_id: endpoint.execution_target_id.clone(),
                    endpoint_id: endpoint.id.clone(),
                };
                let token = self.token();
                self.endpoint_check = Some(Read { token, epoch: self.epoch, request: request.clone() });
                self.endpoint_message = Some("Checking app server connection…".into());
                vec![SessionEditorEffect::Read { token, request }]
            }
            SessionEditorInput::RetryHistory => self.history_request(!self.history.is_empty()),
            SessionEditorInput::RetryDetail => self.detail_request(),
            SessionEditorInput::KnownSessions { sessions } => {
                self.known_sessions = sessions;
                if self.known_selection.as_ref().is_some_and(|identity| {
                    !self.known_sessions.iter().any(|session| &session.identity == identity)
                }) {
                    self.known_selection = None;
                }
                vec![]
            }
            SessionEditorInput::ChooseKnownSession { identity } if self.pending.is_none() => {
                if self.matching_known_sessions().any(|session| session.identity == identity) {
                    self.known_selection = Some(identity);
                    if self.draft.session_type == "claude" {
                        self.draft.original_id.clear();
                    }
                }
                vec![]
            }
            SessionEditorInput::ToggleDirectoryBrowser if self.can_browse_directory() => {
                if self.directory_browser.is_some() {
                    return self.close_directory_browser().into_iter().collect();
                }
                self.directory_browser = Some(DirectoryBrowser::default());
                let path = if self.draft.directory.trim().is_empty() {
                    "~".into()
                } else {
                    self.draft.directory.clone()
                };
                self.browse_directory(path)
            }
            SessionEditorInput::BrowseDirectory { path }
                if self.can_browse_directory() && !path.trim().is_empty() =>
            {
                self.browse_directory(path)
            }
            SessionEditorInput::RetryDirectoryBrowse if self.can_browse_directory() => {
                let Some(path) = self.directory_browser.as_ref()
                    .filter(|browser| browser.view.error.is_some())
                    .map(|browser| browser.path.clone()) else { return vec![] };
                self.browse_directory(path)
            }
            SessionEditorInput::ChooseDirectory { path } if self.can_browse_directory() => {
                let listing = self.directory_browser.as_ref()
                    .and_then(|browser| browser.view.listing.as_ref());
                if listing.is_some_and(|listing| listing.directories.contains(&path)) {
                    self.draft.directory = path;
                    self.directory_edited = true;
                }
                vec![]
            }
            SessionEditorInput::Submit if self.view().can_submit => {
                if let Some(identity) = self.known_selection.clone().filter(|identity| {
                    self.matching_known_sessions().any(|session| &session.identity == identity)
                }) {
                    let token = self.token();
                    self.pending = Some(token);
                    return vec![SessionEditorEffect::Attach { token, identity }];
                }
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
    fn preparation_outcome(&mut self,outcome:PreparationOutcome)->Vec<SessionEditorEffect> {
        match outcome {
            PreparationOutcome::Launch{token,setup}=>vec![SessionEditorEffect::LaunchSetup{token,setup}],
            PreparationOutcome::Running=>{self.message=Some("Setup is running in its Shell Pane. You can enter a password there or interrupt it with Ctrl+C.".into());vec![]}
            PreparationOutcome::Recheck{message}=>{
                self.context_preserves_form=true;
                self.message=message.or_else(||Some("Checking tools on the execution machine…".into()));
                let mut effects=self.directory_read.take().map(|read|vec![SessionEditorEffect::Cancel{token:read.token}]).unwrap_or_default();
                effects.extend(self.reset_local_codex());
                effects.extend(self.directory_request());effects
            }
            PreparationOutcome::Failed(message)=>{self.message=Some(message);vec![]}
            PreparationOutcome::Stale=>{vec![]}
            PreparationOutcome::Ignored=>vec![],
        }
    }
    fn complete(
        &mut self,
        token: u64,
        result: Option<SessionEditorResultV1>,
        error: Option<String>,
    ) -> Vec<SessionEditorEffect> {
        if matches!(self.local_codex, LocalCodexPreparation::Pending(current) if current == token) {
            self.local_codex = match (result, error) {
                (Some(SessionEditorResultV1::EndpointReady), None) => LocalCodexPreparation::Ready,
                (Some(SessionEditorResultV1::EndpointLoginRequired), None) => LocalCodexPreparation::NeedsLogin,
                (_, error) => LocalCodexPreparation::Failed(error.unwrap_or_else(||
                    "The Host did not confirm the local Codex connection.".into())),
            };
            self.history_error = None;
            let mut effects = self.history_request(false);
            effects.extend(self.detail_request());
            return effects;
        }
        if let Some(browser) = self.directory_browser.as_mut().filter(|browser| browser.token == Some(token)) {
            browser.token = None;
            browser.view.loading = false;
            match (result, error) {
                (Some(SessionEditorResultV1::Directory { directory }), None)
                    if directory.execution_target_id == self.draft.target =>
                {
                    browser.view.listing = Some(directory);
                }
                (_, error) => browser.view.error = Some(error.unwrap_or_else(|| {
                    "This Host did not return the requested directory.".into()
                })),
            }
            return vec![];
        }
        if self.endpoint_check.as_ref().is_some_and(|read| read.token == token) {
            self.endpoint_check = None;
            self.endpoint_message = Some(match (result, error) {
                (Some(SessionEditorResultV1::EndpointReady), None) => "App server connected.".into(),
                (_, error) => error.unwrap_or_else(|| "The Host did not confirm the app server connection.".into()),
            });
            return vec![];
        }
        if self.preparation.matches(token) {
            let outcome=self.preparation.complete(self.open.then_some(self.draft.target.as_str()),result,error);
            return self.preparation_outcome(outcome);
        }
        if let Some((_, written)) = self.yolo_write.filter(|(id, _)| *id == token) {
            self.yolo_write = None;
            match (result, error) {
                (Some(SessionEditorResultV1::CreationYolo { yolo }), None) if yolo == written => {
                    self.yolo_save_error = None;
                    if self.desired_yolo == Some(written) { self.desired_yolo = None; }
                    return self.save_yolo();
                }
                (_, error) => {
                    self.yolo_save_error = Some(error.unwrap_or_else(|| "YOLO preference could not be saved. Change it again to retry.".into()));
                    if self.desired_yolo.is_some_and(|desired| desired != written) {
                        return self.save_yolo();
                    }
                    return vec![];
                }
            }
        }
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
                (Some(SessionEditorResultV1::LaunchContext { directory, notice, yolo,tools,codex_endpoints }), None)
                    if directory.execution_target_id == self.draft.target =>
                {
                    self.tools=tools;
                    let mut effects=if let Some(endpoints)=codex_endpoints {self.dispatch(SessionEditorInput::Endpoints{endpoints})}else{vec![]};
                    effects.extend(self.prepare_local_codex());
                    if self.message.as_deref()==Some("Checking tools on the execution machine…") {self.message=None;}
                    if !self.yolo_edited { self.draft.yolo = yolo; }
                    if !self.context_preserves_form && !self.directory_edited && self.draft.directory.is_empty() {
                        self.draft.directory = directory.path.clone();
                        if self.message.is_none() {
                            self.message = notice;
                        }
                    }
                    if !self.context_preserves_form && self.draft.name.is_empty() {
                        self.draft.name = directory
                            .path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or("Session")
                            .into();
                    }
                    if self.browsing() && self.history_read.is_none() && (self.history_error.is_some()||self.history.is_empty()) {effects.extend(self.history_request(false));}
                    return effects;
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


