use std::fmt;

use serde::{Deserialize, Serialize};



#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCenterErrorCode {
    ProjectionStale,
    ActionNotProjected,
    ActionDisabled,
    ConfirmationMismatch,
    ComponentNotFound,
    FrontendSessionNotFound,
    SessionReferenced,
    AssignmentCapacity,
    PresentationUnavailable,
    IntentInvalid,
    Internal,
}

impl ControlCenterErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectionStale => "control_projection_stale",
            Self::ActionNotProjected => "control_action_not_projected",
            Self::ActionDisabled => "control_action_disabled",
            Self::ConfirmationMismatch => "control_confirmation_mismatch",
            Self::ComponentNotFound => "control_component_not_found",
            Self::FrontendSessionNotFound => "control_frontend_session_not_found",
            Self::SessionReferenced => "control_session_referenced",
            Self::AssignmentCapacity => "control_assignment_capacity",
            Self::PresentationUnavailable => "control_presentation_unavailable",
            Self::IntentInvalid => "control_intent_invalid",
            Self::Internal => "control_internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCenterError {
    code: ControlCenterErrorCode,
    message: String,
}

impl ControlCenterError {
    pub fn new(code: ControlCenterErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> ControlCenterErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ControlCenterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ControlCenterError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ControlActionId(String);

impl ControlActionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ControlCenterError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value.trim() == value
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.');
        if !valid {
            return Err(ControlCenterError::new(
                ControlCenterErrorCode::IntentInvalid,
                "Control Center action ID is invalid.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ControlActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionRole {
    Primary,
    Secondary,
    Toggle,
    Recovery,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlConfirmation {
    pub id: String,
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlActionProjection {
    pub id: ControlActionId,
    pub label: String,
    pub role: ControlActionRole,
    pub enabled: bool,
    pub selected: bool,
    pub disabled_reason: Option<String>,
    pub confirmation: Option<ControlConfirmation>,
    pub accessibility_label: String,
}

impl ControlActionProjection {
    /// Shared menu contract; each desktop executes its existing native window factory.
    pub fn new_workspace_window() -> Self {
        Self::enabled(
            "workspace.window.new",
            "New Window",
            ControlActionRole::Primary,
        )
    }

    pub fn archive_workspace() -> Self {
        Self::enabled("workspace.archive", "Archive Workspace", ControlActionRole::Secondary)
            .with_confirmation("archive-workspace", "Archive Workspace?",
                "Keep its layout and bindings for Restore. Background Sessions keep running.", "Archive Workspace")
    }
    pub fn close_window() -> Self {
        Self::enabled("window.close", "Close Window", ControlActionRole::Secondary)
    }

    pub fn enabled(
        id: impl Into<String>,
        label: impl Into<String>,
        role: ControlActionRole,
    ) -> Self {
        let label = label.into();
        Self {
            id: ControlActionId::new(id).expect("valid Control Center action ID"),
            label: label.clone(),
            role,
            enabled: true,
            selected: false,
            disabled_reason: None,
            confirmation: None,
            accessibility_label: label,
        }
    }

    pub(crate) fn disabled(
        id: &'static str,
        label: &'static str,
        role: ControlActionRole,
        reason: &'static str,
    ) -> Self {
        Self {
            enabled: false,
            disabled_reason: Some(reason.to_owned()),
            ..Self::enabled(id, label, role)
        }
    }

    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn with_confirmation(
        mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: impl Into<String>,
    ) -> Self {
        self.confirmation = Some(ControlConfirmation {
            id: id.into(),
            title: title.into(),
            message: message.into(),
            confirm_label: confirm_label.into(),
            destructive: self.role == ControlActionRole::Destructive,
        });
        self
    }
}

/// Local UI policy over a borrowed native catalog. This is not a Host model or
/// another catalog: callers keep storage, layout and window objects native.
#[derive(Debug, Deserialize)]
pub struct WorkspaceLifecycleInput {
    pub action: String,
    pub ids: Vec<String>,
    #[serde(default)]
    pub archived: Vec<String>,
    #[serde(default)]
    pub open: Vec<String>,
    pub selected: String,
    pub bound: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub tab_count: usize,
    #[serde(default)]
    pub tab_index: Option<usize>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WorkspaceLifecycleDecision {
    pub effect: String,
    pub selected: String,
    pub error: Option<String>,
    pub can_archive: bool,
}

pub fn decide_workspace_lifecycle(input: WorkspaceLifecycleInput) -> WorkspaceLifecycleDecision {
    if input.action.starts_with("native.") { return super::native_action::decide(&input); }
    let active: Vec<_> = input.ids.iter().filter(|id| !input.archived.contains(id)).collect();
    let mut result = WorkspaceLifecycleDecision {
        effect: "none".into(), selected: input.selected.clone(), error: None,
        can_archive: active.len() > 1 && input.bound.as_ref().is_some_and(|id| active.contains(&id)),
    };
    let invalid = input.ids.iter().any(|id| id.is_empty())
        || input.ids.iter().collect::<std::collections::HashSet<_>>().len() != input.ids.len()
        || input.archived.iter().collect::<std::collections::HashSet<_>>().len() != input.archived.len()
        || input.archived.iter().any(|id| !input.ids.contains(id))
        || active.is_empty() || !active.contains(&&input.selected)
        || input.bound.as_ref().is_some_and(|id| !active.contains(&id));
    let target = input.target.as_ref();
    let error = if invalid {
        Some("Invalid Workspace catalog or expired window binding.")
    } else {
        match input.action.as_str() {
            "inspect" => None,
            "new_window" => { result.effect = "new_window".into(); None }
            "close" | "workspace.detach" => { result.effect = "close".into(); None }
            "create" => { result.effect = "create".into(); None }
            "delete" | "workspace.delete" => Some("Workspaces cannot be deleted. Use Archive Workspace."),
            "open" if target.is_some_and(|id| active.contains(&id)) => {
                result.effect = if target.is_some_and(|id| input.open.contains(id) && input.bound.as_ref() != Some(id)) { "focus" } else { "bind" }.into();
                result.selected = target.unwrap().clone(); None
            }
            "archive" if target != input.bound.as_ref() || target.is_none() => Some("The Workspace that requested archive is no longer bound to this window."),
            "archive" if active.len() == 1 => Some("Keep at least one unarchived Workspace."),
            "archive" => {
                result.effect = "archive".into();
                if target == Some(&input.selected) {
                    result.selected = (*active.iter().find(|id| Some(**id) != target).expect("another active Workspace")).clone();
                }
                None
            }
            "restore" if target.is_some_and(|id| input.archived.contains(id)) => { result.effect = "restore".into(); None }
            _ => Some("Workspace action is unavailable or its target has changed."),
        }
    };
    result.error = error.map(str::to_owned);
    result
}


