use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{
    ExecutionTargetId, ManagedPaneId, StableTmuxSessionId, WorkspaceId, WorkspaceTabId,
};
use crate::model::WorkspaceCatalog;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionKey {
    pub execution_target_id: ExecutionTargetId,
    pub tmux_session_id: StableTmuxSessionId,
}

impl SessionKey {
    pub const fn new(
        execution_target_id: ExecutionTargetId,
        tmux_session_id: StableTmuxSessionId,
    ) -> Self {
        Self {
            execution_target_id,
            tmux_session_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InFlightPresentation {
    pub workspace_id: WorkspaceId,
    pub panes: Vec<InFlightPaneReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InFlightPaneReference {
    pub tab_id: WorkspaceTabId,
    pub pane_id: ManagedPaneId,
    pub execution_target_id: ExecutionTargetId,
    pub tmux_session_id: StableTmuxSessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Desired,
    InFlight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReferenceBlocker {
    pub kind: ReferenceKind,
    pub workspace_id: WorkspaceId,
    pub tab_id: WorkspaceTabId,
    pub pane_id: ManagedPaneId,
}

#[derive(Debug, Clone, Default)]
pub struct SessionReferenceIndex {
    blockers: BTreeMap<SessionKey, Vec<SessionReferenceBlocker>>,
}

impl SessionReferenceIndex {
    pub fn build_all(
        catalog: &WorkspaceCatalog,
        in_flight_presentations: &[InFlightPresentation],
    ) -> Self {
        let mut index = Self::from_desired(catalog);
        for in_flight in in_flight_presentations {
            index.insert_in_flight(in_flight);
        }
        index
    }

    fn from_desired(catalog: &WorkspaceCatalog) -> Self {
        let mut index = Self::default();
        for workspace in &catalog.workspaces {
            for tab in &workspace.tabs {
                for pane in &tab.panes {
                    index.insert(
                        SessionKey::new(
                            pane.execution_target_id.clone(),
                            pane.tmux_session_id.clone(),
                        ),
                        SessionReferenceBlocker {
                            kind: ReferenceKind::Desired,
                            workspace_id: workspace.id,
                            tab_id: tab.id,
                            pane_id: pane.id,
                        },
                    );
                }
            }
        }
        index
    }

    pub fn blockers(
        &self,
        execution_target_id: &ExecutionTargetId,
        tmux_session_id: &StableTmuxSessionId,
    ) -> Vec<SessionReferenceBlocker> {
        self.blockers
            .get(&SessionKey::new(
                execution_target_id.clone(),
                tmux_session_id.clone(),
            ))
            .cloned()
            .unwrap_or_default()
    }

    fn insert(&mut self, key: SessionKey, blocker: SessionReferenceBlocker) {
        self.blockers.entry(key).or_default().push(blocker);
    }

    fn insert_in_flight(&mut self, in_flight: &InFlightPresentation) {
        for pane in &in_flight.panes {
            self.insert(
                SessionKey::new(
                    pane.execution_target_id.clone(),
                    pane.tmux_session_id.clone(),
                ),
                SessionReferenceBlocker {
                    kind: ReferenceKind::InFlight,
                    workspace_id: in_flight.workspace_id,
                    tab_id: pane.tab_id,
                    pane_id: pane.pane_id,
                },
            );
        }
    }
}
