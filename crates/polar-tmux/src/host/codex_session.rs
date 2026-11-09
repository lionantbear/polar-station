//! A terminal is optional presentation of an original app-server Thread.
//! Its recovery record stays with the terminal Host; monitoring stays with the owner.
use super::{codex_control::CodexControlService, session_lifecycle::{codex_cli_command, ensure_codex_launcher, SessionLifecycle}};
use polar_tmux_protocol::*;
use polar_station_settings::CodexControlModeV1;
use polar_tmux_workspace::{ExecutionTargetId, ProcessRestoreSpec, SessionFact, SessionLaunchSpec};
use std::sync::Arc;

fn error(code: &str, message: impl Into<String>) -> ControlCenterErrorV1 {
    ControlCenterErrorV1 { resume: None, code: code.into(), message: message.into() }
}

pub(crate) enum CodexTerminalStart {
    New { yolo: bool },
    Resume { thread_id: String },
}

impl CodexControlService {
    pub(crate) async fn rename_agent_thread(&self, principal: &str, owner: String, endpoint: String, thread: String, name: String) -> Result<(), ControlCenterErrorV1> {
        if !self.config.read().await.endpoints.iter().any(|e| e.id == endpoint && e.owner_host_id == owner) {
            return Err(error("endpoint_changed", "The original app-server endpoint is no longer configured."));
        }
        let response = self.dispatch(principal, true, CodexRequestEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION, api: CodexApiV1::CodexV1,
            request_id: crate::host_identity::random_uuid_v4(), owner_host_id: owner,
            command: CodexCommandV1::RenameThread { endpoint_id: endpoint, thread_id: thread, name },
        }).await;
        match response.result {
            CodexResultV1::Binding { .. } => Ok(()),
            CodexResultV1::Error { code } => Err(error(&code, format!("The original Thread could not be renamed ({code})."))),
            _ => Err(error("invalid_rename_response", "The app server did not confirm the rename.")),
        }
    }

    pub(crate) async fn catalog_endpoints(&self) -> Vec<polar_station_settings::CodexEndpointSettingsV1> {
        let config = self.config.read().await;
        if config.enabled { config.endpoints.clone() } else { Vec::new() }
    }

    pub(crate) async fn endpoint_states(&self) -> std::collections::BTreeMap<String, String> {
        let config = self.config.read().await;
        let connections = self.endpoints.lock().await;
        config.endpoints.iter().map(|endpoint| {
            let state = if !config.enabled { "Disabled" }
                else if connections.get(&endpoint.id).is_some_and(|c| c.client.connected()) { "Connected" }
                else if endpoint.control_mode == CodexControlModeV1::RemoteHost { "Managed by remote Host" }
                else { "Unavailable · catalog incomplete" };
            (endpoint.id.clone(), state.into())
        }).collect()
    }

    pub(crate) async fn session_catalog(&self, terminals: &mut [SessionRowProjectionV1]) -> Vec<AgentSessionCatalogRowV1> {
        let mut result = CodexResultV1::Bindings { bindings: self.states.read().await.values().cloned().collect() };
        self.project_terminal_associations(&mut result).await;
        let CodexResultV1::Bindings { bindings } = result else { unreachable!() };
        let config = self.config.read().await;
        let mut rows: Vec<AgentSessionCatalogRowV1> = Vec::new();
        for state in bindings {
            let Some(endpoint) = config.endpoints.iter().find(|e| e.id == state.binding.endpoint_id) else { continue };
            let name = catalog_title(&state);
            let mut status = catalog_status(&state).to_owned();
            let mut associated = self.original_terminals(&state).await;
            for terminal in terminals.iter().filter(|row|
                row.identity.execution_target_id == endpoint.execution_target_id
                    && row.original_agent_id.as_deref() == Some(state.binding.thread_id.as_str())) {
                if !associated.contains(&terminal.identity) { associated.push(terminal.identity.clone()); }
            }
            let mut per_host = std::collections::BTreeMap::new();
            for terminal in &associated { *per_host.entry(&terminal.execution_target_id).or_insert(0) += 1; }
            if per_host.values().any(|count| *count > 1) { status.push_str(" · Duplicate terminals on Host"); }
            for terminal in terminals.iter_mut().filter(|row| associated.contains(&row.identity)) {
                terminal.display_name = name.clone();
                terminal.detail_label = Some(format!("{} · {}", catalog_project(&state), status));
            }
            let loaded = state.connection == CodexConnectionStateV1::Connected;
            if !loaded && associated.is_empty() { continue; }
            if let Some(existing) = rows.iter_mut().find(|r| r.execution_target_id == endpoint.execution_target_id && r.thread_id == state.binding.thread_id) {
                for identity in associated { if !existing.terminals.contains(&identity) { existing.terminals.push(identity); } }
                if (loaded, state.metadata.updated_at) > (existing.loaded, existing.metadata.updated_at) {
                    existing.metadata = *state.metadata.clone(); existing.display_name = name; existing.status = status; existing.loaded = loaded;
                }
                continue;
            }
            rows.push(AgentSessionCatalogRowV1 {
                metadata: *state.metadata.clone(),
                terminals: associated,
                execution_target_id: endpoint.execution_target_id.clone(), owner_host_id: state.binding.owner_host_id,
                endpoint_id: endpoint.id.clone(), thread_id: state.binding.thread_id,
                display_name: name, status, loaded,
            });
        }
        rows.sort_by(|a,b| (&a.execution_target_id, &a.metadata.cwd, &a.display_name, &a.thread_id).cmp(&(&b.execution_target_id, &b.metadata.cwd, &b.display_name, &b.thread_id)));
        rows
    }

    pub(crate) async fn original_terminals(&self, state: &CodexStateV1) -> Vec<SessionIdentityV1> {
        let records = if let Some(lifecycle) = self.lifecycle.get() { lifecycle.snapshot().await.sessions } else { Vec::new() };
        let mut associated = Vec::new();
        for record in &records {
            if matches!(record.status, polar_tmux_workspace::SessionRecoveryStatus::Ended | polar_tmux_workspace::SessionRecoveryStatus::Manual) { continue; }
            if matches!(&record.launch.process,
                ProcessRestoreSpec::CodexAppServer { owner_host_id, endpoint_id, thread_id, .. }
                | ProcessRestoreSpec::CodexForwardedCli { owner_host_id, endpoint_id, thread_id, .. }
                if owner_host_id == &state.binding.owner_host_id && endpoint_id == &state.binding.endpoint_id && thread_id == &state.binding.thread_id) {
                associated.push(SessionIdentityV1 { execution_target_id:record.execution_target_id.to_string(), tmux_session_id:record.reference() });
            }
        }
        let aliases: Vec<_> = self.states.read().await.values().filter(|s| s.binding.owner_host_id == state.binding.owner_host_id
            && s.binding.endpoint_id == state.binding.endpoint_id && s.binding.thread_id == state.binding.thread_id)
            .filter_map(|s| s.binding.session.clone()).collect();
        for session in &aliases {
            // A runtime $ID and its managed reference are one terminal. An
            // ended record must not resurrect a stale catalog association.
            let record = records.iter().find(|r| r.execution_target_id.as_str() == session.execution_target.as_str()
                && (r.reference() == session.tmux_session_id || r.runtime_tmux_session_id.as_str() == session.tmux_session_id));
            if record.is_none() {
                if let Some(lifecycle) = self.lifecycle.get() {
                    let (facts, available) = lifecycle.list_sessions(session.execution_target.as_str()).await;
                    if available && !facts.iter().any(|f| f.identity.tmux_session_id.as_str() == session.tmux_session_id) { continue; }
                }
            }
            if record.is_none_or(|r| !matches!(r.status, polar_tmux_workspace::SessionRecoveryStatus::Ended | polar_tmux_workspace::SessionRecoveryStatus::Manual)) {
                let identity = SessionIdentityV1 { execution_target_id:session.execution_target.to_string(),
                    tmux_session_id:record.map_or_else(|| session.tmux_session_id.clone(), |r| r.reference()) };
                if !associated.contains(&identity) { associated.push(identity); }
            }
        }
        associated.sort_by(|a,b| (&a.execution_target_id,&a.tmux_session_id).cmp(&(&b.execution_target_id,&b.tmux_session_id)));
        associated
    }

    /// Create/resume a real Codex Thread without allocating a tmux terminal.
    /// Both local and SSH endpoints are addressed through their configured owner.
    pub(crate) async fn open_agent_thread(
        &self, target_id: &str, endpoint_id: &str, name: String,
        directory: Option<String>, resume_id: Option<String>, yolo: bool,
    ) -> Result<CodexBindingV1, ControlCenterErrorV1> {
        let endpoint = self.config.read().await.endpoints.iter()
            .find(|e| e.id == endpoint_id && e.execution_target_id == target_id)
            .cloned().ok_or_else(||error("endpoint_changed","The Codex app-server endpoint is not configured on the selected Host."))?;
        let command = if let Some(thread_id) = resume_id {
            CodexCommandV1::OpenThread { endpoint_id: endpoint.id, thread_id }
        } else {
            CodexCommandV1::StartThread { endpoint_id: endpoint.id, name: Some(name), working_directory: directory, yolo }
        };
        let response = self.dispatch("desktop-agent-session", true, CodexRequestEnvelopeV1 {
            protocol_version:PROTOCOL_VERSION,api:CodexApiV1::CodexV1,
            request_id:crate::host_identity::random_uuid_v4(),
            owner_host_id:endpoint.owner_host_id,command,
        }).await;
        match response.result {
            CodexResultV1::Binding { binding: Some(state) } => Ok(state.binding),
            CodexResultV1::Error { code } => Err(error(&code,format!("Codex app-server rejected the session operation ({code}); refresh the original Thread before retrying."))),
            _ => Err(error("codex_identity_missing","The app server did not confirm an original Thread ID; refresh before retrying.")),
        }
    }

    /// P3: expose the original app-server's private local or SSH-forwarded
    /// socket to a *client-Host-local* Codex CLI in that Host's tmux. This
    /// preflight is used before creating a new original Thread, so unsupported
    /// remote-owner settings do not accidentally create an orphan Agent.
    pub(crate) async fn preflight_local_cli_plane(&self, target_id: &str, endpoint_id: &str) -> Result<(), ControlCenterErrorV1> {
        let config = self.config.read().await;
        if !config.enabled { return Err(error("control_disabled", "Enable Codex control before opening a CLI plane.")); }
        let endpoint = config.endpoints.iter().find(|e| e.id == endpoint_id && e.execution_target_id == target_id)
            .ok_or_else(||error("endpoint_changed", "The original Codex endpoint is no longer configured on this Host."))?;
        if endpoint.owner_host_id != self.owner || endpoint.control_mode == CodexControlModeV1::RemoteHost {
            return Err(error("local_cli_requires_direct_endpoint", "Local CLI is available for local and SSH Direct endpoints owned by this Station Host. Use the remote tmux plane for a peer-owned endpoint."));
        }
        drop(config);
        let network = self.network.as_ref().ok_or_else(||error("target_unavailable", "The Station target registry is unavailable."))?;
        let local = ExecutionTargetId::local();
        let target = network.targets.get(local.as_str()).ok_or_else(||error("local_target_missing", "This Station Host has no local tmux target."))?;
        let terminal = target.managed_client(&network.ssh).await.map_err(|e|error("local_tmux_unavailable",e.to_string()))?;
        ensure_codex_launcher(terminal.target(), None).await.map_err(|e|error("local_codex_unavailable",e.to_string()))?;
        self.endpoint(endpoint_id).await.map_err(|code|error(code,"The original app-server local socket or SSH forward is unavailable."))?;
        Ok(())
    }

    /// Create an optional local operation plane, never a second Agent. A
    /// forwarded CLI must be reopened explicitly when its socket lease expires.
    pub(crate) async fn create_local_cli_plane(
        &self, target_id: &str, endpoint_id: &str, binding: &CodexBindingV1,
    ) -> Result<SessionFact, ControlCenterErrorV1> {
        self.preflight_local_cli_plane(target_id, endpoint_id).await?;
        if binding.endpoint_id != endpoint_id || binding.owner_host_id != self.owner {
            return Err(error("codex_identity_mismatch", "The original Thread belongs to a different endpoint or owner."));
        }
        let endpoint = self.endpoint(endpoint_id).await.map_err(|code|error(code,"The original app-server endpoint has disconnected."))?;
        if !endpoint.connected() { return Err(error("codex_unavailable", "The original app-server socket forward is disconnected.")); }
        // A read proves the target Thread identity without creating work.
        let read = endpoint.request("thread/read", serde_json::json!({"threadId": binding.thread_id, "includeTurns": false}))
            .await.map_err(|failure|error(failure.code(), "Cannot verify the original Thread before opening its CLI."))?;
        if read.pointer("/thread/id").and_then(serde_json::Value::as_str) != Some(binding.thread_id.as_str()) {
            return Err(error("thread_mismatch", "The app-server did not confirm the requested original Thread."));
        }
        let lifecycle = self.lifecycle.get().ok_or_else(||error("session_service_unavailable", "Session lifecycle is unavailable."))?;
        let network = self.network.as_ref().ok_or_else(||error("target_unavailable", "Target registry is unavailable."))?;
        let local = ExecutionTargetId::local();
        let target = network.targets.get(local.as_str()).ok_or_else(||error("local_target_missing", "The Station Host local terminal is unavailable."))?;
        let gate = target.operation_gate(); let _guard = gate.write().await;
        let terminal = target.managed_client(&network.ssh).await.map_err(|e|error("local_tmux_unavailable",e.to_string()))?;
        if let Some(record) = self.existing_terminal(&terminal, local.as_str(), &binding.thread_id).await? {
            return Ok(record);
        }
        let thread = history_thread(&read["thread"]);
        validate_agent_cwd(&thread.cwd)?;
        let name = terminal_name(&thread.thread_id)?;
        let socket_path = endpoint.local_socket_path().to_owned();
        let agent_local = self.config.read().await.endpoints.iter().any(|e| e.id == endpoint_id && e.control_mode == CodexControlModeV1::Local);
        let terminal_cwd = if agent_local { thread.cwd.clone() } else { std::env::var("HOME").map_err(|_|error("local_directory_missing","Cannot resolve the local terminal directory."))? };
        ensure_codex_launcher(terminal.target(), Some(&terminal_cwd)).await.map_err(|e|error("codex_dependency_unavailable",e.to_string()))?;
        let mut launch = SessionLaunchSpec::shell(name, Some(&terminal_cwd));
        launch.process = ProcessRestoreSpec::CodexForwardedCli {
            owner_host_id: binding.owner_host_id.clone(), endpoint_id: endpoint_id.into(),
            thread_id: binding.thread_id.clone(), socket_path: socket_path.clone(),
        };
        let command = codex_cli_command(&socket_path, &binding.thread_id, Some(&thread.cwd));
        let record = lifecycle.create_with_public_name(&terminal, local.clone(), launch, Some(command), true).await
            .map_err(|e|error("local_cli_failed",format!("Original Thread {} is unchanged; its local CLI plane failed: {e}",binding.thread_id)))?;
        lifecycle.request_refresh(local.clone());
        Ok(lifecycle.created_fact(&record, &target.descriptor().display_name,
            network.ssh.transport_generation(local.as_str()).await).await)
    }

    pub(crate) async fn end_rpc(&self, principal: &str, owner: &str, command: CodexCommandV1) -> Result<CodexResultV1, ControlCenterErrorV1> {
        let response = self.dispatch(principal, true, CodexRequestEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION, api: CodexApiV1::CodexV1,
            request_id: crate::host_identity::random_uuid_v4(), owner_host_id: owner.into(), command,
        }).await;
        match response.result {
            CodexResultV1::Error { code } if code == "thread_release_pending" => Err(error(&code, "Station has stopped this task, closed its terminals and released its subscription. The app server still reports the conversation loaded; another client or its unload delay may be holding it. History is kept for Resume.")),
            CodexResultV1::Error { code } => Err(error(&code, format!("Cannot confirm the original Codex operation ({code}). Refresh its current state before retrying."))),
            result => Ok(result),
        }
    }

    pub(crate) async fn read_original_thread(&self, owner: &str, endpoint: &str, thread: &str) -> Result<CodexHistoryThreadV1, ControlCenterErrorV1> {
        match self.end_rpc("desktop-thread-read", owner, CodexCommandV1::ReadThread { endpoint_id:endpoint.into(), thread_id:thread.into() }).await? {
            CodexResultV1::Thread { thread } => Ok(thread),
            _ => Err(error("invalid_thread_response", "Cannot read the original conversation.")),
        }
    }

    pub(crate) async fn history_detail_entry(&self, target: &str, endpoint: &str, thread_id: String) -> Result<SessionEditorResultV1, ControlCenterErrorV1> {
        let owner = self.config.read().await.endpoints.iter().find(|e|e.id == endpoint && e.execution_target_id == target)
            .map(|e|e.owner_host_id.clone()).ok_or_else(||error("endpoint_changed","The app server no longer belongs to this Host."))?;
        match self.end_rpc("desktop-history-detail", &owner, CodexCommandV1::ReadHistoryDetail { endpoint_id:endpoint.into(), thread_id }).await? {
            CodexResultV1::HistoryDetail { detail } => Ok(SessionEditorResultV1::HistoryDetail { detail }),
            _ => Err(error("invalid_history_response","The Host returned no conversation details.")),
        }
    }

    pub(crate) async fn history_entries(&self, target: &str, endpoint: &str, cursor: Option<String>, search_term: Option<String>, archived: bool, include_subagents: bool) -> Result<SessionEditorResultV1, ControlCenterErrorV1> {
        let owner = self.config.read().await.endpoints.iter().find(|e| e.id == endpoint && e.execution_target_id == target)
            .map(|e| e.owner_host_id.clone()).ok_or_else(|| error("endpoint_changed", "The app-server no longer belongs to this machine."))?;
        let response = self.dispatch("desktop-history", true, CodexRequestEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION, api: CodexApiV1::CodexV1,
            request_id: crate::host_identity::random_uuid_v4(), owner_host_id: owner,
            command: CodexCommandV1::ListHistory { endpoint_id: endpoint.into(), cursor, search_term, archived, include_subagents },
        }).await;
        match response.result {
            CodexResultV1::Error { code } => Err(error(&code, format!("Cannot load history from app-server {endpoint} ({code}). Retry the history query."))),
            CodexResultV1::History { threads, next_cursor } => Ok(SessionEditorResultV1::History { threads, next_cursor }),
            _ => Err(error("invalid_history_response", "The app-server returned no history page.")),
        }
    }

    pub(crate) async fn directory_entries(&self, target: &str, endpoint: &str, path: &str) -> Result<Vec<String>, ControlCenterErrorV1> {
        let owner = self.config.read().await.endpoints.iter().find(|e| e.id == endpoint && e.execution_target_id == target)
            .map(|e| e.owner_host_id.clone()).ok_or_else(|| error("endpoint_changed", "The endpoint no longer belongs to this machine."))?;
        match self.end_rpc("desktop-directory", &owner, CodexCommandV1::ReadDirectory { endpoint_id: endpoint.into(), path: path.into() }).await? {
            CodexResultV1::Directories { paths } => Ok(paths),
            _ => Err(error("invalid_directory_response", "The selected endpoint returned no directory listing.")),
        }
    }

    pub(crate) async fn terminal_end_state(&self, session: &polar_tmux_workspace::SessionKey) -> Result<Option<CodexStateV1>, ControlCenterErrorV1> {
        let record = if let Some(lifecycle) = self.lifecycle.get() { lifecycle.record_for(session.execution_target_id.as_str(), session.tmux_session_id.as_str()).await } else { None };
        let original = record.and_then(|r| match r.launch.process {
            ProcessRestoreSpec::CodexAppServer { owner_host_id, endpoint_id, thread_id, .. }
            | ProcessRestoreSpec::CodexForwardedCli { owner_host_id, endpoint_id, thread_id, .. } => Some((owner_host_id, endpoint_id, thread_id)),
            _ => None,
        });
        let original = match original {
            Some(value) => Some(value),
            None => self.states.read().await.values().find(|s| s.binding.session.as_ref().is_some_and(|key|
                key.execution_target.as_str() == session.execution_target_id.as_str() && key.tmux_session_id == session.tmux_session_id.as_str()))
                .map(|s| (s.binding.owner_host_id.clone(), s.binding.endpoint_id.clone(), s.binding.thread_id.clone())),
        };
        let Some((owner, endpoint, thread)) = original else { return Ok(None) };
        self.original_end_state(&owner, &endpoint, &thread).await.map(Some)
    }

    pub(crate) async fn original_end_state(&self, owner: &str, endpoint: &str, thread: &str) -> Result<CodexStateV1, ControlCenterErrorV1> {
        let result = self.end_rpc("desktop-session-end", owner, CodexCommandV1::GetBindings).await?;
        let CodexResultV1::Bindings { bindings } = result else { return Err(error("codex_state_unavailable", "The original Codex task could not be verified.")) };
        let state = bindings.into_iter().find(|s| s.binding.owner_host_id == owner && s.binding.endpoint_id == endpoint && s.binding.thread_id == thread)
            .ok_or_else(|| error("codex_state_unavailable", "The original Codex task could not be verified; refresh before ending it."))?;
        if state.connection != CodexConnectionStateV1::Connected {
            return Err(error("codex_state_unavailable", "The original app-server is disconnected; the terminal was kept."));
        }
        Ok(state)
    }

    pub(crate) async fn stop_terminal_task(&self, principal: &str, intent_id: &str, state: CodexStateV1) -> Result<(), ControlCenterErrorV1> {
        let owner = state.binding.owner_host_id.clone();
        let binding_id = state.binding.binding_id.clone();
        let turn = state.turn_id.clone();
        let mut changes = self.publisher.subscribe();
        let interrupt_error = if matches!(state.activity, CodexActivityV1::Active | CodexActivityV1::WaitingApproval | CodexActivityV1::WaitingInput) {
            let turn_id = turn.clone().ok_or_else(|| error("codex_turn_unknown", "The active Codex task identity is unavailable."))?;
            let result = self.end_rpc(principal, &owner, CodexCommandV1::Interrupt { binding_id: binding_id.clone(),
                expected_revision: state.binding.revision, intent_id: intent_id.into(), turn_id }).await;
            match result {
                Ok(CodexResultV1::Receipt { receipt: Some(r) }) if r.status == CodexSubmissionStatusV1::Accepted => None,
                Err(error) => Some(error),
                _ => Some(error("codex_stop_unknown", "The Codex interrupt result is not confirmed; the terminal was kept.")),
            }
        } else { None };
        let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let current = self.end_rpc(principal, &owner, CodexCommandV1::GetBinding { binding_id: binding_id.clone() }).await?;
                let CodexResultV1::Binding { binding: Some(current) } = current else {
                    return Err(error("codex_state_unavailable", "The original Codex task could not be verified."));
                };
                if current.binding.thread_id != state.binding.thread_id || current.binding.revision != state.binding.revision
                    || current.turn_id != turn {
                    return Err(error("codex_task_changed", "The Codex task changed. Review the new task before ending this Session."));
                }
                if current.connection != CodexConnectionStateV1::Connected {
                    return Err(error("codex_stop_unknown", "The original app-server disconnected; the terminal was kept."));
                }
                if matches!(current.activity, CodexActivityV1::Idle | CodexActivityV1::Completed | CodexActivityV1::Failed | CodexActivityV1::Interrupted) {
                    return Ok(());
                }
                // A task may complete naturally before interrupt admission. Only
                // fresh terminal evidence for the same task resolves that race.
                if let Some(error) = &interrupt_error { return Err(error.clone()); }
                // Observe the existing publisher; do not poll or hold the mutation lane.
                if changes.recv().await.is_err() { return Err(error("codex_stop_unknown", "Codex observation changed; refresh before retrying.")); }
            }
        }).await;
        completed.unwrap_or_else(|_| Err(error("codex_stop_unknown", "Codex has not confirmed stopping the task. The terminal was kept; refresh its state.")))
    }
}
impl CodexControlService {
    pub(crate) fn attach_session_lifecycle(&self, lifecycle: Arc<SessionLifecycle>) {
        assert!(self.lifecycle.set(lifecycle).is_ok());
    }

