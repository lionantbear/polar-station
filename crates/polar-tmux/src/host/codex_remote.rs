//! A desktop routes typed intents; the execution owner retains every durable fact.
use super::{
    codex_control::{CodexControlService, ControlResult},
    peer_client::PeerClient,
};
use polar_station_settings::{
    CodexControlModeV1, CodexEndpointSettingsV1, HostConnectionSettingsV1,
};
use polar_tmux_protocol::*;
use std::sync::Arc;

pub(super) struct RemoteConnection {
    pub connection: HostConnectionSettingsV1,
    client: Arc<PeerClient>,
}

impl CodexControlService {
    pub(super) async fn remote_command(
        &self,
        request: &CodexRequestEnvelopeV1,
    ) -> ControlResult<CodexResultV1> {
        // The owner applies its own revision/turn checks. This gate protects the
        // desktop's route from being replaced while an intent is in flight.
        let _gate = self.operations.lock().await;
        let config = self.config.read().await.clone();
        if !config.enabled {
            return Err("control_disabled");
        }
        let routes: Vec<_> = config
            .endpoints
            .iter()
            .filter(|e| {
                e.control_mode == CodexControlModeV1::RemoteHost
                    && e.owner_host_id == request.owner_host_id
            })
            .collect();
        if routes.is_empty() {
            return Err("owner_route_unavailable");
        }
        let connection = config
            .host_connections
            .iter()
            .find(|c| c.host_id == request.owner_host_id && c.execution_credential_ref.is_some())
            .ok_or("owner_route_unavailable")?;
        if routes
            .iter()
            .any(|r| r.peer_connection_ref.as_deref() != Some(connection.id.as_str()))
        {
            return Err("owner_route_unavailable");
        }
        if matches!(
            request.command,
            CodexCommandV1::UpdateSettings { .. }
                | CodexCommandV1::SetupCurrentTelegram { .. }
                | CodexCommandV1::SetTelegramEnabled { .. }
                | CodexCommandV1::ConnectImessageSender { .. }
        ) {
            return Err("scope_denied");
        }
        let mut outgoing = request.clone();
        match &mut outgoing.command {
            CodexCommandV1::Bind {
                session,
                endpoint_id,
                ..
            } => {
                let route = route_by_alias(&routes, endpoint_id)?;
                if session.execution_target.as_str() != route.execution_target_id {
                    return Err("target_mapping_mismatch");
                }
                session.execution_target = ExecutionTarget::new(
                    route
                        .remote_target_id
                        .clone()
                        .ok_or("target_mapping_missing")?,
                )
                .map_err(|_| "target_mapping_invalid")?;
                *endpoint_id = route
                    .remote_endpoint_id
                    .clone()
                    .ok_or("endpoint_mapping_missing")?;
            }
            CodexCommandV1::StartThread { endpoint_id, .. }
            | CodexCommandV1::OpenThread { endpoint_id, .. }
            | CodexCommandV1::SubmitThreadText { endpoint_id, .. }
            | CodexCommandV1::GetThreadBinding { endpoint_id, .. }
            | CodexCommandV1::RenameThread { endpoint_id, .. }
            | CodexCommandV1::ReadThread { endpoint_id, .. }
            | CodexCommandV1::ReadHistoryDetail { endpoint_id, .. }
            | CodexCommandV1::ListHistory { endpoint_id, .. }
            | CodexCommandV1::ListLoaded { endpoint_id }
            | CodexCommandV1::ReadDirectory { endpoint_id, .. } => {
                *endpoint_id = route_by_alias(&routes, endpoint_id)?
                    .remote_endpoint_id
                    .clone()
                    .ok_or("endpoint_mapping_missing")?;
            }
            CodexCommandV1::GetCompletions {
                endpoint_id: Some(endpoint_id),
                ..
            } => {
                *endpoint_id = route_by_alias(&routes, endpoint_id)?
                    .remote_endpoint_id
                    .clone()
                    .ok_or("endpoint_mapping_missing")?;
            }
            _ => {}
        }
        let peer = {
            let mut peers = self.peers.lock().await;
            if peers
                .get(&connection.id)
                .is_some_and(|p| p.connection != *connection || !p.client.connected())
            {
                peers.remove(&connection.id);
            }
            if !peers.contains_key(&connection.id) {
                let client = PeerClient::connect(
                    self.network.as_ref().ok_or("ssh_unavailable")?,
                    connection,
                    &self.owner,
                    HostPeerScopeV1::ExecutionController,
                    self.peer_credentials.clone(),
                    self.publisher.clone(),
                )
                .await?;
                peers.insert(
                    connection.id.clone(),
                    RemoteConnection {
                        connection: connection.clone(),
                        client: Arc::new(client),
                    },
                );
            }
            peers
                .get(&connection.id)
                .ok_or("peer_unavailable")?
                .client
                .clone()
        };
        if matches!(outgoing.command, CodexCommandV1::SubmitThreadText { .. } | CodexCommandV1::GetThreadBinding { .. }) {
            let mut capabilities = outgoing.clone();
            capabilities.request_id = crate::host_identity::random_uuid_v4();
            capabilities.command = CodexCommandV1::GetUi;
            require_submit_contract(&request_peer(&peer, capabilities).await?)?;
        }
        if let Some(binding_id) = mutation_binding(&outgoing.command) {
            let check = CodexRequestEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: CodexApiV1::CodexV1,
                request_id: crate::host_identity::random_uuid_v4(),
                owner_host_id: outgoing.owner_host_id.clone(),
                command: CodexCommandV1::GetBinding {
                    binding_id: binding_id.into(),
                },
            };
            let CodexResultV1::Binding {
                binding: Some(state),
            } = request_peer(&peer, check).await?
            else {
                return Err("binding_not_found");
            };
            if state.binding.binding_id != binding_id || route_for_state(&routes, &state).is_none()
            {
                return Err("binding_not_mapped");
            }
        }
        let result = request_peer(&peer, outgoing).await?;
        let mut result=project(result, &routes)?;
        self.project_terminal_associations(&mut result).await;
        Ok(result)
    }

    
}
async fn request_peer(
    peer: &PeerClient,
    request: CodexRequestEnvelopeV1,
) -> ControlResult<CodexResultV1> {
    match peer
        .request(ClientApplicationMessage::CodexV1(Box::new(request)))
        .await?
    {
        HostApplicationMessage::CodexV1(response) => Ok(response.result),
        _ => Err("invalid_peer_response"),
    }
}
fn mutation_binding(command: &CodexCommandV1) -> Option<&str> {
    match command {
        CodexCommandV1::ReleaseThread { binding_id, .. }
        | CodexCommandV1::Unbind { binding_id, .. }
        | CodexCommandV1::Resume { binding_id, .. }
        | CodexCommandV1::UpdateNotifications { binding_id, .. }
        | CodexCommandV1::QueueText { binding_id, .. }
        | CodexCommandV1::Steer { binding_id, .. }
        | CodexCommandV1::Interrupt { binding_id, .. } => Some(binding_id),
        _ => None,
    }
}
fn route_by_alias<'a>(
    routes: &[&'a CodexEndpointSettingsV1],
    id: &str,
) -> ControlResult<&'a CodexEndpointSettingsV1> {
    routes
        .iter()
        .copied()
        .find(|r| r.id == id)
        .ok_or("endpoint_not_mapped")
}
fn route_for_state<'a>(
    routes: &[&'a CodexEndpointSettingsV1],
    state: &CodexStateV1,
) -> Option<&'a CodexEndpointSettingsV1> {
    routes.iter().copied().find(|r| {
        r.owner_host_id == state.binding.owner_host_id
            && r.remote_endpoint_id.as_deref() == Some(state.binding.endpoint_id.as_str())
            && state
                .binding
                .session
                .as_ref()
                .is_none_or(|s| r.remote_target_id.as_deref() == Some(s.execution_target.as_str()))
    })
}
fn translate(state: &mut CodexStateV1, route: &CodexEndpointSettingsV1) -> ControlResult<()> {
    state.binding.endpoint_id = route.id.clone();
    if let Some(session) = &mut state.binding.session {
        session.execution_target = ExecutionTarget::new(route.execution_target_id.clone())
            .map_err(|_| "target_mapping_invalid")?;
    }
    Ok(())
}
fn project(
    result: CodexResultV1,
    routes: &[&CodexEndpointSettingsV1],
) -> ControlResult<CodexResultV1> {
    match result {
        CodexResultV1::Completions {
            completions,
            next_before,
        } => {
            let mut mapped = Vec::new();
            for mut projection in completions {
                let o = &mut projection.completion.origin;
                if let Some(route) = routes.iter().find(|r| {
                    r.owner_host_id == o.owner_host_id
                        && r.remote_endpoint_id.as_deref() == Some(o.endpoint_id.as_str())
                        && r.remote_target_id.as_deref() == Some(o.execution_target_id.as_str())
                }) {
                    o.endpoint_id = route.id.clone();
                    o.execution_target_id = route.execution_target_id.clone();
                    mapped.push(projection);
                }
            }
            // Keep the owner's opaque cursor even when a page was fully filtered.
            Ok(CodexResultV1::Completions {
                completions: mapped,
                next_before,
            })
        }
        CodexResultV1::Bindings { bindings } => {
            let mut mapped = Vec::new();
            for mut state in bindings {
                if let Some(route) = route_for_state(routes, &state) {
                    translate(&mut state, route)?;
                    mapped.push(state);
                }
            }
            Ok(CodexResultV1::Bindings { bindings: mapped })
        }
        CodexResultV1::Binding {
            binding: Some(mut state),
        } => {
            let route = route_for_state(routes, &state).ok_or("binding_not_mapped")?;
            translate(&mut state, route)?;
            Ok(CodexResultV1::Binding {
                binding: Some(state),
            })
        }
        other => Ok(other),
    }
}

fn require_submit_contract(result: &CodexResultV1) -> ControlResult<()> {
    match result {
        CodexResultV1::Ui { labels } if labels.get("contract_version").and_then(|v| v.parse::<u16>().ok()) == Some(CODEX_CONTROL_API_VERSION) => Ok(()),
        _ => Err("unsupported_codex_contract"),
    }
}


