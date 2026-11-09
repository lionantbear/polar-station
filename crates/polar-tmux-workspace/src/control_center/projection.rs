use polar_tmux_protocol::{ExecutionTargetKind, SshRouteProfileV1};
use serde::{Deserialize, Serialize};

use crate::{
    project_desktop, DisplayMode, ExecutionTargetId, ManagedPaneId, PendingRuntimeImpact,
    ProjectionContext, RuntimeHealth, SessionKey, SessionReferenceIndex, SessionSection,
    StableTmuxSessionId, TransportStatus, WorkspaceCatalog, WorkspaceError, WorkspaceId,
    WorkspaceRevision, WorkspaceTabId, MAX_PANES_PER_TAB,
};

use super::{
    badge_for, default_attention, ControlActionProjection, ControlActionRole, PaneSessionPresence,
    SessionAssignmentActions, SessionAttentionMap, SessionDiscoveryStatus, SessionFact,
    SessionLibraryProjection, SessionRowProjection, SessionSectionProjection, SessionSectionState,
    SessionTargetFact,
};

pub const CONTROL_CENTER_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Default)]
pub struct WorkspaceControlContext {
    pub connection_status: ControlConnectionStatus,
    pub connection_route: Option<ControlConnectionRoute>,
    pub connection_reason: Option<String>,
    pub workspace_ssh_target_id: Option<ExecutionTargetId>,
    pub workspace_ssh_target_revision: u64,
    pub workspace_ssh_destination: Option<String>,
    pub workspace_ssh_route_profile: Option<SshRouteProfileV1>,
    pub sessions_visible: bool,
    pub selected_session: Option<SessionKey>,
    pub selected_pane: Option<ManagedPaneId>,
    pub sessions: Vec<SessionFact>,
    pub session_targets: Vec<SessionTargetFact>,
    pub local_session_discovery: SessionDiscoveryStatus,
    pub remote_session_discovery: SessionDiscoveryStatus,
    pub session_references: SessionReferenceIndex,
    pub session_attention: SessionAttentionMap,
    pub projection: ProjectionContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlConnectionRoute {
    Lan,
    Wan,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlConnectionStatus {
    #[default]
    Inactive,
    Connecting,
    Connected,
    Recovering,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceChoiceProjection {
    pub id: WorkspaceId,
    pub name: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHeaderProjection {
    pub workspace_choices: Vec<WorkspaceChoiceProjection>,
    pub selected_workspace_id: WorkspaceId,
    pub connection: ControlConnectionProjection,
    pub ssh_target_editor: WorkspaceSshTargetEditorProjection,
    pub rename_action: ControlActionProjection,
    pub add_tab: ControlActionProjection,
    pub refresh: ControlActionProjection,
    pub workspace_menu_detach_action: ControlActionProjection,
    pub lifecycle_action: Option<ControlActionProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSshTargetEditorProjection {
    pub title: String,
    pub target_id: ExecutionTargetId,
    pub target_revision: u64,
    pub workspace_name: String,
    pub destination: String,
    pub route_profile: Option<SshRouteProfileV1>,
    pub destination_label: String,
    pub destination_placeholder: String,
    pub cancel_label: String,
    pub save_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlConnectionProjection {
    pub label: String,
    pub route: Option<ControlConnectionRoute>,
    pub status: ControlConnectionStatus,
    pub accessibility_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPaneProjection {
    pub id: ManagedPaneId,
    pub execution_target_id: ExecutionTargetId,
    pub tmux_session_id: StableTmuxSessionId,
    pub target_label: String,
    pub target_kind: ExecutionTargetKind,
    pub session_name: String,
    pub transport_status: TransportStatus,
    pub session_presence: super::PaneSessionPresence,
    pub selected: bool,
    pub attention: polar_tmux_protocol::SessionAttentionProjectionV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlTabProjection {
    pub id: WorkspaceTabId,
    pub title: String,
    pub selected: bool,
    pub display_mode: DisplayMode,
    pub focused_pane_id: Option<ManagedPaneId>,
    pub visual_pane_ids: Vec<ManagedPaneId>,
    pub rename_action: ControlActionProjection,
    pub delete_action: ControlActionProjection,
    pub panes: Vec<ControlPaneProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMapProjection {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub desired_revision: WorkspaceRevision,
    pub selected_tab_id: WorkspaceTabId,
    pub tabs: Vec<ControlTabProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlBottomBarProjection {
    pub actions: Vec<ControlActionProjection>,
    pub more_actions: Vec<ControlActionProjection>,
    pub managed_terminal_font_size_points: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceControlProjection {
    pub schema_version: u16,
    pub control_revision: u64,
    pub header: ControlHeaderProjection,
    pub sessions: SessionLibraryProjection,
    pub workspace_map: WorkspaceMapProjection,
    pub bottom_bar: ControlBottomBarProjection,
    pub projected_actions: Vec<ControlActionProjection>,
    pub banner: Option<String>,
    pub attention_badge: polar_tmux_protocol::AttentionBadgeProjectionV1,
}

impl WorkspaceControlProjection {
    pub fn action(&self, id: &super::ControlActionId) -> Option<&ControlActionProjection> {
        self.projected_actions
            .iter()
            .find(|action| action.id == *id)
    }
}

fn pane_session_presence(
    context: &WorkspaceControlContext,
    identity: &SessionKey,
) -> PaneSessionPresence {
    if let Some(fact) = context
        .sessions
        .iter()
        .find(|fact| &fact.identity == identity)
    {
        return if fact.transport_status == TransportStatus::Online {
            PaneSessionPresence::Present
        } else {
            PaneSessionPresence::Unknown
        };
    }
    let discovery = if identity.execution_target_id == ExecutionTargetId::local() {
        context.local_session_discovery
    } else {
        context.remote_session_discovery
    };
    if discovery == SessionDiscoveryStatus::Ready
        && (identity.execution_target_id == ExecutionTargetId::local()
            || context
                .session_targets
                .iter()
                .any(|target| target.id == identity.execution_target_id))
    {
        PaneSessionPresence::Ended
    } else {
        PaneSessionPresence::Unknown
    }
}

pub fn project_control_center(
    catalog: &WorkspaceCatalog,
    context: &WorkspaceControlContext,
) -> Result<WorkspaceControlProjection, WorkspaceError> {
    let desktop = project_desktop(catalog, &context.projection)?;
    let selected_workspace = desktop
        .workspaces
        .iter()
        .find(|workspace| workspace.id == desktop.selected_workspace_id)
        .expect("validated desktop projection contains selected Workspace");
    let lifecycle_action = lifecycle_action(
        selected_workspace.runtime_health,
        selected_workspace.pending_impact,
    );

    let sessions_toggle =
        ControlActionProjection::enabled("sessions.toggle", "Sessions", ControlActionRole::Toggle)
            .selected(context.sessions_visible);
    let selected_tab = selected_workspace
        .tabs
        .iter()
        .find(|tab| tab.id == selected_workspace.selected_tab_id)
        .expect("validated Workspace projection contains selected tab");
    let layout = ControlActionProjection::enabled(
        "presentation.layout.toggle",
        "Layout",
        ControlActionRole::Toggle,
    );
    let inbox = ControlActionProjection::enabled(
        "presentation.inbox.toggle",
        "Inbox",
        ControlActionRole::Toggle,
    );
    let resources = ControlActionProjection::enabled(
        "resources.open",
        "Resources",
        ControlActionRole::Secondary,
    );
    let more = ControlActionProjection::enabled("more.open", "More", ControlActionRole::Secondary);
    let bottom_actions = vec![sessions_toggle, layout, inbox, resources, more];
    let font_size = catalog.managed_terminal_font_size_points;
    let more_actions = vec![
        ControlActionProjection::enabled(
            "managed.terminal.font.size",
            format!("Ghostty Font Size… ({font_size}pt)"),
            ControlActionRole::Secondary,
        ),
        ControlActionProjection::enabled(
            "workspace.create",
            "New Workspace",
            ControlActionRole::Primary,
        ),
        ControlActionProjection::enabled(
            "iphone.open",
            "iPhone Companion…",
            ControlActionRole::Secondary,
        ),
    ];
    let detach_action = ControlActionProjection::close_window();
    let delete_workspace_action = ControlActionProjection::archive_workspace();
    let add_tab = ControlActionProjection::enabled(
        "workspace.tab.add",
        "Add Tab",
        ControlActionRole::Primary,
    );
    let refresh = ControlActionProjection::enabled(
        "workspace.restore",
        "Restore Saved Layout…",
        ControlActionRole::Recovery,
    )
    .with_confirmation(
        format!("restore-saved-layout-{}", selected_workspace.id),
        "Restore Saved Layout?",
        "Restore replaces tabs and panes that differ from the saved Layout. Save non-tmux work first. tmux Sessions keep running.",
        "Restore Layout",
    );
    let rename_workspace = ControlActionProjection::enabled(
        "workspace.rename",
        "Rename Workspace",
        ControlActionRole::Secondary,
    );
    let connection_label = match context.connection_route {
        Some(ControlConnectionRoute::Lan) => "LAN",
        Some(ControlConnectionRoute::Wan) => "WAN",
        None => "SSH",
    };
    let connection_accessibility_label = match (context.connection_status, context.connection_route)
    {
        (ControlConnectionStatus::Connected, Some(ControlConnectionRoute::Lan)) => {
            "Connected over the local network"
        }
        (ControlConnectionStatus::Connected, Some(ControlConnectionRoute::Wan)) => {
            "Connected over the wide-area network"
        }
        (ControlConnectionStatus::Inactive, _) => "SSH target is inactive",
        (ControlConnectionStatus::Connecting, _) => "SSH target is connecting",
        (ControlConnectionStatus::Connected, None) => "SSH target is connected",
        (ControlConnectionStatus::Recovering, _) => "SSH target connection is recovering",
        (ControlConnectionStatus::Offline, _) => "SSH target is offline",
    };
    let connection_accessibility_label = context
        .connection_reason
        .as_deref()
        .map(|reason| format!("{connection_accessibility_label}: {reason}"))
        .unwrap_or_else(|| connection_accessibility_label.to_owned());
    let connection = ControlConnectionProjection {
        label: connection_label.to_owned(),
        route: context.connection_route,
        status: context.connection_status,
        accessibility_label: connection_accessibility_label,
    };
    let target_id = context.workspace_ssh_target_id.clone().unwrap_or_else(|| {
        ExecutionTargetId::new(format!("workspace-{}", selected_workspace.id))
            .expect("deterministic SSH target ID")
    });
    let ssh_target_editor = WorkspaceSshTargetEditorProjection {
        title: "SSH Target".to_owned(),
        target_id,
        target_revision: context.workspace_ssh_target_revision,
        workspace_name: selected_workspace.name.clone(),
        destination: context
            .workspace_ssh_destination
            .clone()
            .unwrap_or_default(),
        route_profile: context.workspace_ssh_route_profile.clone(),
        destination_label: "Destination".to_owned(),
        destination_placeholder: "user@host".to_owned(),
        cancel_label: "Cancel".to_owned(),
        save_label: "Save".to_owned(),
    };

    let workspace_map = WorkspaceMapProjection {
        workspace_id: selected_workspace.id,
        workspace_name: selected_workspace.name.clone(),
        desired_revision: selected_workspace.desired_revision,
        selected_tab_id: selected_workspace.selected_tab_id,
        tabs: selected_workspace
            .tabs
            .iter()
            .map(|tab| ControlTabProjection {
                id: tab.id,
                title: tab.title.clone(),
                selected: tab.id == selected_workspace.selected_tab_id,
                display_mode: tab.display_mode,
                focused_pane_id: tab.focused_pane_id,
                visual_pane_ids: tab.visual_pane_order.clone(),
                rename_action: ControlActionProjection::enabled(
                    "workspace.tab.rename",
                    "Rename Tab",
                    ControlActionRole::Secondary,
                ),
                delete_action: ControlActionProjection::enabled(
                    "workspace.tab.delete",
                    "Delete Tab",
                    ControlActionRole::Destructive,
                )
                .with_confirmation(
                    format!("delete-tab-{}", tab.id),
                    "Delete Tab?",
                    "This removes the tab and its pane assignments. It does not kill tmux sessions.",
                    "Delete Tab",
                ),
                panes: tab
                    .panes
                    .iter()
                    .map(|pane| {
                        let identity = SessionKey::new(
                            pane.execution_target_id.clone(),
                            pane.tmux_session_id.clone(),
                        );
                        ControlPaneProjection {
                            id: pane.id,
                            execution_target_id: pane.execution_target_id.clone(),
                            tmux_session_id: pane.tmux_session_id.clone(),
                            target_label: pane.target_label.clone(),
                            target_kind: pane.target_kind,
                            session_name: pane.session_name.clone(),
                            transport_status: pane.transport_status,
                            session_presence: pane_session_presence(context, &identity),
                            selected: context.selected_pane == Some(pane.id),
                            attention: context
                                .session_attention
                                .get(&identity)
                                .cloned()
                                .unwrap_or_else(|| default_attention(&identity)),
                        }
                    })
                    .collect(),
            })
            .collect(),
    };

    let mut sessions = project_session_library(&SessionLibraryContext {
        sessions_visible: context.sessions_visible,
        selected_session: context.selected_session.clone(),
        sessions: context.sessions.clone(),
        session_targets: context.session_targets.clone(),
        local_session_discovery: context.local_session_discovery,
        remote_session_discovery: context.remote_session_discovery,
    });
    // This pure desktop-domain view is not a Host projection. Assignment and
    // saved-layout reference checks use the caller's own native composition.
    for row in &mut sessions.rows {
        let tab = catalog.selected_workspace().selected_tab();
        row.assigned_to_selected_tab = tab.panes.iter().any(|pane| {
            pane.execution_target_id == row.identity.execution_target_id
                && pane.tmux_session_id == row.identity.tmux_session_id
        });
        let actions = row.assignment_actions.as_ref().expect("assignment copy");
        row.assignment_action = if row.assigned_to_selected_tab {
            actions.remove.clone()
        } else if tab.panes.len() >= MAX_PANES_PER_TAB {
            actions.tab_full.clone()
        } else {
            actions.add.clone()
        };

    }
    let mut projected_actions = vec![rename_workspace.clone(), add_tab.clone(), refresh.clone()];
    if let Some(action) = &lifecycle_action {
        projected_actions.push(action.clone());
    }
    projected_actions.push(sessions.refresh_action.clone());
    projected_actions.extend(
        sessions
            .rows
            .iter()
            .filter_map(|row| row.restart_action.clone())
            .chain(
                sessions
                    .rows
                    .iter()
                    .filter_map(|row| row.replacement_action.clone()),
            ),
    );
    projected_actions.extend(bottom_actions.iter().cloned());
    projected_actions.extend(more_actions.iter().cloned());
    projected_actions.push(detach_action.clone());
    projected_actions.push(delete_workspace_action);
    projected_actions.push(ControlActionProjection::new_workspace_window());
    if selected_tab.display_mode == DisplayMode::Focus && !selected_tab.panes.is_empty() {
        projected_actions.push(ControlActionProjection::enabled(
            "workspace.focus.previous",
            "Focus Previous",
            ControlActionRole::Secondary,
        ));
        projected_actions.push(ControlActionProjection::enabled(
            "workspace.focus.next",
            "Focus Next",
            ControlActionRole::Secondary,
        ));
        for slot in 1..selected_tab.panes.len() {
            projected_actions.push(ControlActionProjection::enabled(
                format!("workspace.focus.sidebar.{slot}"),
                format!("Focus Sidebar {slot}"),
                ControlActionRole::Secondary,
            ));
        }
    }

    let attention_badge = badge_for(
        workspace_map
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|pane| &pane.attention)),
    );

    Ok(WorkspaceControlProjection {
        schema_version: CONTROL_CENTER_SCHEMA_VERSION,
        control_revision: catalog.revision.get(),
        header: ControlHeaderProjection {
            workspace_choices: desktop
                .workspaces
                .iter()
                .map(|workspace| WorkspaceChoiceProjection {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    selected: workspace.id == desktop.selected_workspace_id,
                })
                .collect(),
            selected_workspace_id: desktop.selected_workspace_id,
            connection,
            ssh_target_editor,
            rename_action: rename_workspace,
            add_tab,
            refresh,
            workspace_menu_detach_action: detach_action,
            lifecycle_action,
        },
        sessions,
        workspace_map,
        bottom_bar: ControlBottomBarProjection {
            actions: bottom_actions,
            more_actions,
            managed_terminal_font_size_points: font_size,
        },
        projected_actions,
        banner: context
            .projection
            .runtime_errors
            .get(&desktop.selected_workspace_id)
            .cloned(),
        attention_badge,
    })
}

/// Session facts and action copy without native layout or reference mirrors.
#[derive(Debug, Clone, Default)]
pub struct SessionLibraryContext {
    pub sessions_visible: bool,
    pub selected_session: Option<SessionKey>,
    pub sessions: Vec<SessionFact>,
    pub session_targets: Vec<SessionTargetFact>,
    pub local_session_discovery: SessionDiscoveryStatus,
    pub remote_session_discovery: SessionDiscoveryStatus,
}

pub fn project_session_library(context: &SessionLibraryContext) -> SessionLibraryProjection {
    let remote_targets = context
        .session_targets
        .iter()
        .filter(|target| target.section == SessionSection::Remote)
        .cloned()
        .collect::<Vec<_>>();
    let remote_execution_target_id = if remote_targets.len() == 1 {
        Some(remote_targets[0].id.clone())
    } else {
        None
    };
    let local_row_count = context
        .sessions
        .iter()
        .filter(|session| session.section == SessionSection::Local)
        .count();
    let remote_row_count = context
        .sessions
        .iter()
        .filter(|session| session.section == SessionSection::Remote)
        .count();
    let sections = vec![
        project_session_section(
            SessionSection::Local,
            context.local_session_discovery,
            true,
            local_row_count,
        ),
        project_session_section(
            SessionSection::Remote,
            context.remote_session_discovery,
            !remote_targets.is_empty(),
            remote_row_count,
        ),
    ];
    let mut sessions = SessionLibraryProjection {
        visible: context.sessions_visible,
        title: "Sessions".to_owned(),
        interaction_hint: "Add / remove from current Tab".to_owned(),
        refresh_action: ControlActionProjection::enabled(
            "sessions.refresh",
            "Refresh sessions",
            ControlActionRole::Secondary,
        ),
        close_action: ControlActionProjection::enabled(
            "sessions.toggle",
            "Close sessions",
            ControlActionRole::Toggle,
        )
        .selected(context.sessions_visible),
        sections,
        remote_targets: remote_targets.clone(),
        selected: context.selected_session.clone(),
        rows: context
            .sessions
            .iter()
            .map(|session| {
                let assigned_to_selected_tab = false;
                let assignment_action = ControlActionProjection::enabled(
                    "session.assignment.toggle",
                    "Add to Tab",
                    ControlActionRole::Primary,
                );
                let assignment_actions = SessionAssignmentActions {
                    add: ControlActionProjection::enabled(
                        "session.assignment.toggle",
                        "Add to Tab",
                        ControlActionRole::Primary,
                    ),
                    remove: ControlActionProjection::enabled(
                        "session.assignment.toggle",
                        "Remove from Tab",
                        ControlActionRole::Secondary,
                    )
                    .selected(true),
                    tab_full: ControlActionProjection::disabled(
                        "session.assignment.toggle",
                        "Add to Tab",
                        ControlActionRole::Secondary,
                        "The selected tab already contains nine panes.",
                    ),
                    assigned_elsewhere: ControlActionProjection::enabled(
                        "session.assignment.toggle",
                        "Show in Layout",
                        ControlActionRole::Secondary,
                    ),
                    no_managed_tab: ControlActionProjection::disabled(
                        "session.assignment.toggle",
                        "Add to Tab",
                        ControlActionRole::Secondary,
                        "No managed Tab is selected.",
                    ),
                };
                let mut rename_action = ControlActionProjection::enabled(
                    "session.rename",
                    "Rename Session",
                    ControlActionRole::Secondary,
                );
                let mut kill_action = ControlActionProjection::enabled(
                    "session.kill", "Kill Session", ControlActionRole::Destructive,
                ).with_confirmation(
                    format!("kill-session-{}-{}", session.identity.execution_target_id, session.identity.tmux_session_id),
                    "Kill Session?",
                    format!("End “{}” on {}? All {} connected terminals will disconnect. Tab and Pane layouts will be kept.",
                        session.display_name, session.target_label, session.attached_clients),
                    "Kill Session",
                );
                let usable = session.transport_status == TransportStatus::Online
                    && session.recovery_status.is_none_or(|status| status == crate::SessionRecoveryStatus::Ready);
                let lost = session.replacement.is_some();
                let mut assignment_action = assignment_action;
                let mut assignment_actions = assignment_actions;
                if !usable {
                    let reason = if lost { "The saved Session was lost. Choose the existing Session explicitly." } else { "Session identity is not ready. Refresh Sessions before retrying." };
                    for action in [&mut assignment_action, &mut assignment_actions.add, &mut rename_action, &mut kill_action] {
                        action.enabled = false;
                        action.disabled_reason = Some(reason.into());
                    }
                }
                if session.recovery_status == Some(crate::SessionRecoveryStatus::Ending)
                    && session.transport_status == TransportStatus::Online {
                    kill_action.enabled = true;
                    kill_action.disabled_reason = None;
                }
                let replacement_action = session.replacement.as_ref().map(|replacement| {
                    ControlActionProjection::enabled(
                        format!("session.replace.{}", session.identity.tmux_session_id.as_str().replace('-', ".")),
                        format!("Use existing {}", session.display_name),
                        ControlActionRole::Secondary,
                    ).with_confirmation(
                        format!("replace-session-{}-{}-{}", session.identity.execution_target_id, session.identity.tmux_session_id, replacement.tmux_session_id),
                        "Use existing Session?",
                        format!("Use the existing “{}” in this Workspace and remove the lost saved record? Other Workspaces will not be changed.", session.display_name),
                        "Use existing Session",
                    )
                });
                SessionRowProjection {
                    identity: session.identity.clone(),
                    session_kind: super::SessionKind::from_terminal_name(&session.display_name).map_or(session.session_kind, |(kind, _)| kind),
                    original_agent_id: super::SessionKind::from_terminal_name(&session.display_name).map(|(_, id)| id.to_owned()).or_else(|| session.original_agent_id.clone()),
                    display_name: session.display_name.clone(),
                    detail_label: if session.recovery_status == Some(crate::SessionRecoveryStatus::Ending) {
                        Some("End requested · Refresh or retry ending the original Session".into())
                    } else if lost {
                        Some(format!("Session lost · A different session named {} exists.", session.display_name))
                    } else if session.recovery_status == Some(crate::SessionRecoveryStatus::Conflict) {
                        Some("Session identity conflict".into())
                    } else {
                        let provider = match super::SessionKind::from_terminal_name(&session.display_name).map_or(session.session_kind, |(kind, _)| kind) {
                            crate::SessionKind::CodexPlane => Some("Codex"),
                            crate::SessionKind::ClaudePlane => Some("Claude"),
                            crate::SessionKind::Terminal => None,
                        };
                        match (provider, session.attached_clients) {
                            (Some(kind), 0) => Some(kind.into()),
                            (Some(kind), count) => Some(format!("{kind} · {count} attached")),
                            (None, 0) => None,
                            (None, count) => Some(format!("{count} attached")),
                        }
                    },
                    target_label: session.target_label.clone(),
                    section: session.section,
                    attached_clients: session.attached_clients,
                    transport_status: session.transport_status,
                    selected: context.selected_session.as_ref() == Some(&session.identity),
                    assigned_to_selected_tab,
                    assignment_action,
                    assignment_actions: Some(assignment_actions),
                    rename_action,
                    kill_action,
                    replacement: session.replacement.clone(),
                    replacement_action,
                    restart_action: (session.recovery_status == Some(crate::SessionRecoveryStatus::Manual)
                        && session.transport_status == TransportStatus::Offline
                        && session
                            .identity
                            .tmux_session_id
                            .as_str()
                            .starts_with("station-session-"))
                    .then(|| {
                        ControlActionProjection::enabled(
                            format!(
                                "session.restart.{}",
                                session.identity.tmux_session_id.as_str().replace('-', ".")
                            ),
                            "Run Saved Command",
                            ControlActionRole::Secondary,
                        )
                    }),
                }
            })
            .collect(),
        local_execution_target_id: crate::ExecutionTargetId::local(),
        remote_execution_target_id: remote_execution_target_id.clone(),
        local_new_shell: ControlActionProjection::enabled(
            "session.create.local.shell",
            "New Shell",
            ControlActionRole::Primary,
        ),
        local_new_command: ControlActionProjection::enabled(
            "session.create.local.command",
            "New Command",
            ControlActionRole::Secondary,
        ),
        remote_new_session: if !remote_targets.is_empty() {
            ControlActionProjection::enabled(
                "session.create.remote",
                "New Session",
                ControlActionRole::Primary,
            )
        } else {
            ControlActionProjection::disabled(
                "session.create.remote",
                "New Session",
                ControlActionRole::Primary,
                "No managed remote execution target is currently projected.",
            )
        },
    };
    sessions.rows.sort_by(|left, right| {
        left.section
            .cmp(&right.section)
            .then_with(|| left.target_label.cmp(&right.target_label))
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.identity.cmp(&right.identity))
    });

    sessions
}

fn project_session_section(
    section: SessionSection,
    discovery: SessionDiscoveryStatus,
    target_available: bool,
    row_count: usize,
) -> SessionSectionProjection {
    let (title, state, message) = match (section, discovery, target_available, row_count) {
        (SessionSection::Local, SessionDiscoveryStatus::Loading, _, _) => (
            "Local",
            SessionSectionState::Loading,
            Some("Loading local tmux sessions"),
        ),
        (SessionSection::Remote, SessionDiscoveryStatus::Loading, true, _) => (
            "Remote",
            SessionSectionState::Loading,
            Some("Loading remote tmux sessions"),
        ),
        (SessionSection::Local, SessionDiscoveryStatus::Failed, _, _) => (
            "Local",
            SessionSectionState::Error,
            Some("Local tmux sessions could not be loaded"),
        ),
        (SessionSection::Remote, SessionDiscoveryStatus::Failed, true, _) => (
            "Remote",
            SessionSectionState::Error,
            Some("Remote tmux sessions could not be loaded"),
        ),
        (SessionSection::Remote, _, false, _) => (
            "Remote",
            SessionSectionState::TargetMissing,
            Some("Set an SSH target to discover remote sessions"),
        ),
        (SessionSection::Local, _, _, 0) => (
            "Local",
            SessionSectionState::Empty,
            Some("No sessions on the default local tmux server"),
        ),
        (SessionSection::Remote, _, _, 0) => (
            "Remote",
            SessionSectionState::Empty,
            Some("No remote tmux sessions found"),
        ),
        (SessionSection::Local, _, _, _) => ("Local", SessionSectionState::Populated, None),
        (SessionSection::Remote, _, _, _) => ("Remote", SessionSectionState::Populated, None),
    };
    SessionSectionProjection {
        section,
        title: title.to_owned(),
        state,
        message: message.map(str::to_owned),
        row_count,
    }
}

fn lifecycle_action(
    health: RuntimeHealth,
    impact: PendingRuntimeImpact,
) -> Option<ControlActionProjection> {
    match (health, impact) {
        (RuntimeHealth::Closed, _) => Some(ControlActionProjection::enabled(
            "workspace.open",
            "Open Workspace",
            ControlActionRole::Primary,
        )),
        (RuntimeHealth::Healthy, _) => None,
        (RuntimeHealth::NeedsRepair, _) => Some(ControlActionProjection::enabled(
            "workspace.repair",
            "Repair Workspace",
            ControlActionRole::Primary,
        )),
        (RuntimeHealth::Busy, _) => Some(ControlActionProjection::disabled(
            "workspace.busy",
            "Workspace Busy",
            ControlActionRole::Primary,
            "A Workspace operation is already running.",
        )),
        (RuntimeHealth::Unavailable, _) => Some(ControlActionProjection::disabled(
            "workspace.unavailable",
            "Workspace Unavailable",
            ControlActionRole::Primary,
            "The Workspace presentation frontend is unavailable.",
        )),
    }
}
