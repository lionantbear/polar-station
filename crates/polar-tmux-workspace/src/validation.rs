use std::collections::BTreeSet;

use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::ids::{ManagedPaneId, WorkspaceId, WorkspaceTabId};
use crate::model::{
    DisplayMode, WorkspaceCatalog, MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS, MAX_PANES_PER_TAB,
    MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS, WORKSPACE_SCHEMA_VERSION,
};

pub(crate) fn validate_label(
    value: &str,
    field: &str,
    code: WorkspaceErrorCode,
) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 120
        || value.chars().any(char::is_control)
    {
        return Err(WorkspaceError::new(code, format!("{field} is invalid")));
    }
    Ok(())
}

pub fn validate_catalog(catalog: &WorkspaceCatalog) -> Result<(), WorkspaceError> {
    if catalog.schema_version != WORKSPACE_SCHEMA_VERSION {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::SchemaUnsupported,
            "unsupported Workspace schema",
        ));
    }
    if catalog.workspaces.is_empty() {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::InvalidCatalog,
            "catalog must contain at least one Workspace",
        ));
    }
    if !(MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS..=MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS)
        .contains(&catalog.managed_terminal_font_size_points)
    {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::InvalidCatalog,
            "managed Ghostty font size must be between 10 and 32 points",
        ));
    }

    let mut workspace_ids = BTreeSet::<WorkspaceId>::new();
    for workspace in &catalog.workspaces {
        if !workspace_ids.insert(workspace.id) {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::InvalidCatalog,
                "duplicate Workspace ID",
            ));
        }
        validate_label(
            &workspace.name,
            "workspace name",
            WorkspaceErrorCode::WorkspaceNameInvalid,
        )?;
        if workspace.tabs.is_empty() {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::InvalidCatalog,
                "Workspace must contain at least one tab",
            ));
        }

        let mut tab_ids = BTreeSet::<WorkspaceTabId>::new();
        let mut pane_ids = BTreeSet::<ManagedPaneId>::new();
        let mut session_refs = BTreeSet::new();
        for tab in &workspace.tabs {
            if !tab_ids.insert(tab.id) {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "duplicate tab ID",
                ));
            }
            validate_label(&tab.title, "tab title", WorkspaceErrorCode::TabTitleInvalid)?;
            if tab.panes.len() > MAX_PANES_PER_TAB {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::PaneCapacity,
                    "a tab can contain at most nine panes",
                ));
            }
            for pane in &tab.panes {
                if !pane_ids.insert(pane.id) {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorCode::InvalidCatalog,
                        "duplicate pane ID",
                    ));
                }
                if !session_refs.insert((
                    pane.execution_target_id.clone(),
                    pane.tmux_session_id.clone(),
                )) {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorCode::SessionDuplicate,
                        "a target/session pair may appear only once in a Workspace",
                    ));
                }
            }

            let canonical_pane_ids = tab
                .panes
                .iter()
                .map(|pane| pane.id)
                .collect::<BTreeSet<_>>();
            let visual_pane_ids = tab.visual_pane_ids.iter().copied().collect::<BTreeSet<_>>();
            if tab.visual_pane_ids.len() != tab.panes.len()
                || visual_pane_ids.len() != tab.visual_pane_ids.len()
                || visual_pane_ids != canonical_pane_ids
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::InvalidCatalog,
                    "visual Pane order must be an exact permutation of the Tab's Panes",
                ));
            }

            if tab.panes.is_empty() {
                if tab.display_mode != DisplayMode::Monitor || tab.focused_pane_id.is_some() {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorCode::FocusInvalid,
                        "an empty tab must be Monitor mode with no focused pane",
                    ));
                }
            } else if let Some(focused_pane_id) = tab.focused_pane_id {
                if !tab.panes.iter().any(|pane| pane.id == focused_pane_id) {
                    return Err(WorkspaceError::new(
                        WorkspaceErrorCode::FocusInvalid,
                        "focused pane is not in the tab",
                    ));
                }
            } else if tab.display_mode == DisplayMode::Focus {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::FocusInvalid,
                    "Focus mode requires a focused pane",
                ));
            }
        }

        if !tab_ids.contains(&workspace.selected_tab_id) {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::TabNotFound,
                "selected tab does not exist",
            ));
        }
        if let Some(selected_pane_id) = workspace.selected_pane_id {
            let selected_tab = workspace
                .tabs
                .iter()
                .find(|tab| tab.id == workspace.selected_tab_id)
                .expect("selected Tab was validated above");
            if !selected_tab
                .panes
                .iter()
                .any(|pane| pane.id == selected_pane_id)
            {
                return Err(WorkspaceError::new(
                    WorkspaceErrorCode::PaneNotFound,
                    "selected Pane is not in the selected Tab",
                ));
            }
        }
    }

    if !workspace_ids.contains(&catalog.selected_workspace_id) {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::WorkspaceNotFound,
            "selected Workspace does not exist",
        ));
    }
    Ok(())
}
