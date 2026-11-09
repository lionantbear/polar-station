use polar_tmux_protocol::SshRouteProfileV1;
use serde::{Deserialize, Serialize};

use crate::{
    DisplayMode, ManagedPaneId, SessionKey, WorkspaceCatalog, WorkspaceCommand, WorkspaceId,
    WorkspaceTabId, MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS, MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS,
};

use super::{
    ControlActionId, ControlCenterError, ControlCenterErrorCode, WorkspaceControlContext,
    WorkspaceControlProjection,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum ControlIntent {
    ToggleSessions,
    SetManagedTerminalFontSize {
        points: u8,
    },
    CreateWorkspace {
        name: String,
    },
    SelectWorkspace(WorkspaceId),
    RenameWorkspace {
        name: String,
    },
    SetWorkspaceSshTarget {
        target_id: crate::ExecutionTargetId,
        target_revision: u64,
        destination: String,
        route_profile: Option<SshRouteProfileV1>,
    },
    DeleteWorkspace {
        confirmation_id: String,
    },
    AddTab {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_id: Option<WorkspaceTabId>,
        title: String,
    },
    SelectTab(WorkspaceTabId),
    RenameTab {
        tab_id: WorkspaceTabId,
        title: String,
    },
    DeleteTab {
        tab_id: WorkspaceTabId,
        confirmation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement_tab_id: Option<WorkspaceTabId>,
    },
    ToggleDisplayMode(WorkspaceTabId),
    SelectSession(SessionKey),
    ToggleSessionAssignment {
        tab_id: WorkspaceTabId,
        session: SessionKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<ManagedPaneId>,
    },
    CreateSession {
        execution_target_id: crate::ExecutionTargetId,
        name: String,
        working_directory: Option<String>,
        command: Option<String>,
        auto_restart: bool,
    },
    RenameSession {
        session: SessionKey,
        name: String,
    },
    KillSession {
        session: SessionKey,
        confirmation_id: String,
    },
    SelectPane(ManagedPaneId),
    ActivatePane(ManagedPaneId),
    RemovePane(ManagedPaneId),
    AcknowledgePaneSummary {
        pane_id: ManagedPaneId,
        summary_revision: u64,
    },
    MarkAttentionNotificationHandled {
        pane_id: ManagedPaneId,
        summary_revision: u64,
    },
    ReorderTab {
        tab_id: WorkspaceTabId,
        to_index: usize,
    },
    ReorderPane {
        tab_id: WorkspaceTabId,
        pane_id: ManagedPaneId,
        to_index: usize,
    },
    MovePane {
        pane_id: ManagedPaneId,
        from_tab_id: WorkspaceTabId,
        to_tab_id: WorkspaceTabId,
        to_index: usize,
    },
    InvokeAction {
        action_id: ControlActionId,
        confirmation_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlAuxiliarySurface {
    Resources,
    Iphone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedControlIntent {
    SetSessionsVisible(bool),
    SetSelectedSession(Option<SessionKey>),
    SetSelectedPane(Option<ManagedPaneId>),
    SetWorkspaceSshTarget {
        target_id: crate::ExecutionTargetId,
        target_revision: u64,
        destination: String,
        route_profile: Option<SshRouteProfileV1>,
    },
    Workspace(WorkspaceCommand),
    WorkspaceLifecycleOpen,
    WorkspaceLifecycleApply,
    WorkspaceLifecycleRepair,
    RestoreSavedLayout,
    WorkspaceLifecycleDetach,
    RefreshSessions,
    RestartSession {
        session: SessionKey,
    },
    CreateSession {
        execution_target_id: crate::ExecutionTargetId,
        name: String,
        working_directory: Option<String>,
        command: Option<String>,
        auto_restart: bool,
    },
    RenameSession {
        session: SessionKey,
        name: String,
    },
    KillSession {
        session: SessionKey,
    },
    OpenAuxiliary(ControlAuxiliarySurface),
    OpenMoreMenu,
    ActivatePane(ManagedPaneId),
    AcknowledgePaneSummary {
        pane_id: ManagedPaneId,
        summary_revision: u64,
    },
    MarkAttentionNotificationHandled {
        pane_id: ManagedPaneId,
        summary_revision: u64,
    },
}

pub fn resolve_control_intent(
    catalog: &WorkspaceCatalog,
    context: &WorkspaceControlContext,
    projection: &WorkspaceControlProjection,
    intent: ControlIntent,
) -> Result<ResolvedControlIntent, ControlCenterError> {
    match intent {
        ControlIntent::ToggleSessions => Ok(ResolvedControlIntent::SetSessionsVisible(
            !context.sessions_visible,
        )),
        ControlIntent::SetManagedTerminalFontSize { points } => {
            if !(MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS..=MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS)
                .contains(&points)
            {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::IntentInvalid,
                    "Managed Ghostty font size must be between 10 and 32 points.",
                ));
            }
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::SetManagedTerminalFontSize { points },
            ))
        }
        ControlIntent::CreateWorkspace { name } => Ok(ResolvedControlIntent::Workspace(
            WorkspaceCommand::CreateWorkspace { name },
        )),
        ControlIntent::SelectWorkspace(workspace_id) => {
            require_workspace_projection(projection, workspace_id)?;
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::SelectWorkspace { workspace_id },
            ))
        }
        ControlIntent::RenameWorkspace { name } => Ok(ResolvedControlIntent::Workspace(
            WorkspaceCommand::RenameWorkspace {
                workspace_id: projection.header.selected_workspace_id,
                name,
            },
        )),
        ControlIntent::SetWorkspaceSshTarget {
            target_id,
            target_revision,
            destination,
            route_profile,
        } => {
            if !polar_tmux_protocol::is_canonical_ssh_destination(&destination) {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::IntentInvalid,
                    "Enter one SSH destination such as user@host.",
                ));
            }
            if route_profile
                .as_ref()
                .is_some_and(|profile| profile.validate().is_err())
            {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::IntentInvalid,
                    "Review the LAN/WAN SSH route fields and try again.",
                ));
            }
            Ok(ResolvedControlIntent::SetWorkspaceSshTarget {
                target_id,
                target_revision,
                destination,
                route_profile,
            })
        }
        ControlIntent::DeleteWorkspace { .. } => Err(ControlCenterError::new(
            ControlCenterErrorCode::IntentInvalid,
            "Workspaces cannot be deleted. Use Archive Workspace.",
        )),
        ControlIntent::AddTab { tab_id, title } => {
            Ok(ResolvedControlIntent::Workspace(match tab_id {
                Some(tab_id) => WorkspaceCommand::AddTabWithId {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    title,
                },
                None => WorkspaceCommand::AddTab {
                    workspace_id: projection.header.selected_workspace_id,
                    title,
                },
            }))
        }
        ControlIntent::SelectTab(tab_id) => {
            require_tab_projection(projection, tab_id)?;
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::SelectTab {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                },
            ))
        }
        ControlIntent::RenameTab { tab_id, title } => {
            require_tab_projection(projection, tab_id)?;
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::RenameTab {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    title,
                },
            ))
        }
        ControlIntent::DeleteTab {
            tab_id,
            confirmation_id,
            replacement_tab_id,
        } => {
            let tab = require_tab_projection(projection, tab_id)?;
            require_confirmation(&tab.delete_action, Some(&confirmation_id))?;
            Ok(ResolvedControlIntent::Workspace(match replacement_tab_id {
                Some(replacement_tab_id) => WorkspaceCommand::DeleteTabWithReplacement {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    replacement_tab_id,
                },
                None => WorkspaceCommand::DeleteTab {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                },
            }))
        }
        ControlIntent::ToggleDisplayMode(tab_id) => {
            let tab = require_tab_projection(projection, tab_id)?;
            let display_mode = match tab.display_mode {
                DisplayMode::Monitor => DisplayMode::Focus,
                DisplayMode::Focus => DisplayMode::Monitor,
            };
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::SetDisplayMode {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    display_mode,
                },
            ))
        }
        ControlIntent::SelectSession(session) => {
            if !projection
                .sessions
                .rows
                .iter()
                .any(|row| row.identity == session)
            {
                return Err(component_not_found("Session is not in this projection."));
            }
            Ok(ResolvedControlIntent::SetSelectedSession(Some(session)))
        }
        ControlIntent::ToggleSessionAssignment {
            tab_id,
            session,
            pane_id,
        } => {
            if !projection
                .sessions
                .rows
                .iter()
                .any(|row| row.identity == session)
            {
                return Err(component_not_found("Session is not in this projection."));
            }
            let workspace = catalog.selected_workspace();
            let tab = workspace
                .tab(tab_id)
                .ok_or_else(|| component_not_found("Tab is not in this Workspace."))?;
            if let Some(pane) = tab.panes.iter().find(|pane| {
                pane.execution_target_id == session.execution_target_id
                    && pane.tmux_session_id == session.tmux_session_id
            }) {
                Ok(ResolvedControlIntent::Workspace(
                    WorkspaceCommand::RemovePane {
                        workspace_id: workspace.id,
                        tab_id: tab.id,
                        pane_id: pane.id,
                    },
                ))
            } else {
                Ok(ResolvedControlIntent::Workspace(match pane_id {
                    Some(pane_id) => WorkspaceCommand::AssignSessionWithId {
                        workspace_id: workspace.id,
                        tab_id: tab.id,
                        pane_id,
                        target_id: session.execution_target_id,
                        tmux_session_id: session.tmux_session_id,
                    },
                    None => WorkspaceCommand::AssignSession {
                        workspace_id: workspace.id,
                        tab_id: tab.id,
                        target_id: session.execution_target_id,
                        tmux_session_id: session.tmux_session_id,
                    },
                }))
            }
        }
        ControlIntent::CreateSession {
            execution_target_id,
            name,
            working_directory,
            command,
            auto_restart,
        } => {
            let projected_target = execution_target_id
                == projection.sessions.local_execution_target_id
                || projection
                    .sessions
                    .remote_targets
                    .iter()
                    .any(|target| target.id == execution_target_id);
            if !projected_target {
                return Err(component_not_found(
                    "Session execution target is not in this projection.",
                ));
            }
            Ok(ResolvedControlIntent::CreateSession {
                execution_target_id,
                name,
                working_directory,
                command,
                auto_restart,
            })
        }
        ControlIntent::RenameSession { session, name } => {
            require_session_projection(projection, &session)?;
            Ok(ResolvedControlIntent::RenameSession { session, name })
        }
        ControlIntent::KillSession {
            session,
            confirmation_id,
        } => {
            let row = require_session_projection(projection, &session)?;
            if !row.kill_action.enabled {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::ActionDisabled,
                    row.kill_action
                        .disabled_reason
                        .as_deref()
                        .unwrap_or("Kill Session is disabled."),
                ));
            }
            require_confirmation(&row.kill_action, Some(&confirmation_id))?;
            Ok(ResolvedControlIntent::KillSession { session })
        }
        ControlIntent::SelectPane(pane_id) => {
            require_pane_projection(projection, pane_id)?;
            Ok(ResolvedControlIntent::SetSelectedPane(Some(pane_id)))
        }
        ControlIntent::ActivatePane(pane_id) => {
            require_pane_projection(projection, pane_id)?;
            Ok(ResolvedControlIntent::ActivatePane(pane_id))
        }
        ControlIntent::RemovePane(pane_id) => {
            let tab_id = projection
                .workspace_map
                .tabs
                .iter()
                .find(|tab| tab.panes.iter().any(|pane| pane.id == pane_id))
                .map(|tab| tab.id)
                .ok_or_else(|| component_not_found("Pane is not in this projection."))?;
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::RemovePane {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    pane_id,
                },
            ))
        }
        ControlIntent::AcknowledgePaneSummary {
            pane_id,
            summary_revision,
        } => {
            let pane = require_pane_projection(projection, pane_id)?;
            if pane.attention.summary_revision != summary_revision || summary_revision == 0 {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::ProjectionStale,
                    "The Pane summary changed. Review the refreshed state and try again.",
                ));
            }
            Ok(ResolvedControlIntent::AcknowledgePaneSummary {
                pane_id,
                summary_revision,
            })
        }
        ControlIntent::MarkAttentionNotificationHandled {
            pane_id,
            summary_revision,
        } => {
            let pane = require_pane_projection(projection, pane_id)?;
            if pane.attention.summary_revision != summary_revision || summary_revision == 0 {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::ProjectionStale,
                    "The Pane notification changed. Review the refreshed state and try again.",
                ));
            }
            Ok(ResolvedControlIntent::MarkAttentionNotificationHandled {
                pane_id,
                summary_revision,
            })
        }
        ControlIntent::ReorderTab { tab_id, to_index } => {
            require_tab_projection(projection, tab_id)?;
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::ReorderTab {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    to_index,
                },
            ))
        }
        ControlIntent::ReorderPane {
            tab_id,
            pane_id,
            to_index,
        } => {
            let tab = require_tab_projection(projection, tab_id)?;
            if !tab.panes.iter().any(|pane| pane.id == pane_id) {
                return Err(component_not_found(
                    "Pane is not in the requested source Tab.",
                ));
            }
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::ReorderPane {
                    workspace_id: projection.header.selected_workspace_id,
                    tab_id,
                    pane_id,
                    to_index,
                },
            ))
        }
        ControlIntent::MovePane {
            pane_id,
            from_tab_id,
            to_tab_id,
            to_index,
        } => {
            let source = require_tab_projection(projection, from_tab_id)?;
            require_tab_projection(projection, to_tab_id)?;
            if from_tab_id == to_tab_id || !source.panes.iter().any(|pane| pane.id == pane_id) {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::IntentInvalid,
                    "Pane move must name its current and a different destination Tab.",
                ));
            }
            Ok(ResolvedControlIntent::Workspace(
                WorkspaceCommand::MovePane {
                    workspace_id: projection.header.selected_workspace_id,
                    pane_id,
                    from_tab_id,
                    to_tab_id,
                    to_index,
                },
            ))
        }
        ControlIntent::InvokeAction {
            action_id,
            confirmation_id,
        } => {
            let action = projection.action(&action_id).ok_or_else(|| {
                ControlCenterError::new(
                    ControlCenterErrorCode::ActionNotProjected,
                    "Control Center action is not in the current projection.",
                )
            })?;
            if !action.enabled {
                return Err(ControlCenterError::new(
                    ControlCenterErrorCode::ActionDisabled,
                    action
                        .disabled_reason
                        .as_deref()
                        .unwrap_or("Control Center action is disabled."),
                ));
            }
            require_confirmation(action, confirmation_id.as_deref())?;
            resolve_action(catalog, context, projection, action_id)
        }
    }
}

