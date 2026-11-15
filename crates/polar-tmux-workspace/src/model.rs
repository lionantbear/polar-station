use serde::{Deserialize, Serialize};

use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::ids::{
    CatalogRevision, ExecutionTargetId, ManagedPaneId, StableTmuxSessionId, WorkspaceId,
    WorkspaceRevision, WorkspaceTabId,
};
use crate::validation::{validate_catalog, validate_label};

pub const WORKSPACE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PANES_PER_TAB: usize = 9;
pub const MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS: u8 = 10;
pub const MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS: u8 = 32;
pub const DEFAULT_MANAGED_TERMINAL_FONT_SIZE_POINTS: u8 = 14;

const fn default_managed_terminal_font_size_points() -> u8 {
    DEFAULT_MANAGED_TERMINAL_FONT_SIZE_POINTS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    Monitor,
    Focus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPane {
    pub id: ManagedPaneId,
    pub execution_target_id: ExecutionTargetId,
    pub tmux_session_id: StableTmuxSessionId,
}

impl ManagedPane {
    pub fn new(
        execution_target_id: ExecutionTargetId,
        tmux_session_id: StableTmuxSessionId,
    ) -> Self {
        Self::new_with_id(ManagedPaneId::new(), execution_target_id, tmux_session_id)
    }

    pub fn new_with_id(
        id: ManagedPaneId,
        execution_target_id: ExecutionTargetId,
        tmux_session_id: StableTmuxSessionId,
    ) -> Self {
        Self {
            id,
            execution_target_id,
            tmux_session_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    pub id: WorkspaceTabId,
    pub title: String,
    pub panes: Vec<ManagedPane>,
    #[serde(default)]
    pub visual_pane_ids: Vec<ManagedPaneId>,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<ManagedPaneId>,
}

impl WorkspaceTab {
    pub fn new(title: impl Into<String>) -> Result<Self, WorkspaceError> {
        let title = title.into();
        validate_label(&title, "tab title", WorkspaceErrorCode::TabTitleInvalid)?;
        Self::new_with_id(WorkspaceTabId::new(), title)
    }

    pub fn new_with_id(
        id: WorkspaceTabId,
        title: impl Into<String>,
    ) -> Result<Self, WorkspaceError> {
        let title = title.into();
        validate_label(&title, "tab title", WorkspaceErrorCode::TabTitleInvalid)?;
        Ok(Self {
            id,
            title,
            panes: Vec::new(),
            visual_pane_ids: Vec::new(),
            display_mode: DisplayMode::Monitor,
            focused_pane_id: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub revision: WorkspaceRevision,
    pub selected_tab_id: WorkspaceTabId,
    #[serde(default)]
    pub selected_pane_id: Option<ManagedPaneId>,
    pub tabs: Vec<WorkspaceTab>,
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Result<Self, WorkspaceError> {
        let name = name.into();
        validate_label(
            &name,
            "workspace name",
            WorkspaceErrorCode::WorkspaceNameInvalid,
        )?;
        let tab = WorkspaceTab::new("Main")?;
        Ok(Self {
            id: WorkspaceId::new(),
            name,
            revision: WorkspaceRevision::default(),
            selected_tab_id: tab.id,
            selected_pane_id: None,
            tabs: vec![tab],
        })
    }

    pub fn selected_tab(&self) -> &WorkspaceTab {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.selected_tab_id)
            .expect("validated Workspace always has its selected tab")
    }

    pub fn tab(&self, id: WorkspaceTabId) -> Option<&WorkspaceTab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCatalog {
    pub schema_version: u32,
    pub revision: CatalogRevision,
    pub selected_workspace_id: WorkspaceId,
    pub workspaces: Vec<Workspace>,
    #[serde(default = "default_managed_terminal_font_size_points")]
    pub managed_terminal_font_size_points: u8,
}

impl WorkspaceCatalog {
    pub fn new(name: impl Into<String>) -> Result<Self, WorkspaceError> {
        let workspace = Workspace::new(name)?;
        let catalog = Self {
            schema_version: WORKSPACE_SCHEMA_VERSION,
            revision: CatalogRevision::default(),
            selected_workspace_id: workspace.id,
            workspaces: vec![workspace],
            managed_terminal_font_size_points: DEFAULT_MANAGED_TERMINAL_FONT_SIZE_POINTS,
        };
        validate_catalog(&catalog)?;
        Ok(catalog)
    }

    pub fn selected_workspace(&self) -> &Workspace {
        self.workspace(self.selected_workspace_id)
            .expect("validated catalog always has its selected Workspace")
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }
}
