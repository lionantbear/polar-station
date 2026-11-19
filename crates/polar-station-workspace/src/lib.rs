//! Platform-neutral Workspace behavior for Polar Tmux.
//!
//! This crate owns no tmux, SSH, Ghostty, filesystem, process, socket, or
//! native-UI execution.

pub mod dictate;
pub mod shortcuts;
mod command;
mod control_center;
mod error;
mod ids;
mod layout;
mod model;
mod projection;
mod reference_index;
mod runtime;
mod schema;
mod validation;

pub use command::{
    CommandEnvelope, PendingRuntimeImpact, WorkspaceCommand, WorkspaceEngine, WorkspaceTransition,
};
pub use control_center::{
    ToolSetupCopy, tool_setup_copy,
    decide_workspace_lifecycle, WorkspaceLifecycleInput, WorkspaceLifecycleDecision,
    station_shortcut, StationShortcut, StationShortcutAction, STATION_SHORTCUTS,
    badge_for, default_attention, project_control_center, project_session_library,
    resolve_control_intent, ControlActionId, ControlActionProjection, ControlActionRole,
    ControlAuxiliarySurface, ControlBottomBarProjection, ControlCenterError,
    ControlCenterErrorCode, ControlConfirmation, ControlConnectionProjection,
    ControlConnectionRoute, ControlConnectionStatus, ControlHeaderProjection, ControlIntent,
    ControlPaneProjection, ControlTabProjection, PaneSessionPresence, ResolvedControlIntent,
    SessionAttentionMap, SessionDiscoveryStatus, SessionFact, SessionKind, SessionLibraryContext,
    SessionLibraryProjection, SessionRowProjection, SessionSection, SessionSectionProjection,
    SessionSectionState, SessionTargetFact, WorkspaceChoiceProjection, WorkspaceControlContext,
    WorkspaceControlProjection, WorkspaceMapProjection, WorkspaceSshTargetEditorProjection,
    CONTROL_CENTER_SCHEMA_VERSION,
};
pub use error::{WorkspaceError, WorkspaceErrorCode};
pub use ids::{
    CatalogRevision, ExecutionTargetId, IdentifierError, ManagedPaneId, ManagedSessionId,
    OperationId, StableTmuxSessionId, WorkspaceId, WorkspaceRevision, WorkspaceTabId,
};
pub use layout::{
    canonical_layout, CanonicalLayout, LayoutFingerprint, LayoutRole, LayoutSlot, Ratio,
};
pub use model::{
    DisplayMode, ManagedPane, Workspace, WorkspaceCatalog, WorkspaceTab,
    DEFAULT_MANAGED_TERMINAL_FONT_SIZE_POINTS, MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS,
    MAX_PANES_PER_TAB, MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS, WORKSPACE_SCHEMA_VERSION,
};
pub use projection::{
    project_companion, project_desktop, CompanionCatalogProjection, CompanionPaneProjection,
    CompanionTabProjection, CompanionWorkspaceProjection, DesktopCatalogProjection,
    DesktopPaneProjection, DesktopTabProjection, DesktopWorkspaceProjection, ProjectionContext,
    RuntimeHealth, SessionDisplayFacts, TransportStatus,
};
pub use reference_index::{
    InFlightPaneReference, InFlightPresentation, ReferenceKind, SessionKey,
    SessionReferenceBlocker, SessionReferenceIndex,
};
pub use runtime::{
    decide_recovery, ManagedSessionRecord, ProcessRestoreSpec, SessionLaunchSpec,
    SessionOperationReceipt, SessionRecoveryDecision, SessionRecoveryStatus, SessionRuntimeState,
    SESSION_RUNTIME_SCHEMA_VERSION,
};
pub use schema::{decode_catalog_json, encode_catalog_json};
pub use validation::validate_catalog;

pub use control_center::{ConnectionsEngine, ConnectionsInput, ConnectionsView, ConnectionsEffect, ConnectionsSelection, ConnectionsPage};
pub use control_center::{project_diagnostics, DiagnosticAction, DiagnosticHostConnection, DiagnosticRow, DiagnosticState, DiagnosticsFacts, DiagnosticsProjection, RemoteDiagnostics};

pub use control_center::{SessionEditorEngine, SessionEditorDraft, SessionEditorInput, SessionEditorEffect, SessionEditorView};

pub use control_center::{project_agent_catalog, AgentCatalogInput, AgentCatalogRow};

pub use control_center::{AttentionEngine, AttentionInput, AttentionEffects, AttentionObservation, AttentionNotification};

pub use control_center::recovery::{recovery_view, RecoveryPresentation};

pub use control_center::diagnostics_engine::{DiagnosticsEngine, DiagnosticsInput, DiagnosticsOutput, DiagnosticsEffect};

pub use control_center::{SettingsNavigation, SettingsNavigationInput, SettingsNavigationView, SettingsRoute};

pub use control_center::{SettingsTextActions,SettingsTextFieldView,PromptField,SettingsTextEngine,SettingsTextInput,SettingsTextView,SettingsTextEffect};

pub use control_center::{SettingsEditInput,SettingsEditLease,SettingsEditSection,SettingsEditView};

pub use control_center::workspace_manager;
pub mod portable_workspace;

pub use control_center::backend_selection;
