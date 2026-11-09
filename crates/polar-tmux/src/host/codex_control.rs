//! Original-thread control owned by the execution Host.
use super::application_state::{ApplicationStatePublisher, HostChange, wait_observer_change};
use super::codex_endpoint::{CodexEndpoint, EndpointEvent, RpcFailure};
use super::imessage_sender::now_ms;
use super::session_registry::SessionRegistry;
use codex_app_server_client::AppServerEvent;
use polar_station_settings::{CodexControlModeV1, CodexControlSettingsV1, SettingsRepository};
use polar_station_store_sqlite::{CodexBindingRow, SqliteCodexStore};
use polar_tmux_protocol::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};

pub(super) struct EndpointConnection {
    pub generation: u64,
    pub client: Arc<CodexEndpoint>,
}

pub(super) struct Observation {
    pub row: CodexBindingRow,
    pub started: bool,
}

pub(crate) struct CodexControlService {
    pub(super) lifecycle: std::sync::OnceLock<Arc<super::session_lifecycle::SessionLifecycle>>,
    pub(super) owner: String,
    pub(super) settings: SettingsRepository,
    pub(super) ledger: SqliteCodexStore,
    pub(super) publisher: ApplicationStatePublisher,
    pub(super) sessions: SessionRegistry,
    pub(super) network: Option<super::ssh_socket_forward::CodexNetwork>,
    pub(super) peer_credentials: Arc<dyn crate::host_identity::CredentialStore + Send + Sync>,
    pub(super) peers: Mutex<BTreeMap<String, super::codex_remote::RemoteConnection>>,
    pub(super) operations: Mutex<()>,
    pub(super) config: RwLock<CodexControlSettingsV1>,
    topology: RwLock<polar_station_settings::MessageConnectorSettingsV1>,
    pub(super) telegram_status: RwLock<CodexChannelStatusV1>,
    pub(super) states: RwLock<BTreeMap<String, CodexStateV1>>,
    pub(super) observations: std::sync::Mutex<BTreeMap<String, Observation>>,
    pub(super) endpoints: Mutex<BTreeMap<String, EndpointConnection>>,
    pub(super) inventories: Mutex<BTreeMap<String, u64>>,
    next_generation: AtomicU64,
    events: mpsc::Sender<EndpointEvent>,
}

pub(super) type ControlResult<T> = Result<T, &'static str>;

