//! Headless access to the same Host registries, target gates and mutation cache.
use super::*;
use polar_station_session::{
    Catalog, DeliveryStatus, ListRequest, PaneView, Receipt, ReceiptRequest, SendRequest,
    SessionBackend, SessionReference, SessionRuntime, SessionView, TargetView,
};
use sha2::{Digest, Sha256};

fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn pane_fingerprint(pane: &polar_tmux_protocol::TmuxPane, generation: &str) -> String {
    fingerprint(&format!("{}:{}:{}", pane.id, pane.pid.unwrap_or(0), generation))
}
struct Backend {
    state: AppState,
}
#[cfg(unix)]
pub(super) async fn local_request(
    state: &AppState,
    uid: u32,
    request: polar_local_access_contract::LocalExtensionRequestV1,
) -> polar_local_access_contract::LocalExtensionResponseV1 {
    use polar_local_access_contract::{LocalExtensionResponseV1, LocalExtensionResultV1};
    let result = async {
        anyhow::ensure!(request.version == 1, "unsupported session control version");
        let input: crate::session_client::Request = serde_json::from_value(request.payload)?;
        let runtime = SessionRuntime::default();
        let backend: Arc<dyn SessionBackend> = Arc::new(Backend {
            state: state.clone(),
        });
        runtime.bind(&backend)?;
        Ok::<_, anyhow::Error>(match input {
            crate::session_client::Request::List { input } => {
                serde_json::to_value(runtime.list(input).await?)?
            }
            crate::session_client::Request::Send { principal, input } => serde_json::to_value(
                runtime
                    .send(&format!("session-peer:{uid}:{principal}"), input)
                    .await?,
            )?,
            crate::session_client::Request::Receipt { principal, input } => serde_json::to_value(
                runtime
                    .receipt(&format!("session-peer:{uid}:{principal}"), input)
                    .await?,
            )?,
        })
    }
    .await;
    LocalExtensionResponseV1 {
        extension: request.extension,
        version: request.version,
        request_id: request.request_id,
        result: match result {
            Ok(payload) => LocalExtensionResultV1::Success { payload },
            Err(_) => LocalExtensionResultV1::Rejected {
                error: polar_local_access_contract::ProtocolError::new(
                    "session_request_unconfirmed",
                    "The selected Session operation could not be confirmed.",
                    false,
                ),
            },
        },
    }
}
pub(super) fn mount(
    state: &AppState,
    runtime: Option<&Arc<SessionRuntime>>,
) -> Result<Option<Arc<dyn SessionBackend>>> {
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    let backend: Arc<dyn SessionBackend> = Arc::new(Backend {
        state: state.clone(),
    });
    runtime.bind(&backend)?;
    Ok(Some(backend))
}
fn receipt(request: ReceiptRequest, state: DeliveryStatus, error: Option<String>) -> Receipt {
    Receipt {
        intent_id: request.intent_id,
        reference: request.reference,
        state,
        error,
    }
}
fn result_receipt(request: ReceiptRequest, result: &HostResult) -> Receipt {
    let (state, error) = match result {
        HostResult::Accepted => (DeliveryStatus::Delivered, None),
        HostResult::Error { error } if error.code == "input_target_changed_before_write" => {
            (DeliveryStatus::FailedBeforeWrite, Some(error.code.clone()))
        }
        HostResult::Error { error } => (DeliveryStatus::DeliveryUnknown, Some(error.code.clone())),
        _ => (
            DeliveryStatus::DeliveryUnknown,
            Some("unconfirmed_terminal_input".into()),
        ),
    };
    receipt(request, state, error)
}
#[async_trait::async_trait]
impl SessionBackend for Backend {
    async fn list(&self, request: ListRequest) -> Result<Catalog> {
        let state = &self.state;
        let feed = state.sessions.snapshot().await;
        let managed = state.session_lifecycle.snapshot().await;
        #[cfg(feature = "codex-control")]
        let agents = if let Some(codex) = &state.codex {
            codex.session_catalog(&mut []).await
        } else { Vec::new() };
        #[cfg(not(feature = "codex-control"))]
        let agents: Vec<polar_tmux_protocol::AgentSessionCatalogRowV1> = Vec::new();
        let mut catalog = Catalog {
            source_host_id: state.host.host_id.clone(),
            source_host_name: state.host.host_name.clone(),
            host_epoch: feed.epoch.clone(),
            targets: vec![],
            sessions: vec![],
        };
        for desc in state.targets.descriptors().into_iter().filter(|d| {
            request
                .target_id
                .as_ref()
                .is_none_or(|id| d.id.as_str() == id)
        }) {
            let target = state
                .targets
                .get(desc.id.as_str())
                .context("registered target disappeared")?;
            let gate = target.operation_gate();
            let _lock = gate.read().await;
            let mut view = TargetView {
                target_id: desc.id.to_string(),
                label: desc.display_name,
                available: true,
                error: None,
            };
            let observed = async {
                refresh_target_revision(state, &target)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.code))?;
                let client = target.managed_client(&state.ssh_connections).await?;
                let observation = client.observe_sessions().await?;
                let Some(server) = observation.generation else {
                    return Ok::<_, anyhow::Error>(());
                };
                let generations = client.pane_input_generations().await?;
                for (session, marker) in observation.sessions {
                    let panes = client.list_panes(&session.id).await?;
                    let mut mapped = vec![];
                    for pane in panes {
                        let Some(created) = session.created_unix_seconds else {
                            continue;
                        };
                        mapped.push(PaneView {
                            active: pane.active,
                            reference: SessionReference {
                                source_host_id: catalog.source_host_id.clone(),
                                host_epoch: catalog.host_epoch.clone(),
                                target_id: desc.id.to_string(),
                                target_revision: target.configuration_revision(),
                                server_generation: fingerprint(&server),
                                session_id: session.id.clone(),
                                session_created: created,
                                pane_id: pane.id.clone(),
                                pane_incarnation: pane_fingerprint(&pane, generations.get(&pane.id).map(String::as_str).unwrap_or("")),
                            },
                            working_directory: pane.current_path,
                            available: !pane.dead && pane.pid.is_some_and(|p| p > 0),
                        });
                    }
                    let (kind, original) =
                        polar_tmux_workspace::SessionKind::from_terminal_name(&session.name)
                            .map(|(k, id)| (k, Some(id.to_string())))
                            .unwrap_or_default();
                    let summary = feed
                        .sessions
                        .iter()
                        .find(|s| {
                            s.key.execution_target == desc.id && s.key.tmux_session_id == session.id
                        })
                        .and_then(|s| s.attention.as_ref())
                        .map(|a| a.summary_sections.clone())
                        .unwrap_or_default();
                    let agent = agents.iter().find(|a| {
                        a.terminals.iter().any(|t| t.execution_target_id == desc.id.as_str() && t.tmux_session_id == session.id)
                            || (a.execution_target_id == desc.id.as_str() && original.as_deref() == Some(a.thread_id.as_str()))
                    }).map(|a| polar_station_session::AgentContext {
                        title: a.display_name.clone(), preview: a.metadata.preview.clone(),
                        status: a.status.clone(), updated_at: a.metadata.updated_at,
                    });
                    catalog.sessions.push(SessionView {
                        agent,
                        target_id: desc.id.to_string(),
                        managed_reference: managed.sessions.iter().find(|record|
                            record.execution_target_id.as_str() == desc.id.as_str()
                                && marker.as_deref() == Some(record.id.to_string().as_str())
                        ).map(polar_tmux_workspace::ManagedSessionRecord::reference),
                        session_id: session.id,
                        name: session.name,
                        kind: serde_json::to_value(kind)?
                            .as_str()
                            .unwrap_or("terminal")
                            .into(),
                        original_agent_id: original,
                        summary,
                        panes: mapped,
                    });
                }
                Ok(())
            }
            .await;
            if observed.is_err() {
                view.available = false;
                view.error = Some("target_observation_unavailable".into());
                catalog.sessions.retain(|s| s.target_id != view.target_id);
            }
            catalog.targets.push(view);
        }
        if let Some(target) = request.target_id {
            anyhow::ensure!(
                catalog.targets.iter().any(|t| t.target_id == target),
                "unknown execution target"
            );
        }
        Ok(catalog)
    }
    async fn send(&self, principal: &str, request: SendRequest) -> Result<Receipt> {
        let query = ReceiptRequest {
            intent_id: request.intent_id.clone(),
            reference: request.reference.clone(),
        };
        if request.reference.source_host_id != self.state.host.host_id
            || request.reference.host_epoch != self.state.sessions.snapshot().await.epoch
        {
            return Ok(receipt(
                query,
                DeliveryStatus::DeliveryUnknown,
                Some("stale_host_epoch".into()),
            ));
        }
        let target = self
            .state
            .targets
            .get(&request.reference.target_id)
            .context("unknown execution target")?;
        let envelope = ClientEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.intent_id,
            execution_target: ExecutionTarget::new(&request.reference.target_id)?,
            // The typed path fences the selected binding and pane, not unrelated topology.
            expected_revision: None,
            command: ClientCommand::SendText {
                target: request.reference.pane_id.clone(),
                text: request.text,
                press_enter: request.press_enter,
            },
        };
        let state = self.state.clone();
        let principal = principal.to_owned();
        // Dropping the SDK waiter must not cancel an admitted terminal write.
        let result = tokio::spawn(async move {
            dispatch_cached_mutation(
                &state,
                &principal,
                target,
                ClientPolicy::DesktopFull,
                envelope,
                Some(request.reference),
            )
            .await
        })
        .await?;
        anyhow::ensure!(
            !matches!(&result.result,HostResult::Error{error} if error.code=="request_id_conflict"),
            "intent ID was already used for different terminal input"
        );
        Ok(result_receipt(query, &result.result))
    }
    async fn receipt(&self, principal: &str, request: ReceiptRequest) -> Result<Receipt> {
        let key = RequestCacheKey {
            principal: principal.into(),
            execution_target: request.reference.target_id.clone(),
            request_id: request.intent_id.clone(),
        };
        let entry = self
            .state
            .request_cache
            .lock()
            .await
            .entries
            .get(&key)
            .cloned();
        if let Some(entry) = entry {
            let saved = entry.completed.lock().await;
            if let Some(saved) = saved.as_ref() {
                anyhow::ensure!(
                    saved.reference.as_ref() == Some(&request.reference),
                    "receipt target identity mismatch"
                );
                return Ok(result_receipt(request, &saved.outcome.result));
            }
        }
        Ok(receipt(
            request,
            DeliveryStatus::DeliveryUnknown,
            Some("receipt_unavailable_no_retry".into()),
        ))
    }
}

