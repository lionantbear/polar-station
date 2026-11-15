//! Discover original Codex threads independently of optional terminal presentation.
use super::codex_control::{
    apply_status, binding_from_row, disconnected, loaded, permissions, CodexControlService,
    ControlResult,
};
use super::imessage_sender::now_ms;
use polar_station_settings::CodexControlModeV1;
use polar_station_store_sqlite::{CodexBindingRow, CodexCheckpointRow};
use polar_tmux_protocol::{ApplicationStateApiV1, CodexConnectionStateV1};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn thread_name(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(|name| {
            name.chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect::<String>()
        })
        .filter(|name| !name.trim().is_empty())
}

pub(super) fn apply_thread_metadata(state: &mut polar_tmux_protocol::CodexStateV1, thread: &Value) {
    let m = &mut state.metadata;
    for (key, field) in [("preview", &mut m.preview), ("cwd", &mut m.cwd),
        ("parentThreadId", &mut m.parent_thread_id), ("agentRole", &mut m.agent_role),
        ("agentNickname", &mut m.agent_nickname)] {
        if let Some(value) = thread.get(key) {
            *field = value.as_str().map(|v| v.chars().filter(|c| !c.is_control()).take(4096).collect::<String>()).filter(|v| !v.trim().is_empty());
        }
    }
    if let Some(v) = thread["createdAt"].as_i64() { m.created_at = Some(v); }
    if let Some(v) = thread["updatedAt"].as_i64() { m.updated_at = Some(v); }
}

