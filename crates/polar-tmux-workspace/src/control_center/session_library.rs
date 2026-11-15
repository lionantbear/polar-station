mod catalog;
pub use catalog::*;
use serde::{Deserialize, Serialize};

use crate::{ExecutionTargetId, SessionKey, TransportStatus};

use super::ControlActionProjection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSection {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDiscoveryStatus {
    Loading,
    #[default]
    Ready,
    Failed,
}

/// Classifies a terminal operation plane without changing its tmux identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Terminal,
    CodexPlane,
    ClaudePlane,
}

impl SessionKind {
    /// Public tmux naming convention; independent of the observing client.
    pub fn from_terminal_name(name: &str) -> Option<(Self, &str)> {
        let (kind, id) = if let Some(id) = name.strip_prefix("codex-") {
            (Self::CodexPlane, id)
        } else if let Some(id) = name.strip_prefix("claude-") {
            (Self::ClaudePlane, id)
        } else { return None };
        let parsed = uuid::Uuid::parse_str(id).ok()?;
        (parsed.hyphenated().to_string() == id).then_some((kind, id))
    }

    pub fn terminal_name(self, id: &str) -> Option<String> {
        let id = uuid::Uuid::parse_str(id).ok()?.hyphenated().to_string();
        match self {
            Self::CodexPlane => Some(format!("codex-{id}")),
            Self::ClaudePlane => Some(format!("claude-{id}")),
            Self::Terminal => None,
        }
    }

    pub fn from_restore_spec(spec: &crate::ProcessRestoreSpec) -> Self {
        match spec {
            crate::ProcessRestoreSpec::CodexAppServer { .. }
            | crate::ProcessRestoreSpec::CodexForwardedCli { .. }
            | crate::ProcessRestoreSpec::Codex { .. } => Self::CodexPlane,
            crate::ProcessRestoreSpec::Claude { .. } => Self::ClaudePlane,
            _ => Self::Terminal,
        }
    }
}

/// Observed tmux existence, separate from transport health and native Pane
/// lifetime. A failed discovery cannot prove that a Session ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneSessionPresence {
    Present,
    Ended,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTargetFact {
    pub id: ExecutionTargetId,
    pub label: String,
    pub section: SessionSection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSectionState {
    Loading,
    Populated,
    Empty,
    TargetMissing,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSectionProjection {
    pub section: SessionSection,
    pub title: String,
    pub state: SessionSectionState,
    pub message: Option<String>,
    pub row_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFact {
    pub identity: SessionKey,
    pub display_name: String,
    pub target_label: String,
    pub section: SessionSection,
    pub attached_clients: u32,
    pub transport_status: TransportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_status: Option<crate::SessionRecoveryStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<polar_tmux_protocol::SessionReplacementV1>,
    #[serde(default)]
    pub session_kind: SessionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRowProjection {
    pub identity: SessionKey,
    pub display_name: String,
    pub detail_label: Option<String>,
    pub target_label: String,
    pub section: SessionSection,
    pub attached_clients: u32,
    pub transport_status: TransportStatus,
    pub selected: bool,
    pub assigned_to_selected_tab: bool,
    pub assignment_action: ControlActionProjection,
    pub assignment_actions: Option<SessionAssignmentActions>,
    pub rename_action: ControlActionProjection,
    pub kill_action: ControlActionProjection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_action: Option<ControlActionProjection>,
    pub replacement: Option<polar_tmux_protocol::SessionReplacementV1>,
    pub replacement_action: Option<ControlActionProjection>,
    #[serde(default)]
    pub session_kind: SessionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_agent_id: Option<String>,
}

/// Host-projected copy and availability policy for every native Workspace
/// assignment state. Native clients select one of these actions from their
/// click-time Workspace/Tab/Pane snapshot; the Host does not own that native
/// presentation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAssignmentActions {
    pub add: ControlActionProjection,
    pub remove: ControlActionProjection,
    pub tab_full: ControlActionProjection,
    pub assigned_elsewhere: ControlActionProjection,
    pub no_managed_tab: ControlActionProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLibraryProjection {
    pub visible: bool,
    pub title: String,
    pub interaction_hint: String,
    pub refresh_action: ControlActionProjection,
    pub close_action: ControlActionProjection,
    pub sections: Vec<SessionSectionProjection>,
    pub remote_targets: Vec<SessionTargetFact>,
    pub rows: Vec<SessionRowProjection>,
    pub selected: Option<SessionKey>,
    pub local_execution_target_id: ExecutionTargetId,
    pub remote_execution_target_id: Option<ExecutionTargetId>,
    pub local_new_shell: ControlActionProjection,
    pub local_new_command: ControlActionProjection,
    pub remote_new_session: ControlActionProjection,
}


