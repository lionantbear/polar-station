use std::collections::BTreeMap;

use polar_tmux_protocol::ExecutionTargetKind;
use serde::{Deserialize, Serialize};

use crate::command::PendingRuntimeImpact;
use crate::error::WorkspaceError;
use crate::ids::{
    ExecutionTargetId, ManagedPaneId, OperationId, StableTmuxSessionId, WorkspaceId,
    WorkspaceRevision, WorkspaceTabId,
};
use crate::layout::{canonical_layout, LayoutRole, Ratio};
use crate::model::{DisplayMode, WorkspaceCatalog};
use crate::reference_index::SessionKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportStatus {
    Online,
    Offline,
    Recovering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDisplayFacts {
    pub target_label: String,
    pub target_kind: ExecutionTargetKind,
    pub session_name: String,
    pub transport_status: TransportStatus,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectionContext {
    pub sessions: BTreeMap<SessionKey, SessionDisplayFacts>,
    pub pending_operation: Option<OperationId>,
    pub applied_revisions: BTreeMap<WorkspaceId, WorkspaceRevision>,
    pub runtime_health: BTreeMap<WorkspaceId, RuntimeHealth>,
    pub runtime_errors: BTreeMap<WorkspaceId, String>,
    pub pending_impacts: BTreeMap<WorkspaceId, PendingRuntimeImpact>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealth {
    #[default]
    Closed,
    Healthy,
    NeedsRepair,
    Busy,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopCatalogProjection {
    pub selected_workspace_id: WorkspaceId,
    pub workspaces: Vec<DesktopWorkspaceProjection>,
    pub pending_operation: Option<OperationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopWorkspaceProjection {
    pub id: WorkspaceId,
    pub name: String,
    pub desired_revision: WorkspaceRevision,
    pub applied_revision: Option<WorkspaceRevision>,
    pub runtime_health: RuntimeHealth,
    pub pending_impact: PendingRuntimeImpact,
    pub selected_tab_id: WorkspaceTabId,
    pub tabs: Vec<DesktopTabProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopTabProjection {
    pub id: WorkspaceTabId,
    pub title: String,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<ManagedPaneId>,
    pub pane_order: Vec<ManagedPaneId>,
    pub visual_pane_order: Vec<ManagedPaneId>,
    pub panes: Vec<DesktopPaneProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopPaneProjection {
    pub id: ManagedPaneId,
    pub execution_target_id: ExecutionTargetId,
    pub tmux_session_id: StableTmuxSessionId,
    pub target_label: String,
    pub target_kind: ExecutionTargetKind,
    pub session_name: String,
    pub transport_status: TransportStatus,
    pub column: usize,
    pub row: usize,
    pub width: Ratio,
    pub height: Ratio,
    pub role: LayoutRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionCatalogProjection {
    pub selected_workspace_id: WorkspaceId,
    pub workspaces: Vec<CompanionWorkspaceProjection>,
    pub pending_operation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionWorkspaceProjection {
    pub id: WorkspaceId,
    pub name: String,
    pub desired_revision: WorkspaceRevision,
    pub applied_revision: Option<WorkspaceRevision>,
    pub runtime_health: RuntimeHealth,
    pub pending_impact: PendingRuntimeImpact,
    pub selected_tab_id: WorkspaceTabId,
    pub tabs: Vec<CompanionTabProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionTabProjection {
    pub id: WorkspaceTabId,
    pub title: String,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<ManagedPaneId>,
    pub pane_order: Vec<ManagedPaneId>,
    pub panes: Vec<CompanionPaneProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionPaneProjection {
    pub id: ManagedPaneId,
    pub target_label: String,
    pub target_kind: ExecutionTargetKind,
    pub session_name: String,
    pub transport_status: TransportStatus,
    pub column: usize,
    pub row: usize,
    pub width: Ratio,
    pub height: Ratio,
    pub role: LayoutRole,
}

pub fn project_desktop(
    catalog: &WorkspaceCatalog,
    context: &ProjectionContext,
) -> Result<DesktopCatalogProjection, WorkspaceError> {
    let workspaces = catalog
        .workspaces
        .iter()
        .map(|workspace| {
            let tabs = workspace
                .tabs
                .iter()
                .map(|tab| {
                    let layout = canonical_layout(tab)?;
                    let panes = tab
                        .panes
                        .iter()
                        .map(|pane| {
                            let facts = session_facts(context, pane);
                            let slot = layout
                                .slots
                                .iter()
                                .find(|slot| slot.pane_id == pane.id)
                                .expect("layout contains every pane");
                            DesktopPaneProjection {
                                id: pane.id,
                                execution_target_id: pane.execution_target_id.clone(),
                                tmux_session_id: pane.tmux_session_id.clone(),
                                target_label: facts.target_label,
                                target_kind: facts.target_kind,
                                session_name: facts.session_name,
                                transport_status: facts.transport_status,
                                column: slot.column,
                                row: slot.row,
                                width: slot.width,
                                height: slot.height,
                                role: slot.role,
                            }
                        })
                        .collect();
                    Ok(DesktopTabProjection {
                        id: tab.id,
                        title: tab.title.clone(),
                        display_mode: tab.display_mode,
                        focused_pane_id: tab.focused_pane_id,
                        pane_order: layout.canonical_pane_order,
                        visual_pane_order: tab.visual_pane_ids.clone(),
                        panes,
                    })
                })
                .collect::<Result<Vec<_>, WorkspaceError>>()?;
            Ok(DesktopWorkspaceProjection {
                id: workspace.id,
                name: workspace.name.clone(),
                desired_revision: workspace.revision,
                applied_revision: context.applied_revisions.get(&workspace.id).copied(),
                runtime_health: context
                    .runtime_health
                    .get(&workspace.id)
                    .copied()
                    .unwrap_or_default(),
                pending_impact: context
                    .pending_impacts
                    .get(&workspace.id)
                    .copied()
                    .unwrap_or(PendingRuntimeImpact::None),
                selected_tab_id: workspace.selected_tab_id,
                tabs,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    Ok(DesktopCatalogProjection {
        selected_workspace_id: catalog.selected_workspace_id,
        workspaces,
        pending_operation: context.pending_operation,
    })
}

pub fn project_companion(
    catalog: &WorkspaceCatalog,
    context: &ProjectionContext,
) -> Result<CompanionCatalogProjection, WorkspaceError> {
    let desktop = project_desktop(catalog, context)?;
    Ok(CompanionCatalogProjection {
        selected_workspace_id: desktop.selected_workspace_id,
        pending_operation: desktop.pending_operation.is_some(),
        workspaces: desktop
            .workspaces
            .into_iter()
            .map(|workspace| CompanionWorkspaceProjection {
                id: workspace.id,
                name: workspace.name,
                desired_revision: workspace.desired_revision,
                applied_revision: workspace.applied_revision,
                runtime_health: workspace.runtime_health,
                pending_impact: workspace.pending_impact,
                selected_tab_id: workspace.selected_tab_id,
                tabs: workspace
                    .tabs
                    .into_iter()
                    .map(|tab| CompanionTabProjection {
                        id: tab.id,
                        title: tab.title,
                        display_mode: tab.display_mode,
                        focused_pane_id: tab.focused_pane_id,
                        pane_order: tab.pane_order,
                        panes: tab
                            .panes
                            .into_iter()
                            .map(|pane| CompanionPaneProjection {
                                id: pane.id,
                                target_label: pane.target_label,
                                target_kind: pane.target_kind,
                                session_name: pane.session_name,
                                transport_status: pane.transport_status,
                                column: pane.column,
                                row: pane.row,
                                width: pane.width,
                                height: pane.height,
                                role: pane.role,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    })
}

fn session_facts(
    context: &ProjectionContext,
    pane: &crate::model::ManagedPane,
) -> SessionDisplayFacts {
    context
        .sessions
        .get(&SessionKey::new(
            pane.execution_target_id.clone(),
            pane.tmux_session_id.clone(),
        ))
        .cloned()
        .unwrap_or_else(|| SessionDisplayFacts {
            target_label: pane.execution_target_id.to_string(),
            target_kind: if pane.execution_target_id.as_str()
                == polar_tmux_protocol::LOCAL_EXECUTION_TARGET
            {
                ExecutionTargetKind::Local
            } else {
                ExecutionTargetKind::Ssh
            },
            session_name: pane.tmux_session_id.to_string(),
            transport_status: TransportStatus::Offline,
        })
}
