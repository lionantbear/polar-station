//! Display-safe Session wire adapters; no Workspace layout input.
use polar_tmux_protocol::*;
use polar_tmux_workspace::{
    ControlActionProjection, ControlActionRole, SessionKey, SessionLibraryProjection,
    SessionSection, TransportStatus,
};

pub(super) fn wire_session_library(
    projection: &SessionLibraryProjection,
) -> SessionLibraryProjectionV1 {
    SessionLibraryProjectionV1 {
        visible: projection.visible,
        title: projection.title.clone(),
        interaction_hint: projection.interaction_hint.clone(),
        refresh_action: wire_action(&projection.refresh_action),
        close_action: wire_action(&projection.close_action),
        sections: projection
            .sections
            .iter()
            .map(|section| SessionSectionProjectionV1 {
                section: match section.section {
                    SessionSection::Local => ControlSessionSectionV1::Local,
                    SessionSection::Remote => ControlSessionSectionV1::Remote,
                },
                title: section.title.clone(),
                state: match section.state {
                    polar_tmux_workspace::SessionSectionState::Loading => {
                        SessionSectionStateV1::Loading
                    }
                    polar_tmux_workspace::SessionSectionState::Populated => {
                        SessionSectionStateV1::Populated
                    }
                    polar_tmux_workspace::SessionSectionState::Empty => {
                        SessionSectionStateV1::Empty
                    }
                    polar_tmux_workspace::SessionSectionState::TargetMissing => {
                        SessionSectionStateV1::TargetMissing
                    }
                    polar_tmux_workspace::SessionSectionState::Error => {
                        SessionSectionStateV1::Error
                    }
                },
                message: section.message.clone(),
                row_count: section.row_count,
            })
            .collect(),
        remote_targets: projection
            .remote_targets
            .iter()
            .map(|target| SessionTargetProjectionV1 {
                id: target.id.to_string(),
                label: target.label.clone(),
            })
            .collect(),
        rows: projection
            .rows
            .iter()
            .map(|row| SessionRowProjectionV1 {
                identity: wire_session_key(&row.identity),
                session_kind: match row.session_kind {
                    polar_tmux_workspace::SessionKind::Terminal => polar_tmux_protocol::SessionKindV1::Terminal,
                    polar_tmux_workspace::SessionKind::CodexPlane => polar_tmux_protocol::SessionKindV1::CodexPlane,
                    polar_tmux_workspace::SessionKind::ClaudePlane => polar_tmux_protocol::SessionKindV1::ClaudePlane,
                },
                original_agent_id: row.original_agent_id.clone(),
                display_name: row.display_name.clone(),
                detail_label: row.detail_label.clone(),
                target_label: row.target_label.clone(),
                section: match row.section {
                    SessionSection::Local => ControlSessionSectionV1::Local,
                    SessionSection::Remote => ControlSessionSectionV1::Remote,
                },
                attached_clients: row.attached_clients,
                transport_status: wire_transport_status(row.transport_status),
                selected: row.selected,
                assigned_to_selected_tab: row.assigned_to_selected_tab,
                assignment_action: wire_action(&row.assignment_action),
                assignment_actions: row.assignment_actions.as_ref().map(|actions| {
                    polar_tmux_protocol::SessionAssignmentActionsV1 {
                        add: wire_action(&actions.add),
                        remove: wire_action(&actions.remove),
                        tab_full: wire_action(&actions.tab_full),
                        assigned_elsewhere: wire_action(&actions.assigned_elsewhere),
                        no_managed_tab: wire_action(&actions.no_managed_tab),
                    }
                }),
                rename_action: wire_action(&row.rename_action),
                kill_action: wire_action(&row.kill_action),
                restart_action: row.restart_action.as_ref().map(wire_action),
                replacement: row.replacement.clone(),
                replacement_action: row.replacement_action.as_ref().map(wire_action),
            })
            .collect(),
        selected: projection.selected.as_ref().map(wire_session_key),
        local_execution_target_id: projection.local_execution_target_id.to_string(),
        remote_execution_target_id: projection
            .remote_execution_target_id
            .as_ref()
            .map(ToString::to_string),
        local_new_shell: wire_action(&projection.local_new_shell),
        local_new_command: wire_action(&projection.local_new_command),
        remote_new_session: wire_action(&projection.remote_new_session),
    }
}

pub(super) fn wire_action(action: &ControlActionProjection) -> ControlActionProjectionV1 {
    ControlActionProjectionV1 {
        id: action.id.as_str().to_owned(),
        label: action.label.clone(),
        role: match action.role {
            ControlActionRole::Primary => ControlActionRoleV1::Primary,
            ControlActionRole::Secondary => ControlActionRoleV1::Secondary,
            ControlActionRole::Toggle => ControlActionRoleV1::Toggle,
            ControlActionRole::Recovery => ControlActionRoleV1::Recovery,
            ControlActionRole::Destructive => ControlActionRoleV1::Destructive,
        },
        enabled: action.enabled,
        selected: action.selected,
        disabled_reason: action.disabled_reason.clone(),
        confirmation: action
            .confirmation
            .as_ref()
            .map(|confirmation| ControlConfirmationV1 {
                id: confirmation.id.clone(),
                title: confirmation.title.clone(),
                message: confirmation.message.clone(),
                confirm_label: confirmation.confirm_label.clone(),
                destructive: confirmation.destructive,
            }),
        accessibility_label: action.accessibility_label.clone(),
    }
}

fn wire_session_key(session: &SessionKey) -> SessionIdentityV1 {
    SessionIdentityV1 {
        execution_target_id: session.execution_target_id.to_string(),
        tmux_session_id: session.tmux_session_id.to_string(),
    }
}

fn wire_transport_status(status: TransportStatus) -> ControlTransportStatusV1 {
    match status {
        TransportStatus::Online => ControlTransportStatusV1::Online,
        TransportStatus::Offline => ControlTransportStatusV1::Offline,
        TransportStatus::Recovering => ControlTransportStatusV1::Recovering,
    }
}