fn resolve_action(
    catalog: &WorkspaceCatalog,
    context: &WorkspaceControlContext,
    projection: &WorkspaceControlProjection,
    action_id: ControlActionId,
) -> Result<ResolvedControlIntent, ControlCenterError> {
    if projection.sessions.rows.iter().any(|row| {
        row.replacement_action
            .as_ref()
            .is_some_and(|action| action.id == action_id)
    }) {
        return Err(ControlCenterError::new(
            ControlCenterErrorCode::IntentInvalid,
            "Session replacement requires its explicit typed confirmation intent.",
        ));
    }
    if let Some(row) = projection.sessions.rows.iter().find(|row| {
        row.restart_action
            .as_ref()
            .is_some_and(|action| action.id == action_id)
    }) {
        return Ok(ResolvedControlIntent::RestartSession {
            session: row.identity.clone(),
        });
    }
    match action_id.as_str() {
        "workspace.open" => Ok(ResolvedControlIntent::WorkspaceLifecycleOpen),
        "workspace.apply" => Ok(ResolvedControlIntent::WorkspaceLifecycleApply),
        "workspace.repair" => Ok(ResolvedControlIntent::WorkspaceLifecycleRepair),
        "workspace.restore" => Ok(ResolvedControlIntent::RestoreSavedLayout),
        "workspace.detach" => Ok(ResolvedControlIntent::WorkspaceLifecycleDetach),
        "workspace.create" | "workspace.rename" | "workspace.delete" => {
            Err(ControlCenterError::new(
                ControlCenterErrorCode::IntentInvalid,
                "This Control Center action requires a typed editor intent.",
            ))
        }
        "workspace.tab.add" => Ok(ResolvedControlIntent::Workspace(WorkspaceCommand::AddTab {
            workspace_id: projection.header.selected_workspace_id,
            title: next_tab_title(catalog),
        })),
        "sessions.toggle" => Ok(ResolvedControlIntent::SetSessionsVisible(
            !context.sessions_visible,
        )),
        "sessions.refresh" => Ok(ResolvedControlIntent::RefreshSessions),
        "ssh.target.retry" => Err(ControlCenterError::new(
            ControlCenterErrorCode::IntentInvalid,
            "SSH recovery is owned by the connection badge and target editor.",
        )),
        "workspace.mode.toggle" => resolve_control_intent(
            catalog,
            context,
            projection,
            ControlIntent::ToggleDisplayMode(projection.workspace_map.selected_tab_id),
        ),
        "workspace.focus.previous" => Ok(ResolvedControlIntent::Workspace(
            WorkspaceCommand::FocusPrevious {
                workspace_id: projection.header.selected_workspace_id,
            },
        )),
        "workspace.focus.next" => Ok(ResolvedControlIntent::Workspace(
            WorkspaceCommand::FocusNext {
                workspace_id: projection.header.selected_workspace_id,
            },
        )),
        "resources.open" => Ok(ResolvedControlIntent::OpenAuxiliary(
            ControlAuxiliarySurface::Resources,
        )),
        "iphone.open" => Ok(ResolvedControlIntent::OpenAuxiliary(
            ControlAuxiliarySurface::Iphone,
        )),
        "more.open" => Ok(ResolvedControlIntent::OpenMoreMenu),
        _ => {
            if let Some(slot) = action_id
                .as_str()
                .strip_prefix("workspace.focus.sidebar.")
                .and_then(|value| value.parse::<u8>().ok())
            {
                Ok(ResolvedControlIntent::Workspace(
                    WorkspaceCommand::FocusSidebarSlot {
                        workspace_id: projection.header.selected_workspace_id,
                        slot,
                    },
                ))
            } else {
                Err(ControlCenterError::new(
                    ControlCenterErrorCode::IntentInvalid,
                    "Control Center action has no typed resolver.",
                ))
            }
        }
    }
}

