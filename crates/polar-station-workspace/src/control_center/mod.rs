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
mod diagnostics;
pub use diagnostics::*;
pub use connections::{ConnectionsEngine, ConnectionsInput, ConnectionsView, ConnectionsEffect, Selection as ConnectionsSelection, Page as ConnectionsPage};

mod session_editor;
pub use session_editor::*;

pub mod recovery;

mod tool_preparation;
pub use tool_preparation::{ToolSetupCopy,tool_setup_copy};

pub mod diagnostics_engine;

mod settings_navigation;
pub mod workspace_manager;
pub use settings_navigation::{SettingsNavigation, SettingsNavigationInput, SettingsNavigationView, SettingsRoute};

mod settings_text;
pub use settings_text::{SettingsTextActions,SettingsTextFieldView,PromptField,SettingsTextEngine,SettingsTextInput,SettingsTextView,SettingsTextEffect};

pub use polar_station_settings::{SettingsEditInput,SettingsEditLease,SettingsEditSection,SettingsEditView};

pub mod backend_selection;
