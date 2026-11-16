use serde::{Deserialize, Serialize};

use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::ids::{
    CatalogRevision, ExecutionTargetId, ManagedPaneId, StableTmuxSessionId, WorkspaceId,
    WorkspaceRevision, WorkspaceTabId,
};
use crate::layout::{canonical_layout, LayoutRole};
use crate::model::{
    DisplayMode, ManagedPane, Workspace, WorkspaceCatalog, WorkspaceTab,
    MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS, MAX_PANES_PER_TAB,
    MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS,
};
use crate::validation::{validate_catalog, validate_label};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub expected_catalog_revision: CatalogRevision,
    pub expected_workspace_revision: Option<WorkspaceRevision>,
    pub command: WorkspaceCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum WorkspaceCommand {
    SetManagedTerminalFontSize {
        points: u8,
    },
    CreateWorkspace {
        name: String,
    },
    SelectWorkspace {
        workspace_id: WorkspaceId,
    },
    RenameWorkspace {
        workspace_id: WorkspaceId,
        name: String,
    },
    DeleteWorkspace {
        workspace_id: WorkspaceId,
    },
    AddTab {
        workspace_id: WorkspaceId,
        title: String,
    },
    AddTabWithId {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        title: String,
    },
    SelectTab {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
    },
    RenameTab {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        title: String,
    },
    ReorderTab {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        to_index: usize,
    },
    DeleteTab {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
    },
    DeleteTabWithReplacement {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        replacement_tab_id: WorkspaceTabId,
    },
    AssignSession {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        target_id: ExecutionTargetId,
        tmux_session_id: StableTmuxSessionId,
    },
    AssignSessionWithId {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        pane_id: ManagedPaneId,
        target_id: ExecutionTargetId,
        tmux_session_id: StableTmuxSessionId,
    },
    RemovePane {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        pane_id: ManagedPaneId,
    },
    ReorderPane {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        pane_id: ManagedPaneId,
        to_index: usize,
    },
    MovePane {
        workspace_id: WorkspaceId,
        pane_id: ManagedPaneId,
        from_tab_id: WorkspaceTabId,
        to_tab_id: WorkspaceTabId,
        to_index: usize,
    },
    SetDisplayMode {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        display_mode: DisplayMode,
    },
    SelectFocusPane {
        workspace_id: WorkspaceId,
        tab_id: WorkspaceTabId,
        pane_id: ManagedPaneId,
    },
    FocusPrevious {
        workspace_id: WorkspaceId,
    },
    FocusNext {
        workspace_id: WorkspaceId,
    },
    FocusSidebarSlot {
        workspace_id: WorkspaceId,
        slot: u8,
    },
}

impl WorkspaceCommand {
    pub const fn workspace_id(&self) -> Option<WorkspaceId> {
        match self {
            Self::SetManagedTerminalFontSize { .. } | Self::CreateWorkspace { .. } => None,
            Self::SelectWorkspace { workspace_id }
            | Self::RenameWorkspace { workspace_id, .. }
            | Self::DeleteWorkspace { workspace_id }
            | Self::AddTab { workspace_id, .. }
            | Self::AddTabWithId { workspace_id, .. }
            | Self::SelectTab { workspace_id, .. }
            | Self::RenameTab { workspace_id, .. }
            | Self::ReorderTab { workspace_id, .. }
            | Self::DeleteTab { workspace_id, .. }
            | Self::DeleteTabWithReplacement { workspace_id, .. }
            | Self::AssignSession { workspace_id, .. }
            | Self::AssignSessionWithId { workspace_id, .. }
            | Self::RemovePane { workspace_id, .. }
            | Self::ReorderPane { workspace_id, .. }
            | Self::MovePane { workspace_id, .. }
            | Self::SetDisplayMode { workspace_id, .. }
            | Self::SelectFocusPane { workspace_id, .. }
            | Self::FocusPrevious { workspace_id }
            | Self::FocusNext { workspace_id }
            | Self::FocusSidebarSlot { workspace_id, .. } => Some(*workspace_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingRuntimeImpact {
    None,
    InPlaceCandidate,
    ReplacementMayBeRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTransition {
    pub catalog: WorkspaceCatalog,
    pub impact: PendingRuntimeImpact,
    pub automatic_apply_candidate: bool,
}

pub struct WorkspaceEngine;

impl WorkspaceEngine {
    pub fn apply(
        catalog: &WorkspaceCatalog,
        envelope: CommandEnvelope,
    ) -> Result<WorkspaceTransition, WorkspaceError> {
        if envelope.expected_catalog_revision != catalog.revision {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::RevisionStale,
                "catalog revision is stale",
            ));
        }
        if let Some(workspace_id) = envelope.command.workspace_id() {
            let workspace = catalog.workspace(workspace_id).ok_or_else(|| {
                WorkspaceError::new(
                    WorkspaceErrorCode::WorkspaceNotFound,
                    "Workspace does not exist",
                )
            })?;
            if !matches!(envelope.command, WorkspaceCommand::SelectWorkspace { .. })
                && envelope.expected_workspace_revision != Some(workspace.revision)
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::RevisionStale,
                    "Workspace revision is stale",
                ));
            }
        }

        let mut candidate = catalog.clone();
        let impact = apply_to_candidate(&mut candidate, envelope.command)?;
        candidate.revision = candidate.revision.next();
        validate_catalog(&candidate)?;
        Ok(WorkspaceTransition {
            automatic_apply_candidate: impact == PendingRuntimeImpact::InPlaceCandidate,
            catalog: candidate,
            impact,
        })
    }
}

