use super::*;
use crate::services::session_fact_provider::PreparedSessionEnd;
use sha2::{Digest, Sha256};

impl ControlCenterService {
    pub(super) async fn session_editor(
        &self,
        request: SessionEditorRequestV1,
    ) -> Result<SessionEditorResultV1, ControlCenterErrorV1> {
        match request {
            SessionEditorRequestV1::InspectTools { execution_target_id } => {
                Ok(SessionEditorResultV1::Tools { tools:self.editor_tool_facts(&execution_target_id).await })
            }
            SessionEditorRequestV1::PrepareTools {execution_target_id,tmux,codex,codex_service,claude,delivery} => {
                #[cfg(feature="codex-control")]
                if codex_service && self.codex.get().is_none(){return Err(control_error("codex_unavailable","Codex control is unavailable."))}
                let target=self.targets.get(&execution_target_id).ok_or_else(||control_error("target_missing","Selected machine is unavailable."))?;
                let gate=target.operation_gate();let _guard=gate.read().await;
                let lease=target.resource_ssh_lease(&self.ssh_connections).await.map_err(|_|control_error("ssh_unavailable","Check the selected machine SSH connection."))?;
                let transport=if let Some(lease)=&lease {polar_tmux_core::TmuxTarget::from(lease.endpoint())}else{target.configured_target().clone()};
                #[cfg(not(feature="codex-control"))]
                if codex {return Err(control_error("codex_unsupported","This Host does not support Codex."))}
                if let Some(lease)=&lease {self.ssh_connections.validate(lease).await.map_err(|_|control_error("ssh_changed","SSH connection changed; retry."))?;}
                if delivery==polar_station_protocol::ToolSetupDeliveryV1::NativePane {
                    let script=super::super::target_tools::prepared_script(&transport,tmux,codex,codex_service,claude).await
                        .map_err(|e|control_error("tools_prepare_failed",format!("Cannot prepare tools: {e}")))?;
                    if let Some(lease)=&lease {self.ssh_connections.validate(lease).await.map_err(|_|control_error("ssh_changed","SSH connection changed; retry."))?;}
                    let setup=super::super::target_tools::prepare(&transport,&execution_target_id,&target.descriptor().display_name,&script)
                        .map_err(|e|control_error("tools_prepare_failed",format!("Cannot prepare setup Pane: {e}")))?;
                    return Ok(SessionEditorResultV1::ToolsPrepared{setup});
                }
                super::super::target_tools::launch(&transport,tmux,codex,codex_service,claude).await.map_err(|e|control_error("tools_terminal_failed",format!("Cannot open setup terminal: {e}")))?;
                Ok(SessionEditorResultV1::ToolsLaunched{execution_target_id})
            }

            SessionEditorRequestV1::SetCreationYolo { yolo } => {
                let settings = &self.connection_dependencies.get()
                    .ok_or_else(|| control_error("settings_unavailable", "Desktop settings are unavailable."))?.settings;
                let yolo = settings.set_new_codex_yolo(yolo)
                    .map_err(|_| control_error("settings_save_failed", "YOLO preference could not be saved. Try again."))?;
                Ok(SessionEditorResultV1::CreationYolo { yolo })
            }
            SessionEditorRequestV1::CheckEndpoint { execution_target_id, endpoint_id } => {
                #[cfg(feature = "codex-control")]
                {
                    self.codex.get().ok_or_else(|| control_error("codex_unavailable", "Codex is unavailable."))?
                        .check_session_endpoint(&execution_target_id, &endpoint_id).await
                }
                #[cfg(not(feature = "codex-control"))]
                {
                    let _ = (execution_target_id, endpoint_id);
                    Err(control_error("codex_unsupported", "This Host does not support Codex."))
                }
            }
            SessionEditorRequestV1::EnsureLocalCodex { endpoint_id } => {
                #[cfg(feature = "codex-control")]
                {
                    let service = self.codex.get().ok_or_else(|| {
                        control_error("codex_unavailable", "Codex is unavailable on this Host.")
                    })?;
                    match service.prepare_local_codex(&endpoint_id).await {
                        Ok(true) => Ok(SessionEditorResultV1::EndpointReady),
                        Ok(false) => Ok(SessionEditorResultV1::EndpointLoginRequired),
                        Err(code) => Err(control_error(code, match code {
                            "codex_not_installed" => "Codex is not installed on this machine.",
                            "codex_start_timeout" => "The local Codex service took too long to start.",
                            "codex_start_failed" => "The Codex CLI could not start its local service.",
                            "codex_handshake_timeout" => "The local Codex service did not finish connecting in time.",
                            "codex_handshake_failed" => "Codex started, but Polar Station could not connect to it.",
                            "codex_account_check_timeout" => "The local Codex service did not respond to the account check.",
                            "codex_account_check_failed" => "Polar Station could not check the Codex account.",
                            _ => "The local Codex service could not be prepared.",
                        })),
                    }
                }
                #[cfg(not(feature = "codex-control"))]
                {
                    let _ = endpoint_id;
                    Err(control_error("codex_unsupported", "This Host does not support Codex."))
                }
            }
            SessionEditorRequestV1::HistoryDetail { execution_target_id, endpoint_id, thread_id } => {
                #[cfg(feature = "codex-control")]
                { self.codex.get().ok_or_else(|| control_error("codex_unavailable", "Codex is unavailable."))?
                    .history_detail_entry(&execution_target_id, &endpoint_id, thread_id).await }
                #[cfg(not(feature = "codex-control"))]
                { let _ = (execution_target_id, endpoint_id, thread_id); Err(control_error("codex_unsupported", "Codex is unavailable.")) }
            }
            SessionEditorRequestV1::History { execution_target_id, endpoint_id, cursor, search_term, archived, include_subagents } => {
                #[cfg(feature = "codex-control")]
                {
                    let codex = self.codex.get().ok_or_else(|| control_error("codex_unavailable", "Codex control is unavailable."))?;
                    codex.history_entries(&execution_target_id, &endpoint_id, cursor, search_term, archived, include_subagents).await
                }
                #[cfg(not(feature = "codex-control"))]
                {
                    let _ = (execution_target_id, endpoint_id, cursor, search_term, archived, include_subagents);
                    Err(control_error("codex_unsupported", "This Host does not support Codex history."))
                }
            }
            SessionEditorRequestV1::PreviewEndAgent { execution_target_id: _, owner_host_id, endpoint_id, thread_id } => {
                let (preview, _) = self.prepare_agent_end(&owner_host_id, &endpoint_id, &thread_id).await?;
                Ok(SessionEditorResultV1::EndPreview { preview: Box::new(preview) })
            }
            SessionEditorRequestV1::PreviewEnd {
                execution_target_id,
                tmux_session_id,
            } => {
                let prepared = self
                    .prepare_end_preview(session_key(&execution_target_id, &tmux_session_id)?)
                    .await?;
                Ok(SessionEditorResultV1::EndPreview {
                    preview: Box::new(prepared.preview),
                })
            }
            SessionEditorRequestV1::LaunchContext {
                execution_target_id,
                source_session_id,
            } => {
                let directory_future=async {
                let mut notice = None;
                let directory = if let Some(source) = source_session_id {
                    let inherited: anyhow::Result<_> = async {
                        let target = self
                            .targets
                            .get(&execution_target_id)
                            .ok_or_else(|| anyhow::anyhow!("Machine unavailable"))?;
                        let client = target.managed_client(&self.ssh_connections).await?;
                        let original = self
                            .lifecycle
                            .resolve(&client, &execution_target_id, &source)
                            .await?;
                        let generation = client.server_generation().await?;
                        let panes = client.list_panes(&original).await?;
                        anyhow::ensure!(
                            client.server_generation().await? == generation,
                            "Source Session changed"
                        );
                        let path = panes
                            .iter()
                            .find(|p| p.active && !p.dead && !p.current_path.is_empty())
                            .ok_or_else(|| anyhow::anyhow!("No active directory"))?
                            .current_path
                            .clone();
                        Ok(path)
                    }
                    .await;
                    match inherited {
                        Ok(path) => match self
                            .read_directory(&execution_target_id, Some(&path), None, false)
                            .await
                        {
                            Ok(directory) => directory,
                            Err(_) => {
                                notice = Some("The current directory is unavailable; using the target account Home.".into());
                                self.read_directory(&execution_target_id, None, None, false)
                                    .await?
                            }
                        },
                        Err(_) => {
                            notice = Some("The source terminal is unavailable; using the target account Home.".into());
                            self.read_directory(&execution_target_id, None, None, false)
                                .await?
                        }
                    }
                } else {
                    self.read_directory(&execution_target_id, None, None, false)
                        .await?
                };
                Ok::<_,ControlCenterErrorV1>((directory,notice))
                };
                let (directory,tools)=tokio::join!(directory_future,self.editor_tool_facts(&execution_target_id));
                let (directory,notice)=directory?;
                let yolo = self.connection_dependencies.get()
                    .ok_or_else(|| control_error("settings_unavailable", "Desktop settings are unavailable."))?.settings
                    .load_or_initialize().map_err(|_| control_error("settings_unavailable", "Desktop settings could not be read."))?.session_creation.yolo;
                {
                    #[cfg(feature="codex-control")]
                    let codex_endpoints=if let Some(c)=self.codex.get(){Some(c.config.read().await.endpoints.clone())}else{None};
                    #[cfg(not(feature="codex-control"))]
                    let codex_endpoints=None;
                    Ok(SessionEditorResultV1::LaunchContext { directory, notice, yolo,tools:Some(tools),codex_endpoints })
                }
            }
            SessionEditorRequestV1::BrowseDirectory {
                execution_target_id,
                path,
                endpoint_id,
            } => {
                let directory = self
                    .read_directory(
                        &execution_target_id,
                        Some(&path),
                        endpoint_id.as_deref(),
                        true,
                    )
                    .await?;
                Ok(SessionEditorResultV1::Directory { directory })
            }
        }
    }

