//! Discover original Codex threads independently of optional terminal presentation.
use super::codex_control::{
    apply_status, binding_from_row, disconnected, loaded, permissions, CodexControlService,
    ControlResult,
};
use super::imessage_sender::now_ms;
use polar_station_settings::CodexControlModeV1;
use polar_station_store_sqlite::{CodexBindingRow, CodexCheckpointRow};
use polar_station_protocol::{ApplicationStateApiV1, CodexConnectionStateV1};
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

pub(super) fn apply_thread_metadata(state: &mut polar_station_protocol::CodexStateV1, thread: &Value) {
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
            let target = descriptor.id.as_str();
            tokio::join!(self.discover_account_endpoint(target), self.discover_polar_endpoint(target));
        }
    }
    pub(super) async fn discover_account_endpoint(&self, target_id: &str) -> Option<String> {
        let network = self.network.as_ref()?;
        let id = format!("auto-codex-{target_id}");
        if self.config.read().await.endpoints.iter().any(|e| e.id == id
            && e.owner_host_id == self.owner && e.execution_target_id == target_id) {
            return Some(id);
        }
        let target = network.targets.get(target_id)?;
        let client = target.managed_client(&network.ssh).await.ok()?;
        let local = matches!(client.target(), polar_tmux_core::TmuxTarget::Local);
        let home = if local {
            polar_inhouse_backend_codex::native_codex_home().ok()?
        } else {
            let path = super::target_directory::target_output(client.target(),
                "if [ -n \"${CODEX_HOME:-}\" ]; then dir=$(CDPATH= cd -P -- \"$CODEX_HOME\" && pwd -P) || exit; printf '%s' \"$dir\"; else printf '%s' \"$HOME/.codex\"; fi",
            ).await.ok()?;
            std::path::PathBuf::from(String::from_utf8(path).ok()?)
        };
        let socket = polar_inhouse_backend_codex::client::app_server_control_socket_path(&home).ok()?;
        let mode = if local { CodexControlModeV1::Local } else { CodexControlModeV1::SshDirect };
        self.register_discovered_endpoint(id, target_id, mode, socket.as_path().to_str()?).await
    }

    async fn discover_polar_endpoint(&self, target_id: &str) -> Option<String> {
        use polar_inhouse_backend_codex::client::polar_control_socket;
        use std::path::{Path, PathBuf};
        let network = self.network.as_ref()?;
        let id = format!("auto-polar-{target_id}");
        if self.config.read().await.endpoints.iter().any(|e| e.id == id
            && e.owner_host_id == self.owner && e.execution_target_id == target_id) {
            return Some(id);
        }
        let target = network.targets.get(target_id)?;
        let client = target.managed_client(&network.ssh).await.ok()?;
        let local = matches!(client.target(), polar_tmux_core::TmuxTarget::Local);
        let home = if local {
            std::env::home_dir()?
        } else {
            let absolute = polar_control_socket(Path::new("/")).ok()?;
            let relative = absolute.strip_prefix("/").ok()?.to_str()?;
            let command = format!("if [ -S \"$HOME\"/{} ]; then printf '%s' \"$HOME\"; fi", polar_ssh::quote_posix(relative));
            let home = super::target_directory::target_output(client.target(), &command).await.ok()?;
            PathBuf::from(String::from_utf8(home).ok()?)
        };
        let socket = polar_control_socket(&home).ok()?;
        if local {
            #[cfg(unix)]
            {
                use std::os::unix::fs::{FileTypeExt, PermissionsExt};
                let endpoint = polar_inhouse_backend_codex::client::verified_local_daemon_socket(&socket).ok()??;
                let metadata = tokio::fs::symlink_metadata(endpoint.as_path()).await.ok()?;
                if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o077 != 0 {
                    return None;
                }
            }
            #[cfg(not(unix))]
            return None;
        }
        let mode = if local { CodexControlModeV1::Local } else { CodexControlModeV1::SshDirect };
        self.register_discovered_endpoint(id, target_id, mode, socket.to_str()?).await
    }

    async fn register_discovered_endpoint(&self, id: String, target_id: &str, mode: CodexControlModeV1, socket: &str) -> Option<String> {
        let mut config = self.config.write().await;
        if let Some(existing) = config.endpoints.iter().find(|e| e.execution_target_id == target_id
            && e.socket_path == socket && e.owner_host_id == self.owner && e.control_mode == mode) {
            return Some(existing.id.clone());
        }
        if config.endpoints.iter().any(|e| e.id == id) { return None; }
        config.endpoints.push(polar_station_settings::CodexEndpointSettingsV1 {
            id: id.clone(), execution_target_id: target_id.to_owned(), owner_host_id: self.owner.clone(),
            control_mode: mode,
            socket_path: socket.into(), peer_connection_ref: None, remote_endpoint_id: None, remote_target_id: None,
        });
        drop(config);
        self.publisher.publish(ApplicationStateApiV1::ControlCenterV1);
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        Some(id)
    }

    pub(super) async fn check_default_endpoints(&self, target: &str) {
        let check = |id: Option<String>| async move {
            if let Some(id) = id { let _ = self.endpoint(&id).await; }
        };
        tokio::join!(
            async {
                let account = self.discover_account_endpoint(target).await;
                if target != crate::target_registry::LOCAL_TARGET_ID {
                    check(account).await;
                }
            },
            async { check(self.discover_polar_endpoint(target).await).await },
        );
    }

    pub(super) async fn local_account_endpoint_id(&self) -> Option<String> {
        let home = polar_inhouse_backend_codex::native_codex_home().ok()?;
        let socket = polar_inhouse_backend_codex::client::app_server_control_socket_path(&home).ok()?;
        self.config.read().await.endpoints.iter().find(|endpoint|
            endpoint.owner_host_id == self.owner
                && endpoint.execution_target_id == crate::target_registry::LOCAL_TARGET_ID
                && endpoint.control_mode == CodexControlModeV1::Local
                && std::path::Path::new(&endpoint.socket_path) == socket.as_path()
        ).map(|endpoint| endpoint.id.clone())
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