    // Called only under the terminal target's operation write gate.
    pub(super) async fn existing_terminal(
        &self, client: &polar_tmux_core::TmuxClient, target: &str,
        thread: &str,
    ) -> Result<Option<SessionFact>, ControlCenterErrorV1> {
        let lifecycle = self.lifecycle.get().ok_or_else(|| error("session_service_unavailable", "Session service is unavailable."))?;
        lifecycle.existing_public_session(client, target, &terminal_name(thread)?).await
            .map_err(|e| error("existing_terminal_unavailable", e.to_string()))
    }

    pub(crate) async fn create_terminal(
        &self, target_id: String, endpoint_id: Option<String>, name: Option<String>,
        directory: Option<String>, source: Option<String>, start: CodexTerminalStart,
    ) -> Result<SessionFact, ControlCenterErrorV1> {
        let lifecycle=self.lifecycle.get().ok_or_else(||error("session_service_unavailable","Session service is unavailable."))?;
        let network=self.network.as_ref().ok_or_else(||error("target_unavailable","Target service is unavailable."))?;
        let endpoint={
            let config=self.config.read().await;
            if !config.enabled {return Err(error("control_disabled","Enable Codex control in Connections before creating a Codex Session."));}
            let candidates=config.endpoints.iter().filter(|e|e.execution_target_id==target_id && endpoint_id.as_ref().is_none_or(|id|id==&e.id)).collect::<Vec<_>>();
            if candidates.len()!=1 {return Err(error("endpoint_required","Select one configured Codex app-server endpoint for this machine."));}
            candidates[0].clone()
        };
        let target=network.targets.get(&target_id).ok_or_else(||error("target_missing","The selected machine is no longer configured."))?;
        let gate=target.operation_gate();let _guard=gate.write().await;
        let client=target.managed_client(&network.ssh).await.map_err(|e|error("ssh_unavailable",e.to_string()))?;
        if let CodexTerminalStart::Resume { thread_id } = &start {
            if let Some(record) = self.existing_terminal(&client, &target_id, thread_id).await? {
                return Ok(record);
            }
        }
        let directory=if let Some(source)=source {
            let generation=client.server_generation().await.map_err(|e|error("tmux_unavailable",e.to_string()))?;
            let id=lifecycle.resolve(&client,&target_id,&source).await.map_err(|e|error("source_unavailable",e.to_string()))?;
            let panes=client.list_panes(&id).await.map_err(|e|error("source_unavailable",e.to_string()))?;
            let path=panes.iter().find(|p|p.active&&!p.dead&&!p.current_path.is_empty()).ok_or_else(||error("source_unavailable","The source Session has no active directory."))?.current_path.clone();
            if client.server_generation().await.map_err(|e|error("tmux_unavailable",e.to_string()))? != generation {return Err(error("source_changed","The source tmux server changed. Refresh before creating a Session."));}
            Some(path)
        }else{directory};
        // The original Thread owns Resume's cwd and name. A client's draft is not authority.
        let saved = if let CodexTerminalStart::Resume { thread_id } = &start {
            Some(self.read_original_thread(&endpoint.owner_host_id, &endpoint.id, thread_id).await?)
        } else { None };
        let directory = saved.as_ref().map(|t|t.cwd.clone()).or(directory)
            .ok_or_else(||error("directory_required","Choose a working directory before creating the conversation."))?;
        validate_agent_cwd(&directory)?;
        ensure_codex_launcher(client.target(),Some(&directory)).await.map_err(|e|error("codex_dependency_unavailable",e.to_string()))?;
        let display_name = saved.as_ref().map(|t|t.display_name.clone()).or(name).unwrap_or_else(||
            std::path::Path::new(&directory).file_name().map(|v|v.to_string_lossy().into_owned()).unwrap_or_else(||"Codex".into()));
        let response=self.dispatch("desktop-session-create",true,CodexRequestEnvelopeV1 {
            protocol_version:PROTOCOL_VERSION,api:CodexApiV1::CodexV1,request_id:crate::host_identity::random_uuid_v4(),
            owner_host_id:endpoint.owner_host_id.clone(),command:match start {
                CodexTerminalStart::New { yolo } => CodexCommandV1::StartThread{endpoint_id:endpoint.id.clone(),name:Some(display_name.clone()),working_directory:Some(directory.clone()),yolo},
                CodexTerminalStart::Resume { thread_id } => CodexCommandV1::OpenThread { endpoint_id:endpoint.id.clone(), thread_id },
            },
        }).await;
        let binding=match response.result {
            CodexResultV1::Binding{binding:Some(state)}=>state.binding,
            CodexResultV1::Error{code}=>return Err(error(&code,format!("Codex Thread creation failed ({code}). No terminal was created; an unknown result must be checked before retrying."))),
            _=>return Err(error("invalid_codex_response","The app-server returned no Thread identity. Check the endpoint before retrying.")),
        };
        let recovery = AgentSessionResumeV1 { execution_target_id: target_id.clone(), endpoint_id: endpoint.id.clone(), thread_id: binding.thread_id.clone() };
        let mut launch=SessionLaunchSpec::shell(terminal_name(&binding.thread_id)?,Some(&directory));
        let command=codex_cli_command(&endpoint.socket_path, &binding.thread_id, Some(&directory));
        launch.process=ProcessRestoreSpec::CodexAppServer{owner_host_id:binding.owner_host_id.clone(),endpoint_id:endpoint.id,thread_id:binding.thread_id.clone(),socket_path:endpoint.socket_path};
        let record=lifecycle.create_with_public_name(&client,ExecutionTargetId::new(&target_id).map_err(|e|error("invalid_target",e.to_string()))?,launch,Some(command),true).await
            .map_err(|e| { let mut failure = error("codex_terminal_failed",format!("The conversation exists, but its terminal could not open: {e}. Retry resumes the same conversation.")); failure.resume = Some(recovery.clone()); failure })?;
        let session=SessionKeyV1{execution_target:ExecutionTarget::new(target_id.clone()).expect("validated target"),tmux_session_id:record.reference()};
        lifecycle.request_refresh(record.execution_target_id.clone());
        if binding.owner_host_id==self.owner {
            self.associate_terminal_session(&binding.binding_id, session).await
                .map_err(|code| { let mut failure = error(code,"The conversation and terminal exist. Retry reuses them."); failure.resume = Some(recovery.clone()); failure })?;
        }
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        Ok(lifecycle.created_fact(&record,&target.descriptor().display_name,network.ssh.transport_generation(&target_id).await).await)
    }