    async fn editor_tool_facts(&self, id: &str) -> Vec<polar_station_protocol::SessionTargetToolsV1> {
        #[cfg(feature = "codex-control")]
        let enabled = self.codex.get().is_some();
        #[cfg(not(feature = "codex-control"))]
        let enabled = false;
        let discover = async {
            #[cfg(feature = "codex-control")]
            if let Some(codex) = self.codex.get() {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(2),
                    codex.check_default_endpoints(id)).await;
            }
        };
        let inspect=|id:String| async move {
            let result=tokio::time::timeout(std::time::Duration::from_secs(3),async {
                let target=self.targets.get(&id)?;
                let lease=target.resource_ssh_lease(&self.ssh_connections).await.ok()?;
                let transport=if let Some(lease)=&lease{polar_tmux_core::TmuxTarget::from(lease.endpoint())}else{target.configured_target().clone()};
                let client=target.configured_client().clone().with_target(transport);
                let facts=super::super::target_tools::inspect(&client,&id,enabled).await;
                if let Some(lease)=&lease{self.ssh_connections.validate(lease).await.ok()?;}
                Some(facts)
            }).await.ok().flatten();
            result.unwrap_or(polar_station_protocol::SessionTargetToolsV1{native_setup:true,execution_target_id:id,tmux:polar_station_protocol::SessionToolStateV1::Unavailable,codex:polar_station_protocol::SessionToolStateV1::Unavailable,codex_control_enabled:enabled,default_local_codex_endpoint:None,claude:Some(polar_station_protocol::SessionToolStateV1::Unavailable)})
        };
        let inspect_required = async {
            let local = crate::target_registry::LOCAL_TARGET_ID;
            if id == local {
                vec![inspect(id.into()).await]
            } else {
                let (target, local) = tokio::join!(inspect(id.into()), inspect(local.into()));
                vec![target, local]
            }
        };
        #[allow(unused_mut)]
        let (_, mut facts) = tokio::join!(discover, inspect_required);
        #[cfg(feature = "codex-control")]
        if let Some(codex) = self.codex.get() {
            if let Some(local) = facts.iter_mut().find(|target|
                target.execution_target_id == crate::target_registry::LOCAL_TARGET_ID) {
                local.default_local_codex_endpoint = codex.local_account_endpoint_id().await;
            }
        }
        facts
    }

    async fn read_directory(
        &self,
        id: &str,
        path: Option<&str>,
        endpoint: Option<&str>,
        list: bool,
    ) -> Result<SessionDirectoryV1, ControlCenterErrorV1> {
        let target = self.targets.get(id).ok_or_else(|| {
            control_error(
                "target_missing",
                "The selected machine is no longer configured.",
            )
        })?;
        // Resource leases do not require a running tmux server.
        let lease = target
            .resource_ssh_lease(&self.ssh_connections)
            .await
            .map_err(|_| {
                control_error(
                    "ssh_unavailable",
                    "Cannot browse the selected machine. Check its SSH connection.",
                )
            })?;
        let transport = if let Some(lease) = &lease {
            polar_tmux_core::TmuxTarget::from(lease.endpoint())
        } else {
            target.configured_target().clone()
        };
        let directory =
            super::super::target_directory::read(&transport, id, path, list && endpoint.is_none())
                .await
                .map_err(|e| {
                    control_error(
                        "directory_unavailable",
                        format!("Cannot read the target directory: {e}"),
                    )
                })?;
        #[cfg(feature = "codex-control")]
        let directory = if let Some(endpoint) = endpoint {
            let mut directory = directory;
            let codex = self.codex.get().ok_or_else(|| {
                control_error("codex_unavailable", "Codex control is unavailable.")
            })?;
            directory.directories = codex
                .directory_entries(id, endpoint, &directory.path)
                .await?;
            directory
        } else {
            directory
        };
        #[cfg(not(feature = "codex-control"))]
        if endpoint.is_some() {
            return Err(control_error(
                "codex_unsupported",
                "This Host does not support Codex.",
            ));
        }
        if let Some(lease) = &lease {
            self.ssh_connections.validate(lease).await.map_err(|_| {
                control_error("ssh_changed", "The SSH connection changed; browse again.")
            })?;
        }
        super::super::target_directory::check_size(&directory)
            .map_err(|e| control_error("directory_too_large", e.to_string()))?;
        Ok(directory)
    }

    #[cfg(not(feature = "codex-control"))]
    async fn prepare_agent_end(&self, _: &str, _: &str, _: &str)
        -> Result<(SessionEndPreviewV1, Vec<PreparedSessionEnd>), ControlCenterErrorV1> {
        Err(control_error("codex_unavailable", "Codex control is unavailable."))
    }
    #[cfg(not(feature = "codex-control"))]
    pub(super) async fn end_agent_confirmed(&self, _: &str, _: &str, _: &str, _: &str, _: &str, _: &str)
        -> Result<(), ControlCenterErrorV1> {
        Err(control_error("codex_unavailable", "Codex control is unavailable."))
    }

    #[cfg(feature = "codex-control")]
    async fn prepare_agent_end(&self, owner: &str, endpoint: &str, thread: &str)
        -> Result<(SessionEndPreviewV1, Vec<PreparedSessionEnd>), ControlCenterErrorV1>
    {
        let service = self.codex.get().ok_or_else(|| control_error("codex_unavailable", "Codex control is unavailable."))?;
        let state = service.original_end_state(owner, endpoint, thread).await?;
        let mut terminals = Vec::new();
        for identity in service.original_terminals(&state).await {
            terminals.push(self.session_fact_provider.prepare_session_end(session_key(&identity.execution_target_id, &identity.tmux_session_id)?).await
                .map_err(|e| control_error(e.code, e.message))?);
        }
        let bindings: Vec<_> = terminals.iter().map(|t| (&t.preview.session, &t.binding.session_id, &t.binding.server_generation, t.binding.created_unix_seconds, &t.binding.managed_marker)).collect();
        let scope = serde_json::to_vec(&(&state.binding, &state.turn_id, bindings)).expect("typed Agent end scope");
        let name = super::super::codex_session::catalog_title(&state);
        let preview = SessionEndPreviewV1 { session: None, name: name.clone(), target_label: endpoint.into(),
            attached_clients: terminals.iter().map(|t| t.preview.attached_clients).sum(),
            confirmation: ControlConfirmationV1 {
                id: format!("end-agent-{:x}", Sha256::digest(scope)), title: "End Codex Session?".into(),
                message: format!("End “{name}”? Stop its current task, close {} associated terminal sessions, and release Station's subscription. History is kept for Resume. Other clients may keep the conversation loaded.", terminals.len()),
                confirm_label: "End Session".into(), destructive: true,
            }, codex: Some(state) };
        Ok((preview, terminals))
    }

    #[cfg(feature = "codex-control")]
    pub(super) async fn end_agent_confirmed(&self, principal: &str, request: &str, owner: &str, endpoint: &str, thread: &str, confirmation: &str)
        -> Result<(), ControlCenterErrorV1>
    {
        let (preview, terminals) = self.prepare_agent_end(owner, endpoint, thread).await?;
        if preview.confirmation.id != confirmation { return Err(control_error("confirmation_changed", "The conversation or its terminals changed. Refresh and review again.")); }
        let state = preview.codex.expect("Agent end state");
        let service = self.codex.get().expect("prepared Codex owner");
        service.stop_terminal_task(principal, &format!("end-{:x}", Sha256::digest(format!("{principal}/{request}"))), state.clone()).await?;
        for terminal in terminals {
            let identity = terminal.preview.session.expect("terminal end identity");
            self.session_fact_provider.apply_session_operation(SessionOperation::Kill {
                session:session_key(&identity.execution_target_id, &identity.tmux_session_id)?, binding:Some(terminal.binding)
            }).await.map_err(|e| control_error(e.code, e.message))?;
        }
        service.end_rpc(principal, owner, CodexCommandV1::ReleaseThread {
            binding_id:state.binding.binding_id, expected_revision:state.binding.revision, turn_id:state.turn_id,
        }).await?;
        self.state_events.publish(ApplicationStateApiV1::ControlCenterV1);
        Ok(())
    }

    pub async fn prepare_end_preview(
        &self,
        session: SessionKey,
    ) -> Result<PreparedSessionEnd, ControlCenterErrorV1> {
        let mut prepared = self
            .session_fact_provider
            .prepare_session_end(session.clone())
            .await
            .map_err(|e| control_error(e.code, e.message))?;
        #[cfg(feature = "codex-control")]
        if let Some(service) = self.codex.get() {
            prepared.preview.codex = service.terminal_end_state(&session).await?;
        }
        let codex_scope = prepared.preview.codex.as_ref().map(|s| {
            (
                &s.binding.owner_host_id,
                &s.binding.endpoint_id,
                &s.binding.thread_id,
                &s.binding.binding_id,
                s.binding.revision,
                &s.turn_id,
            )
        });
        let binding = &prepared.binding;
        let scope = serde_json::to_vec(&(
            &prepared.preview.session,
            &binding.session_id,
            &binding.server_generation,
            binding.created_unix_seconds,
            &binding.managed_marker,
            codex_scope,
        ))
        .expect("typed end scope");
        prepared.preview.confirmation.id = format!("end-{:x}", Sha256::digest(scope));
        if prepared.preview.codex.is_some() {
            prepared.preview.confirmation.message.push_str(" The current Codex task will also be interrupted; its Thread and history will be kept.");
        }
        Ok(prepared)
    }

    pub(super) async fn end_confirmed(
        &self,
        principal: &str,
        request_id: &str,
        target: String,
        reference: String,
        confirmation: String,
    ) -> Result<Option<SessionFact>, ControlCenterErrorV1> {
        let session = session_key(&target, &reference)?;
        let prepared = self.prepare_end_preview(session.clone()).await?;
        if confirmation != prepared.preview.confirmation.id {
            return Err(control_error(
                "confirmation_changed",
                "The original Session or Codex task changed. Review its current state and confirm again.",
            ));
        }
        self.lifecycle
            .mark_ending(&target, &reference)
            .await
            .map_err(|_| {
                control_error(
                    "end_intent_unavailable",
                    "The end intent could not be saved; no work was stopped.",
                )
            })?;
        self.lifecycle
            .request_refresh(session.execution_target_id.clone());
        self.state_events
            .publish(ApplicationStateApiV1::ControlCenterV1);
        #[cfg(feature = "codex-control")]
        if let Some(state) = prepared.preview.codex {
            let service = self.codex.get().expect("prepared Codex owner");
            let intent_id = format!(
                "end-{:x}",
                Sha256::digest(format!("{principal}/{request_id}"))
            );
            service
                .stop_terminal_task(principal, &intent_id, state)
                .await?;
        }
        #[cfg(not(feature = "codex-control"))]
        let _ = (principal, request_id);
        let result = self
            .session_fact_provider
            .apply_session_operation(SessionOperation::Kill {
                session,
                binding: Some(prepared.binding),
            })
            .await
            .map_err(|e| control_error(e.code, e.message));
        self.state_events
            .publish(ApplicationStateApiV1::ControlCenterV1);
        result
    }
}