fn apply_to_candidate(
    catalog: &mut WorkspaceCatalog,
    command: WorkspaceCommand,
) -> Result<PendingRuntimeImpact, WorkspaceError> {
    match command {
        WorkspaceCommand::SetManagedTerminalFontSize { points } => {
            if !(MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS..=MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS)
                .contains(&points)
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "managed Ghostty font size must be between 10 and 32 points",
                ));
            }
            catalog.managed_terminal_font_size_points = points;
            Ok(PendingRuntimeImpact::None)
        }
        WorkspaceCommand::CreateWorkspace { name } => {
            let workspace = Workspace::new(name)?;
            catalog.selected_workspace_id = workspace.id;
            catalog.workspaces.push(workspace);
            Ok(PendingRuntimeImpact::None)
        }
        WorkspaceCommand::SelectWorkspace { workspace_id } => {
            catalog.selected_workspace_id = workspace_id;
            Ok(PendingRuntimeImpact::None)
        }
        WorkspaceCommand::RenameWorkspace { workspace_id, name } => {
            validate_label(
                &name,
                "workspace name",
                WorkspaceErrorCode::WorkspaceNameInvalid,
            )?;
            let workspace = workspace_mut(catalog, workspace_id)?;
            workspace.name = name;
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::None)
        }
        WorkspaceCommand::DeleteWorkspace { workspace_id } => {
            let index = catalog
                .workspaces
                .iter()
                .position(|workspace| workspace.id == workspace_id)
                .ok_or_else(workspace_not_found)?;
            catalog.workspaces.remove(index);
            if catalog.workspaces.is_empty() {
                let replacement = Workspace::new("Workspace")?;
                catalog.selected_workspace_id = replacement.id;
                catalog.workspaces.push(replacement);
            } else if catalog.selected_workspace_id == workspace_id {
                let selected_index = index.min(catalog.workspaces.len() - 1);
                catalog.selected_workspace_id = catalog.workspaces[selected_index].id;
            }
            Ok(PendingRuntimeImpact::ReplacementMayBeRequired)
        }
        WorkspaceCommand::AddTab {
            workspace_id,
            title,
        } => {
            let tab = WorkspaceTab::new(title)?;
            let workspace = workspace_mut(catalog, workspace_id)?;
            workspace.selected_tab_id = tab.id;
            workspace.selected_pane_id = None;
            workspace.tabs.push(tab);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::AddTabWithId {
            workspace_id,
            tab_id,
            title,
        } => {
            let tab = WorkspaceTab::new_with_id(tab_id, title)?;
            let workspace = workspace_mut(catalog, workspace_id)?;
            workspace.selected_tab_id = tab.id;
            workspace.selected_pane_id = None;
            workspace.tabs.push(tab);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::SelectTab {
            workspace_id,
            tab_id,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let selected_pane_id = require_tab(workspace, tab_id)?
                .visual_pane_ids
                .first()
                .copied();
            workspace.selected_tab_id = tab_id;
            workspace.selected_pane_id = selected_pane_id;
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::RenameTab {
            workspace_id,
            tab_id,
            title,
        } => {
            validate_label(&title, "tab title", WorkspaceErrorCode::TabTitleInvalid)?;
            let workspace = workspace_mut(catalog, workspace_id)?;
            tab_mut(workspace, tab_id)?.title = title;
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::ReorderTab {
            workspace_id,
            tab_id,
            to_index,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let from_index = workspace
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .ok_or_else(tab_not_found)?;
            if to_index >= workspace.tabs.len() {
                return Err(tab_not_found());
            }
            let tab = workspace.tabs.remove(from_index);
            workspace.tabs.insert(to_index, tab);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::DeleteTab {
            workspace_id,
            tab_id,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let index = workspace
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .ok_or_else(tab_not_found)?;
            let removed = workspace.tabs.remove(index);
            if workspace
                .selected_pane_id
                .is_some_and(|selected| removed.panes.iter().any(|pane| pane.id == selected))
            {
                workspace.selected_pane_id = None;
            }
            if workspace.tabs.is_empty() {
                let replacement = WorkspaceTab::new("Main")?;
                workspace.selected_tab_id = replacement.id;
                workspace.selected_pane_id = None;
                workspace.tabs.push(replacement);
            } else if workspace.selected_tab_id == tab_id {
                workspace.selected_tab_id = workspace.tabs[index.min(workspace.tabs.len() - 1)].id;
                workspace.selected_pane_id = workspace.tabs[index.min(workspace.tabs.len() - 1)]
                    .visual_pane_ids
                    .first()
                    .copied();
            }
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::ReplacementMayBeRequired)
        }
        WorkspaceCommand::DeleteTabWithReplacement {
            workspace_id,
            tab_id,
            replacement_tab_id,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let index = workspace
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .ok_or_else(tab_not_found)?;
            if workspace.tabs.len() != 1
                || workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.id == replacement_tab_id)
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::TabNotFound,
                    "replacement tab identity is valid only when deleting the final tab",
                ));
            }
            let replacement = WorkspaceTab::new_with_id(replacement_tab_id, "Main".to_owned())?;
            workspace.tabs.remove(index);
            workspace.selected_tab_id = replacement.id;
            workspace.selected_pane_id = None;
            workspace.tabs.push(replacement);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::ReplacementMayBeRequired)
        }
        WorkspaceCommand::AssignSession {
            workspace_id,
            tab_id,
            target_id,
            tmux_session_id,
        } => assign_session(
            catalog,
            workspace_id,
            tab_id,
            None,
            target_id,
            tmux_session_id,
        ),
        WorkspaceCommand::AssignSessionWithId {
            workspace_id,
            tab_id,
            pane_id,
            target_id,
            tmux_session_id,
        } => assign_session(
            catalog,
            workspace_id,
            tab_id,
            Some(pane_id),
            target_id,
            tmux_session_id,
        ),
        WorkspaceCommand::RemovePane {
            workspace_id,
            tab_id,
            pane_id,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let removed_was_selected = workspace.selected_pane_id == Some(pane_id);
            let replacement_selection = {
                let tab = tab_mut(workspace, tab_id)?;
                let index = tab
                    .panes
                    .iter()
                    .position(|pane| pane.id == pane_id)
                    .ok_or_else(pane_not_found)?;
                let removed_was_focused = tab.focused_pane_id == Some(pane_id);
                tab.panes.remove(index);
                tab.visual_pane_ids
                    .retain(|candidate| *candidate != pane_id);
                if tab.panes.is_empty() {
                    tab.display_mode = DisplayMode::Monitor;
                    tab.focused_pane_id = None;
                } else if removed_was_focused {
                    tab.focused_pane_id = Some(tab.panes[index.min(tab.panes.len() - 1)].id);
                }
                tab.visual_pane_ids.first().copied()
            };
            if removed_was_selected {
                workspace.selected_pane_id = replacement_selection;
            }
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::ReplacementMayBeRequired)
        }
        WorkspaceCommand::ReorderPane {
            workspace_id,
            tab_id,
            pane_id,
            to_index,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let tab = tab_mut(workspace, tab_id)?;
            let from_index = tab
                .panes
                .iter()
                .position(|pane| pane.id == pane_id)
                .ok_or_else(pane_not_found)?;
            if to_index >= tab.panes.len() {
                return Err(pane_not_found());
            }
            let pane = tab.panes.remove(from_index);
            tab.panes.insert(to_index, pane);
            let visual_from_index = tab
                .visual_pane_ids
                .iter()
                .position(|candidate| *candidate == pane_id)
                .ok_or_else(pane_not_found)?;
            let visual_pane = tab.visual_pane_ids.remove(visual_from_index);
            tab.visual_pane_ids.insert(to_index, visual_pane);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::MovePane {
            workspace_id,
            pane_id,
            from_tab_id,
            to_tab_id,
            to_index,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let from_index = workspace
                .tabs
                .iter()
                .position(|tab| tab.id == from_tab_id)
                .ok_or_else(tab_not_found)?;
            let to_tab_index = workspace
                .tabs
                .iter()
                .position(|tab| tab.id == to_tab_id)
                .ok_or_else(tab_not_found)?;
            let pane_index = workspace.tabs[from_index]
                .panes
                .iter()
                .position(|pane| pane.id == pane_id)
                .ok_or_else(pane_not_found)?;
            let destination_len = workspace.tabs[to_tab_index].panes.len();
            let destination_len_after_removal = if from_index == to_tab_index {
                destination_len.saturating_sub(1)
            } else {
                destination_len
            };
            if to_index > destination_len_after_removal
                || (from_index != to_tab_index && destination_len >= MAX_PANES_PER_TAB)
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::PaneCapacity,
                    "destination tab cannot accept the pane",
                ));
            }
            let pane = workspace.tabs[from_index].panes.remove(pane_index);
            workspace.tabs[from_index]
                .visual_pane_ids
                .retain(|candidate| *candidate != pane_id);
            normalize_focus_after_removal(&mut workspace.tabs[from_index], pane_id, pane_index);
            workspace.tabs[to_tab_index].panes.insert(to_index, pane);
            workspace.tabs[to_tab_index]
                .visual_pane_ids
                .insert(to_index, pane_id);
            workspace.selected_tab_id = to_tab_id;
            workspace.selected_pane_id = Some(pane_id);
            if workspace.tabs[to_tab_index].display_mode == DisplayMode::Focus {
                workspace.tabs[to_tab_index].focused_pane_id = Some(pane_id);
            }
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::ReplacementMayBeRequired)
        }
        WorkspaceCommand::SetDisplayMode {
            workspace_id,
            tab_id,
            display_mode,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let tab = tab_mut(workspace, tab_id)?;
            if display_mode == DisplayMode::Focus && tab.panes.is_empty() {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::FocusInvalid,
                    "an empty tab cannot enter Focus mode",
                ));
            }
            tab.display_mode = display_mode;
            if display_mode == DisplayMode::Focus && tab.focused_pane_id.is_none() {
                tab.focused_pane_id = tab.panes.first().map(|pane| pane.id);
            }
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::SelectFocusPane {
            workspace_id,
            tab_id,
            pane_id,
        } => {
            let workspace = workspace_mut(catalog, workspace_id)?;
            let tab = tab_mut(workspace, tab_id)?;
            if !tab.panes.iter().any(|pane| pane.id == pane_id) {
                return Err(pane_not_found());
            }
            tab.focused_pane_id = Some(pane_id);
            workspace.selected_tab_id = tab_id;
            workspace.selected_pane_id = Some(pane_id);
            bump_workspace(workspace);
            Ok(PendingRuntimeImpact::InPlaceCandidate)
        }
        WorkspaceCommand::FocusPrevious { workspace_id } => {
            move_focus(catalog, workspace_id, FocusMove::Previous)
        }
        WorkspaceCommand::FocusNext { workspace_id } => {
            move_focus(catalog, workspace_id, FocusMove::Next)
        }
        WorkspaceCommand::FocusSidebarSlot { workspace_id, slot } => {
            move_focus(catalog, workspace_id, FocusMove::SidebarSlot(slot))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FocusMove {
    Previous,
    Next,
    SidebarSlot(u8),
}

fn move_focus(
    catalog: &mut WorkspaceCatalog,
    workspace_id: WorkspaceId,
    movement: FocusMove,
) -> Result<PendingRuntimeImpact, WorkspaceError> {
    let workspace = workspace_mut(catalog, workspace_id)?;
    let tab = workspace
        .tabs
        .iter_mut()
        .find(|tab| tab.id == workspace.selected_tab_id)
        .ok_or_else(tab_not_found)?;
    if tab.display_mode != DisplayMode::Focus || tab.panes.is_empty() {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::FocusInvalid,
            "focus navigation requires a non-empty tab in Focus mode",
        ));
    }
    let layout = canonical_layout(tab)?;
    let focused = tab.focused_pane_id.ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorCode::FocusInvalid,
            "Focus mode requires a focused pane",
        )
    })?;
    let next = match movement {
        FocusMove::Previous | FocusMove::Next => {
            let index = layout
                .canonical_pane_order
                .iter()
                .position(|pane_id| *pane_id == focused)
                .ok_or_else(pane_not_found)?;
            let len = layout.canonical_pane_order.len();
            let next_index = match movement {
                FocusMove::Previous => (index + len - 1) % len,
                FocusMove::Next => (index + 1) % len,
                FocusMove::SidebarSlot(_) => unreachable!(),
            };
            layout.canonical_pane_order[next_index]
        }
        FocusMove::SidebarSlot(slot) => layout
            .slots
            .iter()
            .find_map(|candidate| {
                (candidate.role == LayoutRole::Sidebar { slot }).then_some(candidate.pane_id)
            })
            .ok_or_else(|| {
                WorkspaceError::new(
                    WorkspaceErrorCode::FocusInvalid,
                    "Focus sidebar slot does not exist",
                )
            })?,
    };
    tab.focused_pane_id = Some(next);
    bump_workspace(workspace);
    Ok(PendingRuntimeImpact::InPlaceCandidate)
}