    pub(super) async fn associate_terminal_session(
        &self, binding_id: &str, session: SessionKeyV1,
    ) -> super::codex_control::ControlResult<()> {
        // A join publishes its snapshot after RPC/history awaits. Use its
        // admission lane so that publication cannot erase this new association.
        let _operation = self.operations.lock().await;
        let row = self.binding_row(binding_id)?;
        let mut affected = std::collections::BTreeSet::from([session.clone()]);
        if let Ok(Some(previous)) = serde_json::from_str::<Option<SessionKeyV1>>(&row.session_key) {
            affected.insert(previous);
        }
        if !self.ledger.associate_session(
            binding_id.into(), row.revision,
            serde_json::to_string(&Some(session)).map_err(|_| "invalid_session")?,
        ).map_err(|_| "ledger_unavailable")? {
            return Err("binding_conflict");
        }
        self.reload_association(binding_id).await?;
        self.synchronize_attention_for(Some(&affected)).await;
        Ok(())
    }

    pub(super) async fn project_terminal_associations(&self, result: &mut CodexResultV1) {
        if !matches!(result, CodexResultV1::Binding { binding: Some(_) } | CodexResultV1::Bindings { .. } | CodexResultV1::Completions { .. }) { return; }
        let Some(lifecycle)=self.lifecycle.get()else{return};
        let records=lifecycle.snapshot().await.sessions;
        let lookup_table = records.iter().filter(|record| record.status != polar_tmux_workspace::SessionRecoveryStatus::Ended)
            .filter_map(|record| match &record.launch.process {
                ProcessRestoreSpec::CodexAppServer { owner_host_id, endpoint_id, thread_id, .. } | ProcessRestoreSpec::CodexForwardedCli { owner_host_id, endpoint_id, thread_id, .. } =>
                    Some(((owner_host_id.as_str(), endpoint_id.as_str(), thread_id.as_str()), SessionKeyV1 {
                        execution_target: ExecutionTarget::new(record.execution_target_id.to_string()).expect("stored target"),
                        tmux_session_id: record.reference(),
                    })),
                _ => None,
            }).collect::<std::collections::BTreeMap<_, _>>();
        let lookup = |owner: &str, endpoint: &str, thread: &str| lookup_table.get(&(owner, endpoint, thread)).cloned();
        let update=|state:&mut CodexStateV1|{if let Some(session)=lookup(&state.binding.owner_host_id,&state.binding.endpoint_id,&state.binding.thread_id){state.binding.session=Some(session);}};
        match result {
            CodexResultV1::Binding{binding:Some(state)}=>update(state),
            CodexResultV1::Bindings{bindings}=>bindings.iter_mut().for_each(update),
            CodexResultV1::Completions{completions,..}=>for p in completions {
                let origin=&mut p.completion.origin;
                if let Some(session)=lookup(&origin.owner_host_id,&origin.endpoint_id,&origin.thread_id){origin.execution_target_id=session.execution_target.to_string();origin.tmux_session_id=Some(session.tmux_session_id);}
            },
            _=>{},
        }
    }
}