fn require_confirmation(
    action: &super::ControlActionProjection,
    confirmation_id: Option<&str>,
) -> Result<(), ControlCenterError> {
    match (&action.confirmation, confirmation_id) {
        (None, None) => Ok(()),
        (Some(expected), Some(actual)) if expected.id == actual => Ok(()),
        _ => Err(ControlCenterError::new(
            ControlCenterErrorCode::ConfirmationMismatch,
            "The confirmation does not match the current projected action.",
        )),
    }
}

fn next_tab_title(catalog: &WorkspaceCatalog) -> String {
    format!("Tab {}", catalog.selected_workspace().tabs.len() + 1)
}

fn require_workspace_projection(
    projection: &WorkspaceControlProjection,
    workspace_id: WorkspaceId,
) -> Result<(), ControlCenterError> {
    if projection
        .header
        .workspace_choices
        .iter()
        .any(|workspace| workspace.id == workspace_id)
    {
        Ok(())
    } else {
        Err(component_not_found("Workspace is not in this projection."))
    }
}

fn require_tab_projection(
    projection: &WorkspaceControlProjection,
    tab_id: WorkspaceTabId,
) -> Result<&super::ControlTabProjection, ControlCenterError> {
    projection
        .workspace_map
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(|| component_not_found("Tab is not in this projection."))
}

fn require_pane_projection(
    projection: &WorkspaceControlProjection,
    pane_id: ManagedPaneId,
) -> Result<&super::ControlPaneProjection, ControlCenterError> {
    projection
        .workspace_map
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .find(|pane| pane.id == pane_id)
        .ok_or_else(|| component_not_found("Pane is not in this projection."))
}

fn require_session_projection<'a>(
    projection: &'a WorkspaceControlProjection,
    session: &SessionKey,
) -> Result<&'a super::SessionRowProjection, ControlCenterError> {
    projection
        .sessions
        .rows
        .iter()
        .find(|row| row.identity == *session)
        .ok_or_else(|| component_not_found("Session is not in this projection."))
}

fn component_not_found(message: &'static str) -> ControlCenterError {
    ControlCenterError::new(ControlCenterErrorCode::ComponentNotFound, message)
}
