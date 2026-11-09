//! Workspace v1 wire DTOs.
//!
//! These types describe transport shapes only. Workspace validation, layout,
//! reconcile, transaction, and reference safety remain in
//! `polar-tmux-workspace`.

use serde::{Deserialize, Serialize};

use crate::{
    validate_display_label, validate_stable_identifier, ClientEnvelope,
    ControlCenterRequestEnvelopeV1, ControlCenterResponseEnvelopeV1, DisplayMode, HostEnvelope,
    ProtocolError, ProtocolValidationError, ResourcesRequestEnvelopeV1,
    ResourcesResponseEnvelopeV1, PROTOCOL_VERSION,
};

pub const WORKSPACE_API_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceApiV1 {
    WorkspaceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceScopeV1 {
    Catalog,
    Workspace { workspace_id: String },
}

impl WorkspaceScopeV1 {
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Catalog => None,
            Self::Workspace { workspace_id } => Some(workspace_id),
        }
    }

    fn validate(&self) -> Result<(), ProtocolValidationError> {
        if let Self::Workspace { workspace_id } = self {
            validate_stable_identifier(workspace_id, "Workspace ID")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRequestEnvelopeV1 {
    pub protocol_version: u32,
    pub api: WorkspaceApiV1,
    pub request_id: String,
    pub scope: WorkspaceScopeV1,
    pub expected_catalog_revision: Option<u64>,
    pub expected_workspace_revision: Option<u64>,
    pub command: WorkspaceCommandV1,
}

impl WorkspaceRequestEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        self.scope.validate()?;
        if self.command.is_mutation() && self.expected_catalog_revision.is_none() {
            return Err(ProtocolValidationError::MissingExpectedRevision);
        }
        if self.command.requires_workspace_scope() {
            if !matches!(self.scope, WorkspaceScopeV1::Workspace { .. }) {
                return Err(ProtocolValidationError::InvalidWorkspaceScope);
            }
            if self.command.is_mutation() && self.expected_workspace_revision.is_none() {
                return Err(ProtocolValidationError::MissingExpectedRevision);
            }
        } else if !matches!(self.scope, WorkspaceScopeV1::Catalog) {
            return Err(ProtocolValidationError::InvalidWorkspaceScope);
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceCommandV1 {
    GetCatalog,
    GetWorkspace,
    SetManagedTerminalFontSize {
        points: u8,
    },
    CreateWorkspace {
        name: String,
    },
    SelectWorkspace {
        workspace_id: String,
    },
    RenameWorkspace {
        name: String,
    },
    DeleteWorkspace,
    AddTab {
        title: String,
    },
    AddTabWithId {
        tab_id: String,
        title: String,
    },
    SelectTab {
        tab_id: String,
    },
    RenameTab {
        tab_id: String,
        title: String,
    },
    ReorderTab {
        tab_id: String,
        to_index: usize,
    },
    DeleteTab {
        tab_id: String,
    },
    DeleteTabWithReplacement {
        tab_id: String,
        replacement_tab_id: String,
    },
    AssignSession {
        tab_id: String,
        execution_target_id: String,
        tmux_session_id: String,
    },
    AssignSessionWithId {
        tab_id: String,
        pane_id: String,
        execution_target_id: String,
        tmux_session_id: String,
    },
    RemovePane {
        tab_id: String,
        pane_id: String,
    },
    ReorderPane {
        tab_id: String,
        pane_id: String,
        to_index: usize,
    },
    MovePane {
        pane_id: String,
        from_tab_id: String,
        to_tab_id: String,
        to_index: usize,
    },
    SetDisplayMode {
        tab_id: String,
        display_mode: DisplayMode,
    },
    SelectFocusPane {
        tab_id: String,
        pane_id: String,
    },
    Open,
    Apply,
    Repair,
    Rebuild,
    Detach,
    FocusPrevious,
    FocusNext,
    FocusSidebarSlot {
        slot: u8,
    },
}

impl WorkspaceCommandV1 {
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::GetCatalog | Self::GetWorkspace)
    }

    pub fn requires_workspace_scope(&self) -> bool {
        !matches!(
            self,
            Self::GetCatalog
                | Self::SetManagedTerminalFontSize { .. }
                | Self::CreateWorkspace { .. }
                | Self::SelectWorkspace { .. }
        )
    }

    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::SetManagedTerminalFontSize { points } => {
                if (10..=32).contains(points) {
                    Ok(())
                } else {
                    Err(ProtocolValidationError::InvalidWorkspaceCommand)
                }
            }
            Self::CreateWorkspace { name } | Self::RenameWorkspace { name } => {
                validate_display_label(name, "Workspace name")
            }
            Self::SelectWorkspace { workspace_id } => {
                validate_stable_identifier(workspace_id, "Workspace ID")
            }
            Self::AddTab { title } => validate_display_label(title, "tab title"),
            Self::AddTabWithId { tab_id, title } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_display_label(title, "tab title")
            }
            Self::SelectTab { tab_id }
            | Self::DeleteTab { tab_id }
            | Self::SetDisplayMode { tab_id, .. } => validate_stable_identifier(tab_id, "tab ID"),
            Self::DeleteTabWithReplacement {
                tab_id,
                replacement_tab_id,
            } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_stable_identifier(replacement_tab_id, "replacement tab ID")?;
                if tab_id == replacement_tab_id {
                    return Err(ProtocolValidationError::InvalidWorkspaceCommand);
                }
                Ok(())
            }
            Self::RenameTab { tab_id, title } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_display_label(title, "tab title")
            }
            Self::ReorderTab { tab_id, .. } => validate_stable_identifier(tab_id, "tab ID"),
            Self::AssignSession {
                tab_id,
                execution_target_id,
                tmux_session_id,
            } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                validate_stable_identifier(tmux_session_id, "tmux session ID")
            }
            Self::AssignSessionWithId {
                tab_id,
                pane_id,
                execution_target_id,
                tmux_session_id,
            } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_stable_identifier(pane_id, "pane ID")?;
                validate_stable_identifier(execution_target_id, "execution target ID")?;
                validate_stable_identifier(tmux_session_id, "tmux session ID")
            }
            Self::RemovePane { tab_id, pane_id }
            | Self::ReorderPane {
                tab_id, pane_id, ..
            }
            | Self::SelectFocusPane { tab_id, pane_id } => {
                validate_stable_identifier(tab_id, "tab ID")?;
                validate_stable_identifier(pane_id, "pane ID")
            }
            Self::MovePane {
                pane_id,
                from_tab_id,
                to_tab_id,
                ..
            } => {
                validate_stable_identifier(pane_id, "pane ID")?;
                validate_stable_identifier(from_tab_id, "source tab ID")?;
                validate_stable_identifier(to_tab_id, "destination tab ID")
            }
            Self::FocusSidebarSlot { slot } if !(1..=8).contains(slot) => {
                Err(ProtocolValidationError::InvalidWorkspaceCommand)
            }
            Self::GetCatalog
            | Self::GetWorkspace
            | Self::DeleteWorkspace
            | Self::Open
            | Self::Apply
            | Self::Repair
            | Self::Rebuild
            | Self::Detach
            | Self::FocusPrevious
            | Self::FocusNext
            | Self::FocusSidebarSlot { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceResponseEnvelopeV1 {
    pub protocol_version: u32,
    pub api: WorkspaceApiV1,
    pub request_id: String,
    pub scope: WorkspaceScopeV1,
    pub revision: u64,
    pub result: WorkspaceResultV1,
}

impl WorkspaceResponseEnvelopeV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        validate_stable_identifier(&self.request_id, "request ID")?;
        self.scope.validate()?;
        match &self.result {
            WorkspaceResultV1::OperationAccepted { operation_id, .. }
            | WorkspaceResultV1::CompanionOperationAccepted { operation_id, .. } => {
                validate_stable_identifier(operation_id, "operation ID")?;
            }
            WorkspaceResultV1::OperationCompleted {
                operation_id,
                warnings,
                ..
            }
            | WorkspaceResultV1::CompanionOperationCompleted {
                operation_id,
                warnings,
                ..
            } => {
                validate_stable_identifier(operation_id, "operation ID")?;
                for warning in warnings {
                    warning.validate_sanitized()?;
                }
            }
            WorkspaceResultV1::Error { error } => error.validate_sanitized()?,
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceResultV1 {
    Catalog {
        projection: WorkspaceCatalogProjectionV1,
    },
    CompanionCatalog {
        projection: CompanionWorkspaceCatalogProjectionV1,
    },
    Transition {
        projection: WorkspaceCatalogProjectionV1,
        impact: PendingImpactV1,
        automatic_apply_candidate: bool,
    },
    CompanionTransition {
        projection: CompanionWorkspaceCatalogProjectionV1,
        impact: PendingImpactV1,
        automatic_apply_candidate: bool,
    },
    OperationAccepted {
        operation_id: String,
        projection: WorkspaceCatalogProjectionV1,
    },
    CompanionOperationAccepted {
        operation_id: String,
        projection: CompanionWorkspaceCatalogProjectionV1,
    },
    OperationCompleted {
        operation_id: String,
        projection: WorkspaceCatalogProjectionV1,
        warnings: Vec<ProtocolError>,
    },
    CompanionOperationCompleted {
        operation_id: String,
        projection: CompanionWorkspaceCatalogProjectionV1,
        warnings: Vec<ProtocolError>,
    },
    Accepted,
    Error {
        error: ProtocolError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCatalogProjectionV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub selected_workspace_id: String,
    pub workspaces: Vec<WorkspaceProjectionV1>,
    pub pending_operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProjectionV1 {
    pub id: String,
    pub name: String,
    pub desired_revision: u64,
    pub applied_revision: Option<u64>,
    pub runtime_health: RuntimeHealthV1,
    pub pending_impact: PendingImpactV1,
    pub selected_tab_id: String,
    pub tabs: Vec<WorkspaceTabProjectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTabProjectionV1 {
    pub id: String,
    pub title: String,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<String>,
    pub pane_order: Vec<String>,
    pub layout: CanonicalLayoutProjectionV1,
    pub panes: Vec<ManagedPaneProjectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPaneProjectionV1 {
    pub id: String,
    pub execution_target_id: String,
    pub tmux_session_id: String,
    pub target_label: String,
    pub session_name: String,
    pub transport_status: TransportStatusV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionWorkspaceCatalogProjectionV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub selected_workspace_id: String,
    pub workspaces: Vec<CompanionWorkspaceProjectionV1>,
    pub pending_operation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionWorkspaceProjectionV1 {
    pub id: String,
    pub name: String,
    pub desired_revision: u64,
    pub applied_revision: Option<u64>,
    pub runtime_health: RuntimeHealthV1,
    pub pending_impact: PendingImpactV1,
    pub selected_tab_id: String,
    pub tabs: Vec<CompanionWorkspaceTabProjectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionWorkspaceTabProjectionV1 {
    pub id: String,
    pub title: String,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<String>,
    pub pane_order: Vec<String>,
    pub layout: CanonicalLayoutProjectionV1,
    pub panes: Vec<CompanionManagedPaneProjectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionManagedPaneProjectionV1 {
    pub id: String,
    pub target_label: String,
    pub session_name: String,
    pub transport_status: TransportStatusV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalLayoutProjectionV1 {
    pub pane_order: Vec<String>,
    pub slots: Vec<LayoutSlotV1>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSlotV1 {
    pub pane_id: String,
    pub canonical_index: usize,
    pub visual_index: usize,
    pub column: usize,
    pub row: usize,
    pub width: RationalV1,
    pub height: RationalV1,
    pub role: LayoutRoleV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RationalV1 {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutRoleV1 {
    Monitor,
    Main,
    Sidebar { slot: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingImpactV1 {
    None,
    InPlaceCandidate,
    ReplacementMayBeRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealthV1 {
    Closed,
    Healthy,
    NeedsRepair,
    Busy,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportStatusV1 {
    Online,
    Offline,
    Recovering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientApplicationMessage {
    CodexV1(Box<crate::CodexRequestEnvelopeV1>),
    NotificationV1(Box<crate::NotificationRequestEnvelopeV1>),
    ControlCenterV1(ControlCenterRequestEnvelopeV1),
    InboxV1(Box<crate::InboxRequestEnvelopeV1>),
    ResourcesV1(Box<ResourcesRequestEnvelopeV1>),
    SettingsV1(Box<crate::SettingsRequestEnvelopeV1>),
    WorkspaceV1(WorkspaceRequestEnvelopeV1),
    TmuxV1(ClientEnvelope),
}

impl ClientApplicationMessage {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::CodexV1(request) => request.validate(),
            Self::NotificationV1(request) => request.validate(),
            Self::ControlCenterV1(request) => request.validate(),
            Self::InboxV1(request) => request.validate(),
            Self::ResourcesV1(request) => request.validate(),
            Self::SettingsV1(request) => request.validate(),
            Self::WorkspaceV1(request) => request.validate(),
            Self::TmuxV1(request) => request.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HostApplicationMessage {
    CodexV1(Box<crate::CodexResponseEnvelopeV1>),
    NotificationV1(Box<crate::NotificationResponseEnvelopeV1>),
    ControlCenterV1(Box<ControlCenterResponseEnvelopeV1>),
    InboxV1(Box<crate::InboxResponseEnvelopeV1>),
    ResourcesV1(Box<ResourcesResponseEnvelopeV1>),
    SettingsV1(Box<crate::SettingsResponseEnvelopeV1>),
    WorkspaceV1(WorkspaceResponseEnvelopeV1),
    TmuxV1(HostEnvelope),
}

impl HostApplicationMessage {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::CodexV1(response) => response.validate(),
            Self::NotificationV1(response) => response.validate(),
            Self::ControlCenterV1(response) => response.validate(),
            Self::InboxV1(response) => {
                if response.protocol_version != crate::PROTOCOL_VERSION
                    || response.api != crate::InboxApiV1::InboxV1
                {
                    Err(ProtocolValidationError::InvalidIdentifier(
                        "Inbox API version",
                    ))
                } else {
                    validate_stable_identifier(&response.request_id, "Inbox request ID")
                }
            }
            Self::ResourcesV1(response) => response.validate(),
            Self::SettingsV1(response) => response.validate(),
            Self::WorkspaceV1(response) => response.validate(),
            Self::TmuxV1(response) => response.validate(),
        }
    }
}
