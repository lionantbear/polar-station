mod action;
mod native_action;
mod attention;
mod intent;
mod projection;
mod session_library;

pub use action::{
    decide_workspace_lifecycle, WorkspaceLifecycleInput, WorkspaceLifecycleDecision,
    ControlActionId, ControlActionProjection, ControlActionRole, ControlCenterError,
    ControlCenterErrorCode, ControlConfirmation,
};
pub use attention::{AttentionEngine, AttentionInput, AttentionEffects, AttentionObservation, AttentionNotification, badge_for, default_attention, SessionAttentionMap};
pub use intent::{
    resolve_control_intent, ControlAuxiliarySurface, ControlIntent, ResolvedControlIntent,
};
pub use projection::{
    project_control_center, project_session_library, ControlBottomBarProjection,
    ControlConnectionProjection, ControlConnectionRoute, ControlConnectionStatus,
    ControlHeaderProjection, ControlPaneProjection, ControlTabProjection, SessionLibraryContext,
    WorkspaceChoiceProjection, WorkspaceControlContext, WorkspaceControlProjection,
    WorkspaceMapProjection, WorkspaceSshTargetEditorProjection, CONTROL_CENTER_SCHEMA_VERSION,
};
pub use session_library::{
    project_agent_catalog, AgentCatalogInput, AgentCatalogRow,
    PaneSessionPresence, SessionAssignmentActions, SessionDiscoveryStatus, SessionFact, SessionKind,
    SessionLibraryProjection, SessionRowProjection, SessionSection, SessionSectionProjection,
    SessionSectionState, SessionTargetFact,
};

mod shortcuts;
pub use shortcuts::{station_shortcut, StationShortcut, StationShortcutAction, STATION_SHORTCUTS};

mod connections;
pub use connections::{ConnectionsEngine, ConnectionsInput, ConnectionsView, ConnectionsEffect, Selection as ConnectionsSelection, Page as ConnectionsPage};

mod session_editor;
pub use session_editor::*;
