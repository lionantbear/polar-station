//! Target-scoped Session preparation and desktop-owned creation preferences.
use crate::{CodexStateV1, ControlConfirmationV1, ProtocolValidationError, SessionIdentityV1};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionEditorRequestV1 {
    PrepareTools { execution_target_id:String, tmux:bool, codex:bool, codex_service:bool },
    SetCreationYolo { yolo: bool },
    HistoryDetail { execution_target_id: String, endpoint_id: String, thread_id: String },
    History {
        execution_target_id: String,
        endpoint_id: String,
        cursor: Option<String>,
        search_term: Option<String>,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        include_subagents: bool,
    },
    LaunchContext {
        execution_target_id: String,
        source_session_id: Option<String>,
    },
    BrowseDirectory {
        execution_target_id: String,
        path: String,
        endpoint_id: Option<String>,
    },
    PreviewEndAgent { execution_target_id: String, owner_host_id: String, endpoint_id: String, thread_id: String },
    PreviewEnd {
        execution_target_id: String,
        tmux_session_id: String,
    },
}
impl SessionEditorRequestV1 {
    pub fn is_read_only(&self) -> bool {
        match self {
            Self::SetCreationYolo { .. } | Self::PrepareTools { .. } => false,
            Self::HistoryDetail { .. }
            | Self::History { .. }
            | Self::LaunchContext { .. }
            | Self::BrowseDirectory { .. }
            | Self::PreviewEndAgent { .. }
            | Self::PreviewEnd { .. } => true,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ProtocolValidationError> {
        let target = match self {
            Self::PrepareTools { execution_target_id,tmux,codex,codex_service } => { if !codex && (!tmux || *codex_service) {return Err(ProtocolValidationError::InvalidControlCenterCommand)} execution_target_id },
            Self::SetCreationYolo { .. } => return Ok(()),
            Self::HistoryDetail { execution_target_id, endpoint_id, thread_id } => {
                crate::CodexCommandV1::ReadHistoryDetail { endpoint_id: endpoint_id.clone(), thread_id: thread_id.clone() }.validate()?;
                execution_target_id
            }
            Self::History { execution_target_id, endpoint_id, cursor, search_term, archived, include_subagents } => {
                crate::CodexCommandV1::ListHistory { endpoint_id: endpoint_id.clone(), cursor: cursor.clone(), search_term: search_term.clone(), archived: *archived, include_subagents: *include_subagents }.validate()?;
                execution_target_id
            }
            Self::LaunchContext {
                execution_target_id,
                source_session_id,
            } => {
                if let Some(id) = source_session_id {
                    crate::validate_stable_identifier(id, "source Session")?;
                }
                execution_target_id
            }
            Self::BrowseDirectory {
                execution_target_id,
                path,
                endpoint_id,
            } => {
                if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
                if let Some(id) = endpoint_id {
                    crate::validate_stable_identifier(id, "Codex endpoint")?;
                }
                execution_target_id
            }
            Self::PreviewEndAgent { execution_target_id, owner_host_id, endpoint_id, thread_id } => {
                crate::validate_stable_identifier(owner_host_id, "owner Host")?;
                crate::CodexCommandV1::OpenThread { endpoint_id:endpoint_id.clone(), thread_id:thread_id.clone() }.validate()?;
                execution_target_id
            }
            Self::PreviewEnd {
                execution_target_id,
                tmux_session_id,
            } => {
                crate::validate_stable_identifier(tmux_session_id, "Session")?;
                execution_target_id
            }
        };
        crate::validate_stable_identifier(target, "execution target")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDirectoryV1 {
    pub execution_target_id: String,
    pub home: String,
    pub path: String,
    pub parent: Option<String>,
    pub directories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEndPreviewV1 {
    pub session: Option<SessionIdentityV1>,
    pub name: String,
    pub target_label: String,
    pub attached_clients: u32,
    pub confirmation: ControlConfirmationV1,
    pub codex: Option<CodexStateV1>,
}

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum SessionToolStateV1 { Available, Missing, Unavailable }
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
pub struct SessionTargetToolsV1 {
 pub execution_target_id:String, pub tmux:SessionToolStateV1, pub codex:SessionToolStateV1, pub codex_control_enabled:bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEditorResultV1 {
    HistoryDetail { detail: crate::CodexHistoryDetailV1 },
    History { threads: Vec<crate::CodexHistoryThreadV1>, next_cursor: Option<String> },
    LaunchContext {
        directory: SessionDirectoryV1,
        notice: Option<String>,
        yolo: bool,
        #[serde(default,skip_serializing_if="Option::is_none")]
        tools:Option<Vec<SessionTargetToolsV1>>,
        #[serde(default,skip_serializing_if="Option::is_none")]
        codex_endpoints:Option<Vec<polar_station_settings::CodexEndpointSettingsV1>>,
    },
    ToolsLaunched { execution_target_id:String },
    CreationYolo { yolo: bool },
    Directory {
        directory: SessionDirectoryV1,
    },
    EndPreview {
        preview: Box<SessionEndPreviewV1>,
    },
}

impl SessionEditorResultV1 {
    pub(crate) fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::ToolsLaunched {execution_target_id} => crate::validate_stable_identifier(execution_target_id,"execution target"),
            Self::History { .. } | Self::HistoryDetail { .. } | Self::CreationYolo { .. } => Ok(()),
            Self::LaunchContext { directory, .. } | Self::Directory { directory } => {
                crate::validate_stable_identifier(
                    &directory.execution_target_id,
                    "execution target",
                )?;
                let paths = [&directory.home, &directory.path]
                    .into_iter()
                    .chain(directory.parent.iter())
                    .chain(directory.directories.iter());
                let mut bytes = 0;
                for path in paths {
                    if !path.starts_with('/')
                        || path.len() > 4096
                        || path.chars().any(char::is_control)
                    {
                        return Err(ProtocolValidationError::InvalidControlCenterCommand);
                    }
                    bytes += path.len();
                }
                if bytes > 128 * 1024 || directory.directories.len() > 4096 {
                    return Err(ProtocolValidationError::InvalidControlCenterCommand);
                }
                Ok(())
            }
            Self::EndPreview { preview } => {
                if let Some(session) = &preview.session {
                crate::validate_stable_identifier(
                    &session.execution_target_id,
                    "execution target",
                )?;
                crate::validate_stable_identifier(&session.tmux_session_id, "Session")?;
                } else if preview.codex.is_none() { return Err(ProtocolValidationError::InvalidControlCenterCommand); }
                crate::validate_stable_identifier(&preview.confirmation.id, "confirmation")
            }
        }
    }
}