impl CodexControlService {
    /// Discover the account's upstream default socket, alongside explicit endpoints.
    /// No daemon is started and no application configuration is written.
    pub(super) async fn discover_account_endpoints(&self) {
        let Some(network) = &self.network else { return; };
        for descriptor in network.targets.descriptors() {
            self.discover_account_endpoint(descriptor.id.as_str()).await;
        }
    }
    pub(super) async fn discover_account_endpoint(&self,target_id:&str) {
        let Some(network)=&self.network else{return};
            let id = format!("auto-codex-{target_id}");
            if self.config.read().await.endpoints.iter().any(|e| e.id == id) { return; }
            let Some(target) = network.targets.get(target_id) else { return; };
            let Ok(client) = target.managed_client(&network.ssh).await else { return; };
            let local = matches!(client.target(), polar_tmux_core::TmuxTarget::Local);
            let home = if local {
                std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".codex")))
            } else {
                super::target_directory::target_output(client.target(),
                    "printf '%s' \"${CODEX_HOME:-$HOME/.codex}\"").await.ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok()).map(std::path::PathBuf::from)
            };
            let Some(home) = home else { return; };
            let Ok(socket) = codex_app_server_client::app_server_control_socket_path(&home) else { return; };
            let Some(socket) = socket.as_path().to_str() else { return; };
            let mut config = self.config.write().await;
            if config.endpoints.iter().any(|e| e.execution_target_id == target_id && e.socket_path == socket) { return; }
            config.endpoints.push(polar_station_settings::CodexEndpointSettingsV1 {
                id, execution_target_id: target_id.to_owned(), owner_host_id: self.owner.clone(),
                control_mode: if local { CodexControlModeV1::Local } else { CodexControlModeV1::SshDirect },
                socket_path: socket.into(), peer_connection_ref: None, remote_endpoint_id: None, remote_target_id: None,
            });
            drop(config);
            self.publisher.publish(ApplicationStateApiV1::ControlCenterV1);
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
    }

    // The recovery tick may call this, but a healthy generation performs no RPC.
    // New threads arrive through the upstream global thread/started notification.
    
    pub(super) async fn discover_with_topology(&self, topology: &polar_station_settings::MessageConnectorSettingsV1) {
        let endpoints = self.config.read().await.endpoints.clone();
        for endpoint in endpoints.into_iter().filter(|e| {
            e.owner_host_id == self.owner
                && matches!(
                    e.control_mode,
                    CodexControlModeV1::Local | CodexControlModeV1::SshDirect
                )
                && !topology.disabled_monitors.contains(&e.id)
        }) {
            let Ok(client) = self.endpoint(&endpoint.id).await else {
                continue;
            };
            let Some(generation) = self
                .endpoints
                .lock()
                .await
                .get(&endpoint.id)
                .map(|c| c.generation)
            else {
                continue;
            };
            if self.inventories.lock().await.get(&endpoint.id) == Some(&generation) {
                continue;
            }
            let Ok(threads) = loaded(&client).await else {
                continue;
            };
            {
                let mut states = self.states.write().await;
                states.retain(|_, s| s.binding.endpoint_id != endpoint.id || threads.contains(&s.binding.thread_id) || s.binding.session.is_some());
                self.observations.lock().unwrap().retain(|id, _| states.contains_key(id));
            }
            let mut success = true;
            for thread in threads {
                let Ok(id) = self
                    .observe_loaded_thread(&endpoint.id, &thread, None)
                    .await
                else {
                    success = false;
                    continue;
                };
                // A failed join is repaired individually; it must not turn
                // a healthy catalog into periodic full inventory scanning.
                if let Err(code) = self.join_loaded(&id, &client).await {
                    if code == "codex_rejected" {
                        if let Some(state) = self.states.write().await.get_mut(&id) {
                            state.connection = CodexConnectionStateV1::Stale;
                            state.error_code = Some(code.into());
                            permissions(state);
                        }
                    }
                    // A loaded empty thread may not have a rollout yet. Anchor
                    // observation now so its first later completion is not
                    // mistaken for historical work when a status event rejoins.
                    if let Ok(row) = self.persist_binding(&id) {
                        if matches!(self.ledger.checkpoint(&id), Ok(None)) {
                            let _ = self.ledger.record_completion(
                                CodexCheckpointRow {
                                    binding_id: id,
                                    binding_revision: row.revision,
                                    last_reconciled_turn: None,
                                    bound_at_ms: row.bound_at_ms,
                                    reconcile_status: "current".into(),
                                },
                                vec![],
                            );
                        }
                    }
                }
            }
            if success {
                self.inventories
                    .lock()
                    .await
                    .insert(endpoint.id.clone(), generation);
            }
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
        }
    }

    /// Explicit Resume is the only operation that reenables released history.
    pub(super) async fn enable_original_thread(&self, endpoint: &str, thread: &str) -> ControlResult<()> {
        for mut row in self.ledger.bindings().map_err(|_| "ledger_unavailable")? {
            if row.owner_host_id != self.owner || row.endpoint_id != endpoint || row.thread_id != thread || row.enabled { continue; }
            let previous = row.revision;
            row.enabled = true;
            row.session_key = "null".into();
            row.revision = previous.checked_add(1).ok_or("revision_exhausted")?;
            if !self.ledger.save_binding(previous, row.clone()).map_err(|_| "ledger_unavailable")? { return Err("binding_conflict"); }
            self.states.write().await.insert(row.id.clone(), disconnected(binding_from_row(row)?));
        }
        Ok(())
    }

    pub(super) async fn observe_loaded_thread(
        &self,
        endpoint: &str,
        thread: &str,
        since: Option<i64>,
    ) -> ControlResult<String> {
        if let Some(state) = self
            .states
            .read()
            .await
            .values()
            .find(|s| s.binding.endpoint_id == endpoint && s.binding.thread_id == thread)
        {
            return Ok(state.binding.binding_id.clone());
        }
        let digest = Sha256::digest(
            serde_json::to_vec(&(&self.owner, endpoint, thread)).map_err(|_| "invalid_thread")?,
        );
        let id = format!("codex-{digest:x}");
        let existing = self.ledger.binding(&id).map_err(|_| "ledger_unavailable")?;
        if existing.as_ref().is_some_and(|r| !r.enabled) { return Err("thread_released"); }
        let previous = existing.as_ref().map_or(0, |r| r.revision);
        let config = self.config.read().await;
        let destination = config
            .telegram
            .destinations
            .iter()
            .find(|d| d.id == "current")
            .or_else(|| {
                (config.telegram.destinations.len() == 1).then(|| &config.telegram.destinations[0])
            })
            .map(|d| d.id.clone());
        let row = existing.unwrap_or(CodexBindingRow {
            id: id.clone(),
            owner_host_id: self.owner.clone(),
            revision: previous.checked_add(1).ok_or("revision_exhausted")?,
            session_key: "null".into(),
            endpoint_id: endpoint.into(),
            thread_id: thread.into(),
            enabled: true,
            telegram_destination_ref: destination,
            imessage_enabled: config.imessage_enabled,
            bound_at_ms: since.unwrap_or_else(now_ms),
        });
        drop(config);
        self.observations.lock().unwrap().insert(id.clone(), super::codex_control::Observation { row: row.clone(), started: since.is_some() });
        self.states
            .write()
            .await
            .insert(id.clone(), disconnected(binding_from_row(row)?));
        Ok(id)
    }

    pub(super) async fn discover_thread_event(&self, endpoint: &str, thread: &Value) {
        let Some(thread_id) = thread["id"].as_str() else {
            return;
        };
        let Ok(id) = self
            .observe_loaded_thread(endpoint, thread_id, Some(now_ms()))
            .await
        else {
            return;
        };
        // Global discovery does not establish a turn subscription. Empty threads
        // may not have a rollout yet; wait for global active/idle status to retry
        // a rejected join, without turning recovery into inventory polling.
        if let Some(state) = self.states.write().await.get_mut(&id) {
            apply_thread_metadata(state, thread);
            state.thread_name = thread_name(&thread["name"]).or_else(|| state.thread_name.clone());
            apply_status(state, &thread["status"]);
        }
        if let Err(code) = self.join(&id, false).await {
            if let Some(state) = self.states.write().await.get_mut(&id) {
                state.connection = CodexConnectionStateV1::Stale;
                state.error_code = Some(code.into());
                permissions(state);
            }
        }
        if let Some(state) = self.states.write().await.get_mut(&id) {
            permissions(state);
        }
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
    }

    pub(super) async fn reload_association(&self, id: &str) -> ControlResult<()> {
        let row = self.binding_row(id)?;
        if let Some(state) = self.states.write().await.get_mut(id) { state.binding = binding_from_row(row)?; }
        Ok(())
    }

}