fn workspace_mut(
    catalog: &mut WorkspaceCatalog,
    workspace_id: WorkspaceId,
) -> Result<&mut Workspace, WorkspaceError> {
    catalog
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(workspace_not_found)
}

fn assign_session(
    catalog: &mut WorkspaceCatalog,
    workspace_id: WorkspaceId,
    tab_id: WorkspaceTabId,
    pane_id: Option<ManagedPaneId>,
    target_id: ExecutionTargetId,
    tmux_session_id: StableTmuxSessionId,
) -> Result<PendingRuntimeImpact, WorkspaceError> {
    let workspace = workspace_mut(catalog, workspace_id)?;
    if workspace
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .any(|pane| {
            pane.execution_target_id == target_id && pane.tmux_session_id == tmux_session_id
        })
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::SessionDuplicate,
            "target/session pair is already assigned",
        ));
    }
    let tab = tab_mut(workspace, tab_id)?;
    if tab.panes.len() >= MAX_PANES_PER_TAB {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::PaneCapacity,
            "a tab can contain at most nine panes",
        ));
    }
    let pane = if let Some(pane_id) = pane_id {
        ManagedPane::new_with_id(pane_id, target_id, tmux_session_id)
    } else {
        ManagedPane::new(target_id, tmux_session_id)
    };
    if tab.display_mode == DisplayMode::Focus && tab.focused_pane_id.is_none() {
        tab.focused_pane_id = Some(pane.id);
    }
    let pane_id = pane.id;
    tab.panes.push(pane);
    tab.visual_pane_ids.push(pane_id);
    workspace.selected_tab_id = tab_id;
    workspace.selected_pane_id = Some(pane_id);
    bump_workspace(workspace);
    Ok(PendingRuntimeImpact::InPlaceCandidate)
}

