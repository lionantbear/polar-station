use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::ids::{ExecutionTargetId, ManagedSessionId, StableTmuxSessionId};
use crate::validation::validate_label;

pub const SESSION_RUNTIME_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessRestoreSpec {
    #[default]
    Shell,
    Command {
        command: String,
        #[serde(default)]
        auto_restart: bool,
    },
    CodexAppServer {
        owner_host_id: String,
        endpoint_id: String,
        thread_id: String,
        socket_path: String,
    },
    /// An optional client-host CLI, bound to a live SSH/local forward. This
    /// transient socket is never a durable auto-restart recipe.
    CodexForwardedCli {
        owner_host_id: String,
        endpoint_id: String,
        thread_id: String,
        socket_path: String,
    },
    Codex {
        session_id: String,
        #[serde(default)]
        yolo: bool,
    },
    /// Claude's real conversation UUID, independent of this tmux plane.
    Claude { session_id: String },
}

impl ProcessRestoreSpec {
    pub fn original_agent_id(&self) -> Option<&str> {
        match self {
            Self::Claude { session_id } | Self::Codex { session_id, .. } => Some(session_id),
            Self::CodexAppServer { thread_id, .. } | Self::CodexForwardedCli { thread_id, .. } => Some(thread_id),
            Self::Shell | Self::Command { .. } => None,
        }
    }

    fn validate(&self) -> Result<(), WorkspaceError> {
        match self {
            Self::CodexAppServer { owner_host_id, endpoint_id, thread_id, socket_path }
            | Self::CodexForwardedCli { owner_host_id, endpoint_id, thread_id, socket_path } => {
                if [owner_host_id, endpoint_id, thread_id].iter().any(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
                    || !socket_path.starts_with('/') || socket_path.len() > 4096 || socket_path.chars().any(char::is_control) {
                    return Err(WorkspaceError::new(WorkspaceErrorCode::InvalidCatalog, "Invalid original app-server identity"));
                }
                Ok(())
            }
            Self::Shell => Ok(()),
            Self::Command { command, .. } => {
                if command.trim().is_empty() || command.contains('\0') {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorCode::InvalidCatalog,
                        "Command recovery recipe is invalid",
                    ));
                }
                Ok(())
            }
            Self::Codex { session_id, .. } if session_id.trim().is_empty() => {
                Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "Codex session ID must not be empty",
                ))
            }
            Self::Codex { session_id, .. }
                if session_id.len() > 256 || session_id.chars().any(char::is_control) =>
            {
                Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "Codex session ID is invalid",
                ))
            }
            Self::Codex { .. } => Ok(()),
            Self::Claude { session_id } => {
                if uuid::Uuid::parse_str(session_id).is_err() {
                    return Err(WorkspaceError::new(WorkspaceErrorCode::InvalidCatalog, "Claude session ID must be a UUID"));
                }
                Ok(())
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunchSpec {
    pub session_name: String,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub process: ProcessRestoreSpec,
}

impl SessionLaunchSpec {
    pub fn shell(session_name: impl Into<String>, working_directory: Option<&str>) -> Self {
        Self {
            session_name: session_name.into(),
            working_directory: working_directory.map(str::to_owned),
            process: ProcessRestoreSpec::Shell,
        }
    }

    pub fn codex(
        session_name: impl Into<String>,
        working_directory: Option<&str>,
        session_id: impl Into<String>,
        yolo: bool,
    ) -> Result<Self, WorkspaceError> {
        let spec = Self {
            session_name: session_name.into(),
            working_directory: working_directory.map(str::to_owned),
            process: ProcessRestoreSpec::Codex {
                session_id: session_id.into(),
                yolo,
            },
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        validate_label(
            &self.session_name,
            "session launch name",
            WorkspaceErrorCode::InvalidCatalog,
        )?;
        if let Some(path) = &self.working_directory {
            if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "session working directory is invalid",
                ));
            }
        }
        self.process.validate()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryStatus {
    #[default]
    Ready,
    Recovering,
    Manual,
    Ended,
    Ending,
    Conflict,
    Unavailable,
}

/// One recipe per Session, independent of the number of native consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSessionRecord {
    pub id: ManagedSessionId,
    pub execution_target_id: ExecutionTargetId,
    pub runtime_tmux_session_id: StableTmuxSessionId,
    pub server_generation: String,
    /// Last observed exact identity; a hint until the remote guard succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_unix_seconds: Option<i64>,
    pub launch: SessionLaunchSpec,
    #[serde(default)]
    pub process_completed: bool,
    #[serde(default)]
    pub status: SessionRecoveryStatus,
}

impl ManagedSessionRecord {
    pub fn reference(&self) -> String {
        format!("station-session-{}", self.id)
    }

    pub fn permits_restart(&self) -> bool {
        !self.process_completed
            && !matches!(self.status, SessionRecoveryStatus::Ended | SessionRecoveryStatus::Ending)
            && !matches!(
                self.launch.process,
                ProcessRestoreSpec::CodexForwardedCli { .. } | ProcessRestoreSpec::Command {
                    auto_restart: false,
                    ..
                }
            )
    }
}

/// Pure work recovery policy; transport/read failures are not observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRecoveryDecision {
    KeepEnded,
    Adopt,
    Conflict,
    End,
    Manual,
    Restore,
}

pub fn decide_recovery(
    record: &ManagedSessionRecord,
    same_server: bool,
    matching_markers: usize,
    occupied: bool,
) -> SessionRecoveryDecision {
    if record.status == SessionRecoveryStatus::Ended {
        SessionRecoveryDecision::KeepEnded
    } else if record.status == SessionRecoveryStatus::Ending {
        if matching_markers == 0 && !occupied { SessionRecoveryDecision::End } else { SessionRecoveryDecision::KeepEnded }
    } else if matching_markers > 1 {
        SessionRecoveryDecision::Conflict
    } else if matching_markers == 1 {
        SessionRecoveryDecision::Adopt
    } else if occupied {
        SessionRecoveryDecision::Conflict
    } else if same_server || record.process_completed {
        SessionRecoveryDecision::End
    } else if record.permits_restart() {
        SessionRecoveryDecision::Restore
    } else {
        SessionRecoveryDecision::Manual
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOperationReceipt {
    pub key: String,
    pub fingerprint: String,
    pub response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRuntimeState {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub sessions: Vec<ManagedSessionRecord>,
    #[serde(default)]
    pub operation_receipts: Vec<SessionOperationReceipt>,
}

impl SessionRuntimeState {
    pub fn new() -> Self {
        Self {
            schema_version: SESSION_RUNTIME_SCHEMA_VERSION,
            revision: 0,
            sessions: Vec::new(),
            operation_receipts: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.schema_version != SESSION_RUNTIME_SCHEMA_VERSION {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::SchemaUnsupported,
                "unsupported Session runtime schema",
            ));
        }
        let mut ids = BTreeSet::new();
        for session in &self.sessions {
            session.launch.validate()?;
            if !ids.insert(session.id) || session.server_generation.is_empty() {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "invalid managed Session identity",
                ));
            }
        }
        Ok(())
    }
}

impl Default for SessionRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}


