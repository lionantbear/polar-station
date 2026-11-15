use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceErrorCode {
    WorkspaceNotFound,
    RevisionStale,
    WorkspaceNameInvalid,
    TabNotFound,
    TabTitleInvalid,
    PaneNotFound,
    PaneCapacity,
    SessionDuplicate,
    FocusInvalid,
    SessionReferenced,
    RuntimeMissing,
    ObservationStale,
    FrontendCapabilityMissing,
    ReplacementConfirmationRequired,
    TransactionBusy,
    RequestIdConflict,
    TransactionFailed,
    RollbackIncomplete,
    InvalidCatalog,
    SchemaUnsupported,
}

impl WorkspaceErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::RevisionStale => "workspace_revision_stale",
            Self::WorkspaceNameInvalid => "workspace_name_invalid",
            Self::TabNotFound => "workspace_tab_not_found",
            Self::TabTitleInvalid => "workspace_tab_title_invalid",
            Self::PaneNotFound => "workspace_pane_not_found",
            Self::PaneCapacity => "workspace_pane_capacity",
            Self::SessionDuplicate => "workspace_session_duplicate",
            Self::FocusInvalid => "workspace_focus_invalid",
            Self::SessionReferenced => "workspace_session_referenced",
            Self::RuntimeMissing => "workspace_runtime_missing",
            Self::ObservationStale => "workspace_observation_stale",
            Self::FrontendCapabilityMissing => "workspace_frontend_capability_missing",
            Self::ReplacementConfirmationRequired => "workspace_replacement_confirmation_required",
            Self::TransactionBusy => "workspace_transaction_busy",
            Self::RequestIdConflict => "request_id_conflict",
            Self::TransactionFailed => "workspace_transaction_failed",
            Self::RollbackIncomplete => "workspace_rollback_incomplete",
            Self::InvalidCatalog => "workspace_catalog_invalid",
            Self::SchemaUnsupported => "workspace_schema_unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceError {
    code: WorkspaceErrorCode,
    message: String,
}

impl WorkspaceError {
    pub fn new(code: WorkspaceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> WorkspaceErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for WorkspaceError {}