fn catalog_project(state: &CodexStateV1) -> &str {
    state.metadata.cwd.as_deref().and_then(|p| p.trim_end_matches('/').rsplit('/').next()).filter(|p| !p.is_empty()).unwrap_or("Unknown project")
}

pub(super) fn catalog_title(state: &CodexStateV1) -> String {
    if let Some(name) = state.thread_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        let generated = name.strip_prefix("Session-").is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()));
        if !generated && !matches!(name, "Codex conversation" | "Codex Session" | "New conversation") { return name.into(); }
    }
    if let Some(preview) = state.metadata.preview.as_deref().filter(|p| !p.trim().is_empty()) {
        return preview.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
    }
    let time = state.metadata.created_at.and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
        .map(|t| format!("{} {:02}:{:02} UTC", t.date(), t.hour(), t.minute())).unwrap_or_else(|| "Undated".into());
    format!("{} · New conversation · {}", catalog_project(state), time)
}

fn catalog_status(state: &CodexStateV1) -> &'static str {
    if state.connection != CodexConnectionStateV1::Connected { return "Unavailable"; }
    match state.activity {
        CodexActivityV1::Idle => "Idle", CodexActivityV1::Active => "Working",
        CodexActivityV1::WaitingApproval => "Needs approval", CodexActivityV1::WaitingInput => "Needs input",
        CodexActivityV1::Completed => "Completed", CodexActivityV1::Failed => "Failed", CodexActivityV1::Interrupted => "Interrupted",
    }
}