pub(super) async fn guarded_input(
    state: &AppState,
    target: &RegisteredTarget,
    command: ClientCommand,
    reference: &SessionReference,
) -> DispatchOutcome {
    let result = async {
        if target.configuration_revision() != reference.target_revision
            || state.targets.get(&reference.target_id).is_none_or(|current|
                current.configuration_revision() != reference.target_revision)
        {
            return Err(ProtocolError {
                code: "input_target_changed_before_write".into(),
                message: "original target configuration changed".into(),
                retryable: false,
            });
        }
        let client = target
            .managed_client(&state.ssh_connections)
            .await
            .map_err(ssh_protocol_error)?;
        let observed = client.observe_sessions().await.map_err(protocol_error)?;
        let server = observed
            .generation
            .filter(|s| fingerprint(s) == reference.server_generation)
            .ok_or_else(|| ProtocolError {
                code: "input_target_changed_before_write".into(),
                message: "original tmux server changed".into(),
                retryable: false,
            })?;
        let (session, marker) = observed
            .sessions
            .into_iter()
            .find(|(s, _)| {
                s.id == reference.session_id
                    && s.created_unix_seconds == Some(reference.session_created)
            })
            .ok_or_else(|| ProtocolError {
                code: "input_target_changed_before_write".into(),
                message: "original Session changed".into(),
                retryable: false,
            })?;
        let generations = client.pane_input_generations().await.map_err(protocol_error)?;
        let generation = generations.get(&reference.pane_id).map(String::as_str).unwrap_or("");
        let pane = client
            .list_panes(&session.id)
            .await
            .map_err(protocol_error)?
            .into_iter()
            .find(|p| {
                p.id == reference.pane_id
                    && !p.dead
                    && pane_fingerprint(p, generation) == reference.pane_incarnation
            })
            .ok_or_else(|| ProtocolError {
                code: "input_reference_changed_no_retry".into(),
                message: "original Pane or input generation changed; no retry".into(),
                retryable: false,
            })?;
        let binding = polar_tmux_core::TmuxSessionBinding {
            session_id: session.id,
            server_generation: server,
            created_unix_seconds: reference.session_created,
            managed_marker: marker,
        };
        let ClientCommand::SendText {
            text, press_enter, ..
        } = command
        else {
            unreachable!("only the typed Session SDK mounts guarded input")
        };
        let written = client
            .send_bound_text(
                &binding,
                &pane,
                generation,
                &text,
                press_enter,
            )
            .await
            .map_err(protocol_error)?;
        if !written {
            return Err(ProtocolError {
                code: "input_reference_changed_no_retry".into(),
                message: "original input reference changed; outcome must be checked, no retry".into(),
                retryable: false,
            });
        }
        Ok::<_, ProtocolError>(HostResult::Accepted)
    }
    .await;
    DispatchOutcome {
        result: result.unwrap_or_else(|error| HostResult::Error { error }),
        revision: target.revision(),
    }
}