fn require_tab(
    workspace: &Workspace,
    tab_id: WorkspaceTabId,
) -> Result<&WorkspaceTab, WorkspaceError> {
    workspace
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(tab_not_found)
}

fn tab_mut(
    workspace: &mut Workspace,
    tab_id: WorkspaceTabId,
) -> Result<&mut WorkspaceTab, WorkspaceError> {
    workspace
        .tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(tab_not_found)
}

fn bump_workspace(workspace: &mut Workspace) {
    workspace.revision = workspace.revision.next();
}

fn normalize_focus_after_removal(
    tab: &mut WorkspaceTab,
    removed_pane_id: ManagedPaneId,
    removed_index: usize,
) {
    if tab.panes.is_empty() {
        tab.display_mode = DisplayMode::Monitor;
        tab.focused_pane_id = None;
    } else if tab.focused_pane_id == Some(removed_pane_id) {
        tab.focused_pane_id = Some(tab.panes[removed_index.min(tab.panes.len() - 1)].id);
    }
}

fn workspace_not_found() -> WorkspaceError {
    WorkspaceError::new(
        WorkspaceErrorCode::WorkspaceNotFound,
        "Workspace does not exist",
    )
}

fn tab_not_found() -> WorkspaceError {
    WorkspaceError::new(WorkspaceErrorCode::TabNotFound, "tab does not exist")
}

fn pane_not_found() -> WorkspaceError {
    WorkspaceError::new(WorkspaceErrorCode::PaneNotFound, "pane does not exist")
}