impl CodexControlService {
    pub fn open(
        owner: String,
        settings: SettingsRepository,
        ledger: SqliteCodexStore,
        publisher: ApplicationStatePublisher,
        sessions: SessionRegistry,
        network: Option<super::ssh_socket_forward::CodexNetwork>,
        peer_credentials: Arc<dyn crate::host_identity::CredentialStore + Send + Sync>,
    ) -> ControlResult<Arc<Self>> {
        let config = settings
            .load_codex_control()
            .map_err(|_| "settings_unavailable")?;
        let states = BTreeMap::new();
        let topology = settings.load_message_connectors().map_err(|_| "settings_unavailable")?;
        let (events, mut receiver) = mpsc::channel(64);
        let service = Arc::new(Self {
            lifecycle: std::sync::OnceLock::new(),
            owner,
            settings,
            ledger,
            publisher,
            sessions,
            network,
            peer_credentials,
            peers: Mutex::new(BTreeMap::new()),
            operations: Mutex::new(()),
            topology: RwLock::new(topology),
            config: RwLock::new(config),
            telegram_status: RwLock::new(CodexChannelStatusV1 {
                state: if cfg!(feature = "telegram-channel") {
                    CodexChannelStateV1::Disabled
                } else {
                    CodexChannelStateV1::Unsupported
                },
                error_code: None,
            }),
            states: RwLock::new(states),
            observations: std::sync::Mutex::new(BTreeMap::new()),
            endpoints: Mutex::new(BTreeMap::new()),
            inventories: Mutex::new(BTreeMap::new()),
            next_generation: AtomicU64::new(1),
            events,
        });
        let weak = Arc::downgrade(&service);
        let mut changes = service.publisher.subscribe_work();
        let mut session_events = service.sessions.subscribe();
        tokio::spawn(async move {
            let mut retry_at = None;
            let mut session_facts = BTreeMap::new();
            let mut retry_needed = if let Some(service) = weak.upgrade() {
                service.reconnect().await;
                service.needs_recovery().await
            } else { return; };
            loop {
                if retry_needed {
                    retry_at.get_or_insert_with(|| tokio::time::Instant::now() + std::time::Duration::from_secs(3));
                } else { retry_at = None; }
                let deadline = retry_at.unwrap_or_else(tokio::time::Instant::now);
                tokio::select! {
                    event = receiver.recv() => {
                        let (Some(service), Some(event)) = (weak.upgrade(), event) else { break };
                        if service.observe(event).await { retry_needed = service.needs_recovery().await; }
                    }
                    change = wait_observer_change(&mut changes) => {
                        let Some(service) = weak.upgrade() else { break };
                        match change {
                            Ok(HostChange::Recovery) => { retry_needed = service.needs_recovery().await; }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                service.reconnect().await;
                                retry_needed = service.needs_recovery().await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = session_events.recv() => {
                        let Some(service) = weak.upgrade() else { break };
                        match event {
                            Ok(event) => {
                                let mut affected = std::collections::BTreeSet::new();
                                for change in event.changes {
                                    match change {
                                        SessionFeedChangeV1::Upsert { session } => {
                                            let fact = (session.lifecycle, session.transport_generation);
                                            if session_facts.get(&session.key) != Some(&fact) {
                                                session_facts.insert(session.key.clone(), fact);
                                                affected.insert(session.key);
                                            }
                                        }
                                        SessionFeedChangeV1::Remove { key } => { session_facts.remove(&key); }
                                    }
                                }
                                if !affected.is_empty() { service.synchronize_attention_for(Some(&affected)).await; }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                session_facts.clear();
                                service.synchronize_session_attention().await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = tokio::time::sleep_until(deadline), if retry_needed => {
                        retry_at = None;
                        let Some(service) = weak.upgrade() else { break };
                        service.reconnect().await;
                        retry_needed = service.needs_recovery().await;
                    }
                }
            }
        });
        Ok(service)
    }

    pub async fn dispatch(
        &self,
        principal: &str,
        may_configure: bool,
        request: CodexRequestEnvelopeV1,
    ) -> CodexResponseEnvelopeV1 {
        if request.validate().is_ok() && request.owner_host_id != self.owner && may_configure {
            let result = self.remote_command(&request).await;
            return valid_response(CodexResponseEnvelopeV1 {
                protocol_version: PROTOCOL_VERSION,
                api: CodexApiV1::CodexV1,
                request_id: request.request_id,
                owner_host_id: request.owner_host_id,
                result: result.unwrap_or_else(|code| CodexResultV1::Error { code: code.into() }),
            });
        }
        let result = if request.validate().is_err() {
            Err("invalid_request")
        } else if request.owner_host_id != self.owner {
            Err("owner_mismatch")
        } else {
            self.command(principal, may_configure, request.command)
                .await
        };
        valid_response(CodexResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: CodexApiV1::CodexV1,
            request_id: request.request_id,
            owner_host_id: self.owner.clone(),
            result: result.unwrap_or_else(|code| CodexResultV1::Error { code: code.into() }),
        })
    }

    async fn command(
        &self,
        principal: &str,
        may_configure: bool,
        command: CodexCommandV1,
    ) -> ControlResult<CodexResultV1> {
        // Reads remain usable while a mutation waits for its RPC response.
        match &command {
            CodexCommandV1::GetUi => return Ok(CodexResultV1::Ui {
                labels: [
                    ("contract_version", "2"),
                    ("title", "Codex control and notifications"),
                    ("close", "Close"),
                    ("refresh", "Refresh"),
                    ("save", "Save"),
                    ("cancel", "Cancel"),
                    ("connect", "Connect Codex"),
                    ("endpoint", "Codex endpoint"),
                    ("choose_endpoint", "Choose an endpoint"),
                    ("loaded", "Loaded conversations"),
                    ("find_loaded", "Find loaded conversations"),
                    (
                        "no_endpoint",
                        "Configure a Codex endpoint for this target in Settings.",
                    ),
                    (
                        "no_loaded",
                        "No loaded conversation is available on this endpoint.",
                    ),
                    ("telegram", "Telegram"),
                    ("imessage", "iMessage completion notifications"),
                    ("notifications", "Completion notifications"),
                    ("none", "Off"),
                    ("save_notifications", "Save notification choices"),
                    ("loading", "Connecting to the execution Host…"),
                    (
                        "operation_failed",
                        "The operation could not be completed. Refresh its state before retrying.",
                    ),
                    ("unsupported", "This Host does not support Codex control."),
                    ("completions", "Codex completions"),
                    ("more", "Load earlier completions"),
                    ("no_completions", "No Codex completion has been recorded."),
                    ("open_session", "Open original Session"),
                ]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            }),
            CodexCommandV1::GetTelegramStatus => {
                return Ok(CodexResultV1::TelegramStatus {
                    status: self.telegram_status.read().await.clone(),
                })
            }
            CodexCommandV1::GetSettings => {
                return Ok(CodexResultV1::Settings {
                    settings: self.config.read().await.clone(),
                })
            }
            CodexCommandV1::ReadThread { endpoint_id, thread_id }
            | CodexCommandV1::ReadHistoryDetail { endpoint_id, thread_id } => {
                let client = self.endpoint(endpoint_id).await?;
                let value = client.request("thread/read", json!({"threadId":thread_id,"includeTurns":false})).await.map_err(RpcFailure::code)?;
                if value["thread"]["id"].as_str() != Some(thread_id.as_str()) { return Err("thread_mismatch"); }
                let thread = super::codex_session::history_thread(&value["thread"]);
                if matches!(command, CodexCommandV1::ReadThread { .. }) {
                    return Ok(CodexResultV1::Thread { thread });
                }
                let turns = client.request("thread/turns/list", json!({"threadId":thread_id,"limit":1,"sortDirection":"desc","itemsView":"summary"})).await.map_err(RpcFailure::code)?;
                return Ok(CodexResultV1::HistoryDetail { detail: super::codex_session::history_detail(thread, &turns["data"][0]) });
            }
            CodexCommandV1::ListHistory { endpoint_id, cursor, search_term, archived, include_subagents } => {
                let client = self.endpoint(endpoint_id).await?;
                // Explicit filters avoid the upstream current-provider and interactive-only defaults.
                let mut sources = vec!["cli", "vscode", "exec", "appServer", "unknown"];
                if *include_subagents { sources.push("subAgent"); }
                let value = client.request("thread/list", json!({
                    "cursor": cursor, "limit": 50, "sortKey": "updated_at",
                    "sortDirection": "desc", "modelProviders": [], "sourceKinds": sources,
                    "archived": archived, "searchTerm": search_term, "useStateDbOnly": true,
                })).await.map_err(RpcFailure::code)?;
                let data = value["data"].as_array().ok_or("invalid_codex_response")?;
                let mut threads = Vec::with_capacity(data.len());
                for thread in data {
                    let id = thread["id"].as_str().ok_or("invalid_codex_response")?;
                    threads.push(CodexHistoryThreadV1 {
                        source_path: thread.get("path").and_then(Value::as_str).map(str::to_owned),
                        preview: super::codex_session::history_text(&thread["preview"], 4096),
                        thread_id: id.into(),
                        display_name: super::codex_discovery::thread_name(&thread["name"])
                            .or_else(|| super::codex_discovery::thread_name(&thread["preview"]))
                            .unwrap_or_else(|| "Untitled conversation".into()),
                        cwd: thread["cwd"].as_str().unwrap_or_default().into(),
                        updated_at: thread["updatedAt"].as_i64().unwrap_or_default(),
                        status: thread["status"]["type"].as_str().unwrap_or("unknown").into(),
                    });
                }
                return Ok(CodexResultV1::History { threads,
                    next_cursor: value["nextCursor"].as_str().map(str::to_owned) });
            }
            CodexCommandV1::GetBindings => {
                let states: Vec<_> = self.states.read().await.values().cloned().collect();
                let mut bindings = Vec::new();
                for mut state in states {
                    state.final_answer = None;
                    bindings.push(state);
                }
                return Ok(CodexResultV1::Bindings { bindings })
            }
            CodexCommandV1::GetCompletions {
                endpoint_id,
                before,
                limit,
            } => {
                return self.completion_page(endpoint_id.as_deref(), before.clone(), *limit);
            }
            CodexCommandV1::GetThreadBinding { endpoint_id, thread_id } => {
                let config = self.config.read().await;
                if !config.endpoints.iter().any(|e| e.id == *endpoint_id && e.owner_host_id == self.owner) { return Err("endpoint_not_found"); }
                drop(config);
                let state = self.states.read().await.values().find(|s| s.binding.endpoint_id == *endpoint_id && s.binding.thread_id == *thread_id).cloned();
                let digest = Sha256::digest(serde_json::to_vec(&(&self.owner, endpoint_id, thread_id)).map_err(|_| "invalid_thread")?);
                let binding = state.or_else(|| self.binding_row(&format!("codex-{digest:x}")).ok().and_then(|row| binding_from_row(row).ok()).map(disconnected));
                return Ok(CodexResultV1::Binding { binding });
            }
            CodexCommandV1::GetBinding { binding_id } => {
                return Ok(CodexResultV1::Binding {
                    binding: self.states.read().await.get(binding_id).cloned().or_else(|| {
                        self.binding_row(binding_id).ok().and_then(|row| binding_from_row(row).ok()).map(|binding| {
                            let mut state = disconnected(binding);
                            state.connection = CodexConnectionStateV1::Stale;
                            permissions(&mut state);
                            state
                        })
                    }),
                })
            }
            CodexCommandV1::GetReceipt { intent_id } => {
                return self.reconcile_receipt(principal, intent_id).await
            }
            _ => {}
        }
        // Dropped after the operation lock, including early-return errors.
        // Notify recovery without changing error-response projection semantics.
        let _recovery = self.publisher.recovery_after_mutation();
        let _operation = self.operations.lock().await;
        let result = match command {
            CodexCommandV1::ConnectImessageSender {
                expected_revision,
                ssh_target_id,
                port,
                pairing,
            } => {
                if !may_configure {
                    return Err("scope_denied");
                }
                self.connect_imessage_sender(expected_revision, &ssh_target_id, port, pairing)
                    .await
            }
            CodexCommandV1::UpdateSettings {
                expected_revision,
                settings,
            } => {
                if !may_configure {
                    return Err("scope_denied");
                }
                self.save_control_settings(expected_revision, settings)
                    .await
            }
            CodexCommandV1::SetupCurrentTelegram {
                expected_revision,
                allowed_sender_id,
            } => {
                if !may_configure {
                    return Err("scope_denied");
                }
                #[cfg(feature = "telegram-channel")]
                {
                    self.setup_current_telegram(expected_revision, allowed_sender_id)
                        .await
                }
                #[cfg(not(feature = "telegram-channel"))]
                {
                    let _ = (expected_revision, allowed_sender_id);
                    Err("telegram_unsupported")
                }
            }
            CodexCommandV1::SetTelegramEnabled {
                expected_revision,
                enabled,
            } => {
                if !may_configure {
                    return Err("scope_denied");
                }
                let mut settings = self.config.read().await.clone();
                if settings.revision != expected_revision {
                    return Err("settings_conflict");
                }
                if enabled
                    && !self
                        .states
                        .read()
                        .await
                        .values()
                        .any(|s| s.binding.telegram_destination_ref.is_some())
                {
                    return Err("telegram_binding_required");
                }
                settings.telegram.enabled = enabled;
                self.save_control_settings(expected_revision, settings)
                    .await
            }
            CodexCommandV1::ReadDirectory { endpoint_id, path } => {
                let client = self.endpoint(&endpoint_id).await?;
                let response = client.request("fs/readDirectory", json!({ "path": path })).await.map_err(RpcFailure::code)?;
                let response: codex_app_server_protocol::FsReadDirectoryResponse = serde_json::from_value(response).map_err(|_| "invalid_directory_response")?;
                let mut paths = response.entries.into_iter().filter(|e| e.is_directory && !e.file_name.chars().any(char::is_control))
                    .map(|e| std::path::Path::new(&path).join(e.file_name).to_string_lossy().into_owned()).collect::<Vec<_>>();
                if paths.len() > 4096 || paths.iter().map(String::len).sum::<usize>() > 128 * 1024 {
                    return Err("directory_too_large");
                }
                paths.sort();
                Ok(CodexResultV1::Directories { paths })
            }
            CodexCommandV1::RenameThread { endpoint_id, thread_id, name } => {
                if !may_configure { return Err("scope_denied"); }
                let client = self.endpoint(&endpoint_id).await?;
                client.request("thread/name/set", json!({"threadId": thread_id, "name": name})).await.map_err(RpcFailure::code)?;
                let mut states = self.states.write().await;
                let mut binding = None;
                for state in states.values_mut().filter(|s| s.binding.endpoint_id == endpoint_id && s.binding.thread_id == thread_id) {
                    state.thread_name = Some(name.clone());
                    binding = Some(state.clone());
                }
                drop(states);
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
                Ok(CodexResultV1::Binding { binding })
            }
            CodexCommandV1::StartThread { endpoint_id, name, working_directory, yolo } => {
                if !may_configure { return Err("scope_denied"); }
                let topology=self.settings.load_message_connectors().map_err(|_|"settings_unavailable")?;
                if topology.disabled_monitors.contains(&endpoint_id) { return Err("monitor_disabled"); }
                let client=self.endpoint(&endpoint_id).await?;
                let mut params=json!({"cwd":working_directory,"ephemeral":false});
                if yolo {
                    params["approvalPolicy"]=serde_json::to_value(codex_app_server_protocol::AskForApproval::Never).map_err(|_|"invalid_codex_request")?;
                    params["sandbox"]=serde_json::to_value(codex_app_server_protocol::SandboxMode::DangerFullAccess).map_err(|_|"invalid_codex_request")?;
                }
                let value=client.request("thread/start",params).await.map_err(RpcFailure::code)?;
                let thread=value.pointer("/thread/id").and_then(Value::as_str).ok_or("invalid_thread_response")?;
                let id=self.observe_loaded_thread(&endpoint_id,thread,None).await?;
                self.persist_binding(&id)?;
                // Naming persists the original empty Thread through app-server's
                // own store, so its terminal can resume without a synthetic turn.
                // Keep the binding before preparation so failures remain discoverable.
                client.request("thread/name/set", json!({
                    "threadId":thread,"name":name.as_deref().unwrap_or("Codex Session")
                })).await.map_err(RpcFailure::code)?;
                self.join(&id,false).await?;
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
                Ok(CodexResultV1::Binding { binding:self.states.read().await.get(&id).cloned() })
            }
            CodexCommandV1::OpenThread { endpoint_id, thread_id } => {
                if !may_configure { return Err("scope_denied"); }
                let topology = self.settings.load_message_connectors().map_err(|_| "settings_unavailable")?;
                if topology.disabled_monitors.contains(&endpoint_id) { return Err("monitor_disabled"); }
                // Observe first only after the original app server proves the exact ID.
                // A failed thread/resume must never manufacture a fresh Thread.
                let client = self.endpoint(&endpoint_id).await?;
                let value = client.request("thread/resume", json!({"threadId":thread_id,"excludeTurns":true})).await.map_err(RpcFailure::code)?;
                if value.pointer("/thread/id").and_then(Value::as_str) != Some(thread_id.as_str()) { return Err("thread_mismatch"); }
                self.enable_original_thread(&endpoint_id, &thread_id).await?;
                let id = self.observe_loaded_thread(&endpoint_id, &thread_id, None).await?;
                // The verified resume reply is the join evidence. Do not issue a
                // second resume (a second writer operation) for the same Thread.
                self.finish_join(&id, &client, value).await?;
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
                Ok(CodexResultV1::Binding { binding:self.states.read().await.get(&id).cloned() })
            }
            CodexCommandV1::ListLoaded { endpoint_id } => {
                let client = self.endpoint(&endpoint_id).await?;
                let ids = loaded(&client).await?;
                let mut threads = Vec::new();
                for thread_id in ids {
                    let value = client
                        .request(
                            "thread/read",
                            json!({"threadId":thread_id,"includeTurns":false}),
                        )
                        .await
                        .map_err(RpcFailure::code)?;
                    let display_name = value
                        .pointer("/thread/name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&thread_id)
                        .chars()
                        .take(120)
                        .collect();
                    threads.push(CodexLoadedThreadV1 {
                        thread_id,
                        display_name,
                    });
                }
                Ok(CodexResultV1::Loaded { threads })
            }
            CodexCommandV1::Bind {
                session,
                endpoint_id,
                thread_id,
            } => {
                if !self
                    .sessions
                    .snapshot()
                    .await
                    .sessions
                    .iter()
                    .any(|s| s.key == session)
                {
                    return Err("session_not_found");
                }
                let config = self.config.read().await;
                let endpoint = config
                    .endpoints
                    .iter()
                    .find(|e| e.id == endpoint_id)
                    .ok_or("endpoint_not_found")?;
                if endpoint.execution_target_id != session.execution_target.as_str() {
                    return Err("target_mismatch");
                }
                drop(config);
                let client = self.endpoint(&endpoint_id).await?;
                if !loaded(&client).await?.contains(&thread_id) {
                    return Err("thread_not_loaded");
                }
                let id = self
                    .observe_loaded_thread(&endpoint_id, &thread_id, None)
                    .await?;
                let revision = self.persist_binding(&id)?.revision;
                if !self
                    .ledger
                    .associate_session(
                        id.clone(),
                        revision,
                        serde_json::to_string(&Some(session)).map_err(|_| "invalid_session")?,
                    )
                    .map_err(|_| "ledger_unavailable")?
                {
                    return Err("binding_conflict");
                }
                self.reload_association(&id).await?;
                self.join(&id, false).await?;
                Ok(CodexResultV1::Binding {
                    binding: self.states.read().await.get(&id).cloned(),
                })
            }
            CodexCommandV1::Resume {
                binding_id,
                expected_revision,
            } => {
                let row = self.binding_row(&binding_id)?;
                self.states.write().await.entry(binding_id.clone()).or_insert(disconnected(binding_from_row(row)?));
                self.current_binding(&binding_id, expected_revision).await?;
                self.join(&binding_id, true).await?;
                Ok(CodexResultV1::Binding {
                    binding: self.states.read().await.get(&binding_id).cloned(),
                })
            }
            CodexCommandV1::ReleaseThread { binding_id, expected_revision, turn_id } => {
                if !may_configure { return Err("scope_denied"); }
                let state = self.current_binding(&binding_id, expected_revision).await?;
                if state.connection != CodexConnectionStateV1::Connected || state.turn_id != turn_id {
                    return Err("codex_task_changed");
                }
                if !matches!(state.activity, CodexActivityV1::Idle | CodexActivityV1::Completed | CodexActivityV1::Interrupted | CodexActivityV1::Failed) {
                    return Err("codex_task_active");
                }
                let client = self.endpoint(&state.binding.endpoint_id).await?;
                let reply = client.request("thread/unsubscribe", json!({"threadId":state.binding.thread_id})).await.map_err(RpcFailure::code)?;
                if !matches!(reply["status"].as_str(), Some("unsubscribed" | "notLoaded" | "notSubscribed")) {
                    return Err("unsubscribe_unconfirmed");
                }
                // This existing durable flag suppresses recovery/discovery after
                // explicit exit. It does not archive or delete the original.
                for mut row in self.ledger.bindings().map_err(|_| "ledger_unavailable")? {
                    if row.owner_host_id != state.binding.owner_host_id || row.endpoint_id != state.binding.endpoint_id || row.thread_id != state.binding.thread_id || !row.enabled { continue; }
                    let previous = row.revision;
                    row.enabled = false;
                    row.revision = row.revision.checked_add(1).ok_or("revision_exhausted")?;
                    if !self.ledger.save_binding(previous, row).map_err(|_| "ledger_unavailable")? { return Err("binding_conflict"); }
                }
                self.states.write().await.retain(|_, s| s.binding.owner_host_id != state.binding.owner_host_id
                    || s.binding.endpoint_id != state.binding.endpoint_id || s.binding.thread_id != state.binding.thread_id);
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
                if loaded(&client).await?.contains(&state.binding.thread_id) { return Err("thread_release_pending"); }
                Ok(CodexResultV1::Binding { binding: None })
            }
            CodexCommandV1::Unbind {
                binding_id,
                expected_revision,
            } => {
                self.current_binding(&binding_id, expected_revision).await?;
                if !self
                    .ledger
                    .associate_session(binding_id.clone(), expected_revision as i64, "null".into())
                    .map_err(|_| "ledger_unavailable")?
                {
                    return Err("binding_conflict");
                }
                self.reload_association(&binding_id).await?;
                Ok(CodexResultV1::Binding { binding: None })
            }
            CodexCommandV1::UpdateNotifications {
                binding_id,
                expected_revision,
                telegram_destination_ref,
                imessage_enabled,
            } => {
                self.current_binding(&binding_id, expected_revision).await?;
                let config = self.config.read().await;
                if telegram_destination_ref
                    .as_ref()
                    .is_some_and(|id| !config.telegram.destinations.iter().any(|d| &d.id == id))
                {
                    return Err("destination_not_found");
                }
                if imessage_enabled && config.imessage_sender.is_none() {
                    return Err("sender_not_configured");
                }
                drop(config);
                let mut row = self.binding_row(&binding_id)?;
                row.revision += 1;
                row.telegram_destination_ref = telegram_destination_ref;
                row.imessage_enabled = imessage_enabled;
                if !self
                    .ledger
                    .save_binding(expected_revision as i64, row.clone())
                    .map_err(|_| "ledger_unavailable")?
                {
                    return Err("binding_conflict");
                }
                let mut states = self.states.write().await;
                let state = states.get_mut(&binding_id).ok_or("binding_not_found")?;
                state.binding = binding_from_row(row)?;
                Ok(CodexResultV1::Binding {
                    binding: Some(state.clone()),
                })
            }
            command @ (CodexCommandV1::SubmitThreadText { .. }
            | CodexCommandV1::QueueText { .. }
            | CodexCommandV1::Steer { .. }
            | CodexCommandV1::Interrupt { .. }) => self.submit(principal, command).await,
            _ => Err("invalid_request"),
        };
        self.synchronize_session_attention().await;
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        result
    }

    pub(super) async fn save_control_settings(
        &self,
        expected_revision: u64,
        settings: CodexControlSettingsV1,
    ) -> ControlResult<CodexResultV1> {
        // Endpoint identity changes are explicit rebinds. A config edit
        // cannot silently redirect already-authorized notifications.
        let old = self.config.read().await.clone();
        for state in self.states.read().await.values() {
            let id = &state.binding.endpoint_id;
            if old.endpoints.iter().find(|e| &e.id == id)
                != settings.endpoints.iter().find(|e| &e.id == id)
            {
                return Err("unbind_before_endpoint_change");
            }
        }
        let saved = self
            .settings
            .update_codex_control(expected_revision, settings)
            .map_err(|_| "settings_conflict")?;
        *self.config.write().await = saved.clone();
        self.publisher.configuration(super::application_state::ConfigurationKind::Codex, saved.revision);
        self.peers
            .lock()
            .await
            .retain(|_, peer| saved.enabled && saved.host_connections.contains(&peer.connection));
        {
            let mut states = self.states.write().await;
            for (id, state) in states.iter_mut() {
                if let Some(binding) = self.ledger.binding(id).map_err(|_| "ledger_unavailable")? {
                    if binding.enabled && binding.owner_host_id == self.owner {
                        state.binding = binding_from_row(binding)?;
                    }
                }
            }
        }
        self.endpoints.lock().await.retain(|id, _| {
            old.endpoints.iter().find(|e| &e.id == id)
                == saved.endpoints.iter().find(|e| &e.id == id)
        });
        if !saved.enabled {
            self.endpoints.lock().await.clear();
            for state in self.states.write().await.values_mut() {
                mark_disconnected(state, "control_disabled");
            }
        }
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        self.publisher.publish(ApplicationStateApiV1::ControlCenterV1);
        Ok(CodexResultV1::Settings { settings: saved })
    }

    pub(super) fn binding_row(&self, id: &str) -> ControlResult<CodexBindingRow> {
        self.ledger
            .binding(id)
            .map_err(|_| "ledger_unavailable")?
            .or_else(|| self.observations.lock().unwrap().get(id).map(|o| o.row.clone()))
            .filter(|b| b.enabled && b.owner_host_id == self.owner)
            .ok_or("binding_not_found")
    }

    pub(super) fn persist_binding(&self, id: &str) -> ControlResult<CodexBindingRow> {
        let row = self.binding_row(id)?;
        if self.ledger.binding(id).map_err(|_| "ledger_unavailable")?.is_none()
            && !self.ledger.save_binding(0, row.clone()).map_err(|_| "ledger_unavailable")? {
            return Err("binding_conflict");
        }
        self.observations.lock().unwrap().remove(id);
        Ok(row)
    }

    pub(super) async fn current_binding(
        &self,
        id: &str,
        revision: u64,
    ) -> ControlResult<CodexStateV1> {
        let state = self
            .states
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or("binding_not_found")?;
        if state.binding.revision != revision || self.persist_binding(id)?.revision != revision as i64 {
            return Err("binding_conflict");
        }
        Ok(state)
    }

    pub(super) async fn endpoint(&self, id: &str) -> ControlResult<Arc<CodexEndpoint>> {
        let config = self.config.read().await;
        if !config.enabled {
            return Err("control_disabled");
        }
        let endpoint = config
            .endpoints
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or("endpoint_not_found")?;
        drop(config);
        if endpoint.owner_host_id != self.owner {
            return Err("owner_mismatch");
        }
        if endpoint.control_mode == CodexControlModeV1::RemoteHost {
            return Err("endpoint_mode_unavailable");
        }
        let mut endpoints = self.endpoints.lock().await;
        if let Some(connection) = endpoints.get(id).filter(|e| e.client.connected()) {
            return Ok(connection.client.clone());
        }
        endpoints.remove(id);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let forward = if endpoint.control_mode == CodexControlModeV1::SshDirect {
            Some(Arc::new(
                super::ssh_socket_forward::SshSocketForward::connect(
                    self.network.as_ref().ok_or("ssh_unavailable")?,
                    &endpoint.execution_target_id,
                    &endpoint.socket_path,
                )
                .await?,
            ))
        } else {
            None
        };
        let socket = forward
            .as_ref()
            .map(|f| f.socket_path().to_string_lossy().into_owned())
            .unwrap_or(endpoint.socket_path);
        let client = Arc::new(
            CodexEndpoint::connect(id.into(), generation, &socket, self.events.clone(), forward)
                .await
                .map_err(RpcFailure::code)?,
        );
        endpoints.insert(
            id.into(),
            EndpointConnection {
                generation,
                client: client.clone(),
            },
        );
        Ok(client)
    }

    pub(super) async fn join(&self, id: &str, allow_resume: bool) -> ControlResult<()> {
        if !self.binding_row(id)?.enabled { return Err("thread_released"); }
        let binding = self
            .states
            .read()
            .await
            .get(id)
            .ok_or("binding_not_found")?
            .binding
            .clone();
        let client = self.endpoint(&binding.endpoint_id).await?;
        if !allow_resume && !loaded(&client).await?.contains(&binding.thread_id) {
            if let Some(state) = self.states.write().await.get_mut(id) {
                mark_disconnected(state, "thread_not_loaded");
                state.connection = CodexConnectionStateV1::Stale;
                permissions(state);
            }
            return Err("thread_not_loaded");
        }
        self.join_loaded(id, &client).await
    }

    pub(super) async fn join_loaded(&self, id: &str, client: &CodexEndpoint) -> ControlResult<()> {
        let binding = self.binding_row(id)?;
        let value = Self::resume_original_thread(client, &binding.thread_id).await?;
        self.finish_join(id, client, value).await
    }

    pub(super) async fn resume_original_thread(client: &CodexEndpoint, thread: &str) -> ControlResult<Value> {
        let value = client.request("thread/resume", json!({"threadId":thread,"excludeTurns":true})).await.map_err(RpcFailure::code)?;
        if value.pointer("/thread/id").and_then(Value::as_str) != Some(thread) { return Err("thread_mismatch"); }
        Ok(value)
    }

    async fn finish_join(&self, id: &str, client: &CodexEndpoint, value: Value) -> ControlResult<()> {
        let mut state = self
            .states
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or("binding_not_found")?;
        if value.pointer("/thread/id").and_then(Value::as_str) != Some(state.binding.thread_id.as_str()) {
            return Err("thread_mismatch");
        }
        super::codex_discovery::apply_thread_metadata(&mut state, &value["thread"]);
        state.thread_name = super::codex_discovery::thread_name(&value["thread"]["name"]);
        state.connection = CodexConnectionStateV1::Connected;
        state.error_code = None;
        self.reconcile_history(&mut state, client).await?;
        // Approval flags are more precise than the turn's inProgress status.
        apply_status(&mut state, &value["thread"]["status"]);
        permissions(&mut state);
        self.states.write().await.insert(id.into(), state);
        // Previously received input may have been waiting for this original
        // thread to become usable. Wake its durable workers, not UI observers.
        self.publisher.work(super::application_state::DeliveryChannel::Telegram);
        self.publisher.work(super::application_state::DeliveryChannel::Connector);
        Ok(())
    }

    async fn needs_recovery(&self) -> bool {
        let config = self.config.read().await;
        if !config.enabled { return false; }
        let disabled = self.topology.read().await.disabled_monitors.clone();
        let endpoints = self.endpoints.lock().await.iter().map(|(id,c)| (id.clone(),(c.client.connected(), c.generation))).collect::<BTreeMap<_,_>>();
        let inventories = self.inventories.lock().await;
        config.endpoints.iter().filter(|e| e.owner_host_id == self.owner && e.control_mode != CodexControlModeV1::RemoteHost && !disabled.contains(&e.id) && !e.id.starts_with("auto-codex-"))
            .any(|e| endpoints.get(&e.id).is_none_or(|c| !c.0 || inventories.get(&e.id) != Some(&c.1)))
            || self.states.read().await.values().any(|s| s.connection == CodexConnectionStateV1::Disconnected && !disabled.contains(&s.binding.endpoint_id))
    }

    async fn reconnect(&self) {
        // Configuration events must not be lost behind an in-flight mutation.
        let _operation = self.operations.lock().await;
        if !self.config.read().await.enabled {
            return;
        }
        let Ok(topology) = self.settings.load_message_connectors() else {
            return;
        };
        *self.topology.write().await = topology.clone();
        self.endpoints
            .lock()
            .await
            .retain(|id, _| !topology.disabled_monitors.contains(id));
        self.inventories
            .lock()
            .await
            .retain(|id, _| !topology.disabled_monitors.contains(id));
        self.discover_account_endpoints().await;
        self.discover_with_topology(&topology).await;
        let clients = self
            .endpoints
            .lock()
            .await
            .iter()
            .map(|(id, c)| (id.clone(), c.client.connected()))
            .collect::<BTreeMap<_, _>>();
        let mut reconnect = Vec::new();
        let mut changed = false;
        {
            let mut states = self.states.write().await;
            for (id, state) in states.iter_mut() {
                let previous = state.clone();
                if topology
                    .disabled_monitors
                    .contains(&state.binding.endpoint_id)
                {
                    mark_disconnected(state, "monitor_disabled");
                    changed |= *state != previous;
                    continue;
                }
                if clients.get(&state.binding.endpoint_id) != Some(&true) {
                    mark_disconnected(state, "codex_unavailable");
                }
                changed |= *state != previous;
                if state.connection == CodexConnectionStateV1::Disconnected {
                    reconnect.push(id.clone());
                }
            }
        }
        for id in reconnect {
            let previous = self.states.read().await.get(&id).cloned();
            if let Err(code) = self.join(&id, false).await {
                if let Some(state) = self.states.write().await.get_mut(&id) {
                    state.error_code = Some(code.into());
                }
            }
            changed |= self.states.read().await.get(&id) != previous.as_ref();
        }
        if changed {
            self.synchronize_session_attention().await;
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
        }
    }

    async fn observe(&self, event: EndpointEvent) -> bool {
        // This Host consumes lifecycle and final-answer facts, not token output.
        // Classify using the upstream typed API before serialization, settings I/O
        // or the admission lock; relevant events still pass all fences below.
        if !super::codex_endpoint::relevant_event(&event.event) { return false; }
        // Completion admission observes the same binding/configuration boundary
        // as submission and rebind. RPC responses use the independent client lane.
        let _operation = self.operations.lock().await;
        if self
            .endpoints
            .lock()
            .await
            .get(&event.endpoint_id)
            .is_none_or(|e| e.generation != event.generation)
        {
            return false;
        }
        // Repeated connected status is a no-op, including permissions. Check
        // after the generation/operation fences but before settings I/O. Changed
        // facts and completion admission still validate current durable settings.
        if let AppServerEvent::ServerNotification(notification) = &event.event {
            if let codex_app_server_protocol::ServerNotification::ThreadStatusChanged(status) = notification.as_ref() {
                if let Ok(value) = serde_json::to_value(&status.status) {
                    let states = self.states.read().await;
                    if states.values().filter(|state| state.binding.endpoint_id == event.endpoint_id && state.binding.thread_id == status.thread_id).all(|state| {
                        if state.connection != CodexConnectionStateV1::Connected { return false; }
                        let mut next = state.clone();
                        apply_status(&mut next, &value);
                        permissions(&mut next);
                        *state == next
                    }) { return false; }
                }
            }
        }
        if self
            .settings
            .load_message_connectors()
            .map_or(true, |t| t.disabled_monitors.contains(&event.endpoint_id))
        {
            return false;
        }
        let value = match event.event {
            AppServerEvent::ServerNotification(n) => serde_json::to_value(n).ok(),
            AppServerEvent::ServerRequest(r) => serde_json::to_value(r).ok(),
            AppServerEvent::Disconnected { .. } | AppServerEvent::Lagged { .. } => {
                self.endpoints.lock().await.remove(&event.endpoint_id);
                for state in self
                    .states
                    .write()
                    .await
                    .values_mut()
                    .filter(|s| s.binding.endpoint_id == event.endpoint_id)
                {
                    mark_disconnected(state, "codex_unavailable");
                }
                self.synchronize_session_attention().await;
                self.publisher.publish(ApplicationStateApiV1::CodexV1);
                return true;
            }
        };
        let Some(value) = value else { return false };
        let method = value["method"].as_str().unwrap_or_default();
        let params = &value["params"];
        if method == "thread/started" {
            self.discover_thread_event(&event.endpoint_id, &params["thread"])
                .await;
            return true;
        }
        let Some(thread_id) = params["threadId"].as_str() else {
            return false;
        };
        let mut recovery_changed = false;
        if method == "thread/status/changed" && params["status"]["type"] != "notLoaded" {
            let retry = self
                .states
                .read()
                .await
                .values()
                .find(|s| {
                    s.binding.endpoint_id == event.endpoint_id
                        && s.binding.thread_id == thread_id
                        && s.connection != CodexConnectionStateV1::Connected
                })
                .map(|s| s.binding.binding_id.clone());
            if let Some(id) = retry {
                let _ = self.join(&id, false).await;
                recovery_changed = true;
            }
        }
        let mut changed = false;
        let mut affected = std::collections::BTreeSet::new();
        let mut states = self.states.write().await;
        for state in states.values_mut().filter(|s| {
            s.binding.endpoint_id == event.endpoint_id && s.binding.thread_id == thread_id
        }) {
            let previous = state.clone();
            match method {
                "thread/name/updated" => {
                    state.thread_name = super::codex_discovery::thread_name(&params["threadName"])
                }
                "thread/status/changed" => apply_status(state, &params["status"]),
                "turn/started" | "turn/completed" => apply_turn(state, &params["turn"]),
                "item/completed" if params["turnId"].as_str() == state.turn_id.as_deref() => {
                    let item = &params["item"];
                    if item["type"] == "agentMessage"
                        && (item["phase"].is_null() || item["phase"] == "final_answer")
                    {
                        state.final_answer = item["text"].as_str().map(bounded_answer);
                    } else if item["type"] == "userMessage" && state.metadata.preview.is_none() {
                        if let Some(content) = item["content"].as_array() {
                            let preview = content.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join(" ");
                            let preview = preview.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !preview.is_empty() { state.metadata.preview = Some(preview.chars().take(4096).collect()); }
                        }
                    } else {
                        continue;
                    }
                }
                "thread/closed" => {
                    mark_disconnected(state, "thread_not_loaded");
                    state.connection = CodexConnectionStateV1::Stale;
                }
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    state.activity = CodexActivityV1::WaitingApproval
                }
                "item/tool/requestUserInput" => state.activity = CodexActivityV1::WaitingInput,
                _ => continue,
            }
            permissions(state);
            if method == "turn/completed" {
                if let Err(code) = self.record_turn_completion(state, false).await {
                    mark_disconnected(state, code);
                }
            }
            if *state != previous {
                if let Some(key) = state.binding.session.clone() { affected.insert(key); }
            }
            recovery_changed |= state.connection != previous.connection;
            changed |= *state != previous;
        }
        if method == "thread/closed" || (method == "thread/status/changed" && params["status"]["type"] == "notLoaded") {
            states.retain(|_, s| s.binding.endpoint_id != event.endpoint_id || s.binding.thread_id != thread_id || s.binding.session.is_some());
            self.observations.lock().unwrap().retain(|id, _| states.contains_key(id));
        }
        drop(states);
        if changed {
            self.synchronize_attention_for(Some(&affected)).await;
            self.publisher.publish(ApplicationStateApiV1::CodexV1);
        }
        recovery_changed
    }

    async fn synchronize_session_attention(&self) {
        self.synchronize_attention_for(None).await;
    }
    pub(super) async fn synchronize_attention_for(&self, keys: Option<&std::collections::BTreeSet<SessionKeyV1>>) {
        let projections = self
            .states
            .read()
            .await
            .values()
            .filter_map(|state| {
                let key = state.binding.session.clone()?;
                if keys.is_some_and(|keys| !keys.contains(&key)) { return None; }
                let (attention, label) = match state.activity {
                    CodexActivityV1::Idle => (SessionAttentionStateV1::Idle, "Codex is idle"),
                    CodexActivityV1::Active => {
                        (SessionAttentionStateV1::Working, "Codex is working")
                    }
                    CodexActivityV1::WaitingApproval => (
                        SessionAttentionStateV1::ApprovalRequired,
                        "Codex requires approval",
                    ),
                    CodexActivityV1::WaitingInput => (
                        SessionAttentionStateV1::DecisionRequired,
                        "Codex needs user input",
                    ),
                    CodexActivityV1::Completed => {
                        (SessionAttentionStateV1::ReviewReady, "Codex turn completed")
                    }
                    CodexActivityV1::Failed => {
                        (SessionAttentionStateV1::Failed, "Codex turn failed")
                    }
                    CodexActivityV1::Interrupted => (
                        SessionAttentionStateV1::NeedsAttention,
                        "Codex turn interrupted",
                    ),
                };
                let connected = state.connection == CodexConnectionStateV1::Connected;
                let actionable = attention.is_red() || attention.is_orange();
                let event = format!(
                    "codex-{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(
                            &state.binding.owner_host_id,
                            &state.binding.endpoint_id,
                            &state.binding.thread_id,
                            &state.turn_id,
                            state.activity,
                        ))
                        .expect("Codex attention identity")
                    )
                );
                let projection = SessionAttentionProjectionV1 {
                    execution_target_id: key.execution_target.to_string(),
                    tmux_session_id: key.tmux_session_id.clone(),
                    summary_sections: vec![label.into()],
                    state: attention,
                    freshness: if connected {
                        SessionSummaryFreshnessV1::Fresh
                    } else {
                        SessionSummaryFreshnessV1::Stale
                    },
                    confidence: SessionAttentionConfidenceV1::Direct,
                    monitor_health: if connected {
                        SessionMonitorHealthV1::Connected
                    } else {
                        SessionMonitorHealthV1::Disconnected
                    },
                    monitor_detail: Some(
                        if connected {
                            "Codex app-server"
                        } else {
                            "Codex app-server observation is stale"
                        }
                        .into(),
                    ),
                    source_event_id: Some(event),
                    attention_since_ms: actionable.then(|| now_ms().max(0) as u64),
                    unread: actionable,
                    // Codex completion admission, not a terminal hook, owns notification delivery.
                    notification_pending: false,
                    ..Default::default()
                };
                Some((key, projection))
            })
            .collect();
        let _ = self.sessions.update_protocol_attention(projections, keys).await;
    }
}

pub(super) async fn loaded(client: &CodexEndpoint) -> ControlResult<Vec<String>> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut cursors = std::collections::BTreeSet::new();
    loop {
        let value = client
            .request("thread/loaded/list", json!({"limit":128,"cursor":cursor}))
            .await
            .map_err(RpcFailure::code)?;
        let data = value["data"].as_array().ok_or("invalid_codex_response")?;
        for id in data {
            ids.push(id.as_str().ok_or("invalid_codex_response")?.to_owned());
        }
        let Some(next) = value["nextCursor"].as_str() else {
            break;
        };
        if data.is_empty() || !cursors.insert(next.to_owned()) {
            return Err("invalid_codex_response");
        }
        cursor = Some(next.to_owned());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub(super) fn binding_from_row(row: CodexBindingRow) -> ControlResult<CodexBindingV1> {
    Ok(CodexBindingV1 {
        owner_host_id: row.owner_host_id,
        binding_id: row.id,
        revision: row.revision.try_into().map_err(|_| "invalid_binding")?,
        session: serde_json::from_str(&row.session_key).map_err(|_| "invalid_binding")?,
        endpoint_id: row.endpoint_id,
        thread_id: row.thread_id,
        telegram_destination_ref: row.telegram_destination_ref,
        imessage_enabled: row.imessage_enabled,
    })
}

pub(super) fn disconnected(binding: CodexBindingV1) -> CodexStateV1 {
    let mut state = CodexStateV1 {
        metadata: Default::default(),
        thread_name: None,
        binding,
        connection: CodexConnectionStateV1::Disconnected,
        activity: CodexActivityV1::Idle,
        turn_id: None,
        final_answer: None,
        error_code: None,
        can_queue: false,
        can_steer: false,
        can_interrupt: false,
        presentation: None,
    };
    permissions(&mut state);
    state
}

fn mark_disconnected(state: &mut CodexStateV1, code: &str) {
    state.connection = CodexConnectionStateV1::Disconnected;
    state.error_code = Some(code.into());
    permissions(state);
}

pub(super) fn permissions(state: &mut CodexStateV1) {
    state.can_queue = state.connection == CodexConnectionStateV1::Connected;
    state.can_steer =
        state.can_queue && state.activity == CodexActivityV1::Active && state.turn_id.is_some();
    state.can_interrupt = state.can_queue
        && state.turn_id.is_some()
        && matches!(
            state.activity,
            CodexActivityV1::Active
                | CodexActivityV1::WaitingApproval
                | CodexActivityV1::WaitingInput
        );
    state.presentation = Some(session_presentation(state));
}

fn session_presentation(state: &CodexStateV1) -> CodexSessionPresentationV1 {
    let connection_label = match state.connection {
        CodexConnectionStateV1::Connected => "Connected to Codex",
        CodexConnectionStateV1::Connecting => "Connecting to Codex",
        CodexConnectionStateV1::Disconnected => "Codex observation is disconnected",
        CodexConnectionStateV1::Stale => "Codex thread is not loaded",
        CodexConnectionStateV1::Unsupported => "This Codex cannot be controlled",
        CodexConnectionStateV1::Unbound => "Connect Codex",
        CodexConnectionStateV1::Error => "Codex connection failed",
    };
    let task_label = match state.activity {
        CodexActivityV1::Idle => "Idle",
        CodexActivityV1::Active => "Working",
        CodexActivityV1::WaitingApproval => "Approval required in the original terminal",
        CodexActivityV1::WaitingInput => "Input required in the original terminal",
        CodexActivityV1::Completed => "Completed",
        CodexActivityV1::Failed => "Failed",
        CodexActivityV1::Interrupted => "Interrupted",
    };
    let detail = state.error_code.as_deref().map(|code| {
        match code {
            "control_disabled" => "Codex control is disabled in Settings.",
            "thread_not_loaded" => "Resume explicitly to load this original conversation.",
            "history_backlog" => "More completion history remains to be reconciled.",
            "history_gap" => "The previous completion watermark could not be found.",
            "codex_unavailable" => "The last observed task state is retained; no input is resent.",
            _ => "The Codex operation could not be completed. Refresh to inspect its state.",
        }
        .into()
    });
    let action = |id: &str, label: &str, enabled, reason: &str, role| ControlActionProjectionV1 {
        id: id.into(),
        label: label.into(),
        role,
        enabled,
        selected: false,
        disabled_reason: (!enabled).then(|| reason.into()),
        confirmation: None,
        accessibility_label: label.into(),
    };
    let mut interrupt = action(
        "interrupt",
        "Interrupt task",
        state.can_interrupt,
        "No connected active turn is available.",
        ControlActionRoleV1::Destructive,
    );
    if state.can_interrupt {
        interrupt.confirmation = Some(ControlConfirmationV1 {
            id: format!(
                "interrupt-{:x}",
                Sha256::digest(state.turn_id.as_deref().unwrap_or_default())
            ),
            title: "Interrupt this Codex task?".into(),
            message: format!(
                "Interrupt turn {} in the original conversation?",
                state.turn_id.as_deref().unwrap_or_default()
            ),
            confirm_label: "Interrupt task".into(),
            destructive: true,
        });
    }
    CodexSessionPresentationV1 {
        connection_label: connection_label.into(),
        task_label: task_label.into(),
        input_label: "Send original text to Codex".into(),
        detail,
        actions: vec![
            action(
                "queue",
                "Queue input",
                state.can_queue,
                "Connect to the original Codex conversation first.",
                ControlActionRoleV1::Primary,
            ),
            action(
                "steer",
                "Steer current task",
                state.can_steer,
                "No connected running turn is available.",
                ControlActionRoleV1::Secondary,
            ),
            interrupt,
            action(
                "resume",
                "Resume original conversation",
                state.connection == CodexConnectionStateV1::Stale
                    && state.error_code.as_deref() == Some("thread_not_loaded"),
                "This conversation does not need explicit loading.",
                ControlActionRoleV1::Recovery,
            ),
            action("unbind", "Unbind", true, "", ControlActionRoleV1::Secondary),
        ],
    }
}

pub(super) fn apply_status(state: &mut CodexStateV1, value: &Value) {
    match value["type"].as_str() {
        Some("active") => {
            let flags = value["activeFlags"].as_array();
            state.activity = if flags.is_some_and(|f| f.contains(&json!("waitingOnApproval"))) {
                CodexActivityV1::WaitingApproval
            } else if flags.is_some_and(|f| f.contains(&json!("waitingOnUserInput"))) {
                CodexActivityV1::WaitingInput
            } else {
                CodexActivityV1::Active
            };
        }
        Some("notLoaded") => {
            mark_disconnected(state, "thread_not_loaded");
            state.connection = CodexConnectionStateV1::Stale;
        }
        Some("systemError") => {
            state.activity = CodexActivityV1::Failed;
            state.error_code = Some("codex_system_error".into());
        }
        Some("idle")
            if !matches!(
                state.activity,
                CodexActivityV1::Completed | CodexActivityV1::Failed | CodexActivityV1::Interrupted
            ) =>
        {
            state.activity = CodexActivityV1::Idle
        }
        _ => {}
    }
}

pub(super) fn apply_turn(state: &mut CodexStateV1, turn: &Value) {
    if let Some(time) = turn["completedAt"].as_i64().or_else(|| turn["startedAt"].as_i64()) {
        state.metadata.updated_at = Some(state.metadata.updated_at.unwrap_or(0).max(time));
    }
    if let Some(id) = turn["id"].as_str() {
        if state.turn_id.as_deref() != Some(id) {
            state.final_answer = None;
        }
        state.turn_id = Some(id.into());
    }
    state.activity = match turn["status"].as_str() {
        Some("inProgress") => CodexActivityV1::Active,
        Some("completed") => CodexActivityV1::Completed,
        Some("failed") => CodexActivityV1::Failed,
        Some("interrupted") => CodexActivityV1::Interrupted,
        _ => state.activity,
    };
    if let Some(items) = turn["items"].as_array() {
        let answer = items
            .iter()
            .rev()
            .find(|i| i["type"] == "agentMessage" && i["phase"] == "final_answer")
            .or_else(|| {
                items
                    .iter()
                    .rev()
                    .find(|i| i["type"] == "agentMessage" && i["phase"].is_null())
            });
        if let Some(text) = answer.and_then(|i| i["text"].as_str()) {
            state.final_answer = Some(bounded_answer(text));
        }
    }
}

fn bounded_answer(text: &str) -> String {
    super::codex_notifications::bounded_text(text, 3500)
}

fn valid_response(mut response: CodexResponseEnvelopeV1) -> CodexResponseEnvelopeV1 {
    // Preserve the request's identity and the existing protocol limit. A
    // single oversized result must not reach the writer as an invalid frame
    // and tear down every unrelated workspace request on this connection.
    if response.validate().is_err() {
        response.result = CodexResultV1::Error {
            code: "response_too_large".into(),
        };
    }
    response
}