fn validate_agent_cwd(path: &str) -> Result<(), ControlCenterErrorV1> {
    if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(error("directory_unavailable", "The original conversation has no usable working directory."));
    }
    Ok(())
}

fn terminal_name(thread: &str) -> Result<String, ControlCenterErrorV1> {
    polar_tmux_workspace::SessionKind::CodexPlane.terminal_name(thread)
        .ok_or_else(|| error("invalid_thread_identity", "Codex returned an invalid original Thread ID."))
}

pub(super) fn history_text(value: &serde_json::Value, limit: usize) -> Option<String> {
    value.as_str().map(|text|text.chars().filter(|c|!c.is_control() || *c == '\n' || *c == '\t').take(limit).collect::<String>()).filter(|text|!text.trim().is_empty())
}
pub(super) fn history_thread(thread: &serde_json::Value) -> CodexHistoryThreadV1 {
    CodexHistoryThreadV1 {
        source_path: thread.get("path").and_then(serde_json::Value::as_str).map(str::to_owned),
        thread_id: thread["id"].as_str().unwrap_or_default().into(),
        display_name: super::codex_discovery::thread_name(&thread["name"])
            .or_else(||super::codex_discovery::thread_name(&thread["preview"]))
            .unwrap_or_else(||"Untitled conversation".into()),
        preview: history_text(&thread["preview"], 4096),
        cwd: thread["cwd"].as_str().unwrap_or_default().into(),
        updated_at: thread["updatedAt"].as_i64().unwrap_or_default(),
        status: thread["status"]["type"].as_str().unwrap_or("unknown").into(),
    }
}
pub(super) fn history_detail(thread: CodexHistoryThreadV1, turn: &serde_json::Value) -> CodexHistoryDetailV1 {
    let mut detail = CodexHistoryDetailV1 { thread, turn_id:history_text(&turn["id"],256), turn_status:history_text(&turn["status"],64), user_text:None, assistant_text:None };
    for item in turn["items"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("userMessage") => {
                let text = item["content"].as_array().into_iter().flatten().filter(|v|v["type"]=="text")
                    .filter_map(|v|v["text"].as_str()).collect::<Vec<_>>().join("\n");
                detail.user_text=history_text(&serde_json::Value::String(text),8192);
            }
            Some("agentMessage") if item["phase"].as_str().is_none_or(|p|p=="final_answer") => {
                detail.assistant_text=history_text(&item["text"],16384);
            }
            _ => {}
        }
    }
    detail
}


