//! Read-only composition of existing Session, Inbox and original Codex facts.
use super::inbox_service::InboxService;
use polar_tmux_protocol::*;

impl InboxService {
    pub(super) async fn overview(&self, messages: &[InboxMessageProjectionV1]) -> InboxOverviewV1 {
        let mut result = InboxOverviewV1::default();
        for session in self.sessions.snapshot().await.sessions {
            // Original Codex events own these Sessions; terminal observations
            // must never create a second activity item for the same task.
            if self.sessions.protocol_owns_attention(&session.key).await {
                continue;
            }
            let Some(attention) = session.attention else {
                continue;
            };
            if attention.state == SessionAttentionStateV1::Idle {
                continue;
            }
            let (status, label) = terminal_status(attention.state);
            let stale = attention.monitor_health != SessionMonitorHealthV1::Connected;
            result.activity.push(InboxOverviewRowV1 {
                id: format!(
                    "terminal:{}:{}",
                    session.key.execution_target.as_str(),
                    session.key.tmux_session_id
                ),
                source: "terminal".into(),
                execution_target_id: session.key.execution_target.as_str().into(),
                title: session.display_name,
                task_status: status.into(),
                task_label: label.into(),
                observation_label: if stale {
                    "Observation disconnected; last known task state"
                } else {
                    "Observing terminal"
                }
                .into(),
                stale,
                summary: attention.summary_sections.join(" · "),
                created_at_ms: attention.attention_since_ms.unwrap_or_default(),
                session: Some(session.key),
                terminal_message_id: None,
                codex_owner_id: None,
                codex_binding_id: None,
                deliveries: vec![],
            });
        }
        for message in messages {
            result.records.push(InboxOverviewRowV1 {
                id: format!("terminal:{}", message.id.as_str()),
                source: "terminal".into(),
                execution_target_id: message.origin.execution_target_id.clone(),
                title: message.session_label.clone(),
                task_status: message.attention.clone(),
                task_label: message.attention.clone(),
                observation_label: "Recorded terminal observation".into(),
                stale: message.stale_context,
                summary: message.summary.clone(),
                created_at_ms: message.created_at_ms,
                session: ExecutionTarget::new(message.origin.execution_target_id.clone())
                    .ok()
                    .map(|execution_target| SessionKeyV1 {
                        execution_target,
                        tmux_session_id: message.origin.tmux_session_id.clone(),
                    }),
                terminal_message_id: Some(message.id.clone()),
                codex_owner_id: None,
                codex_binding_id: None,
                deliveries: vec![],
            });
        }
        #[cfg(feature = "codex-control")]
        if let Some(codex) = self.codex.get() {
            codex.append_inbox_overview(&mut result).await;
        }
        result.activity.sort_by(|a, b| {
            a.execution_target_id
                .cmp(&b.execution_target_id)
                .then_with(|| a.id.cmp(&b.id))
        });
        result.records.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        result
    }
}

fn terminal_status(state: SessionAttentionStateV1) -> (&'static str, &'static str) {
    match state {
        SessionAttentionStateV1::Idle => ("idle", "Idle"),
        SessionAttentionStateV1::Unknown => ("unknown", "Unknown"),
        SessionAttentionStateV1::Working => ("active", "Running"),
        SessionAttentionStateV1::ReviewReady => ("review_ready", "Ready for review"),
        SessionAttentionStateV1::NeedsAttention => ("needs_attention", "Needs attention"),
        SessionAttentionStateV1::ApprovalRequired => ("waiting_approval", "Waiting for approval"),
        SessionAttentionStateV1::DecisionRequired => ("waiting_input", "Waiting for a decision"),
        SessionAttentionStateV1::PermissionRequired => {
            ("waiting_approval", "Waiting for permission")
        }
        SessionAttentionStateV1::Failed => ("failed", "Failed"),
    }
}

#[cfg(feature = "codex-control")]
impl super::codex_control::CodexControlService {
    async fn append_inbox_overview(&self, overview: &mut InboxOverviewV1) {
        use std::collections::BTreeSet;
        let config = self.config.read().await.clone();
        let owners: BTreeSet<_> = std::iter::once(self.owner.clone())
            .chain(
                config
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.owner_host_id.clone()),
            )
            .collect();
        let reads = owners.into_iter().map(|owner| async move {
            let read = |command| {
                self.dispatch(
                    "desktop-inbox",
                    true,
                    CodexRequestEnvelopeV1 {
                        protocol_version: PROTOCOL_VERSION,
                        api: CodexApiV1::CodexV1,
                        request_id: format!("inbox-{}", crate::host_identity::random_uuid_v4()),
                        owner_host_id: owner.clone(),
                        command,
                    },
                )
            };
            let (states, records) = tokio::join!(
                read(CodexCommandV1::GetBindings),
                read(CodexCommandV1::GetCompletions {
                    endpoint_id: None,
                    before: None,
                    limit: 32,
                })
            );
            (owner, states.result, records.result)
        });
        for (owner, states, records) in futures_util::future::join_all(reads).await {
            match states {
                CodexResultV1::Bindings { bindings } => {
                    for state in bindings {
                        let stale = state.connection != CodexConnectionStateV1::Connected;
                        if state.activity == CodexActivityV1::Idle && !stale {
                            continue;
                        }
                        // Completed tasks belong to history; an observation loss
                        // remains visible independently of the last task outcome.
                        if !stale
                            && matches!(
                                state.activity,
                                CodexActivityV1::Completed | CodexActivityV1::Interrupted
                            )
                        {
                            continue;
                        }
                        let target = config
                            .endpoints
                            .iter()
                            .find(|e| e.id == state.binding.endpoint_id && e.owner_host_id == owner)
                            .map(|e| e.execution_target_id.clone())
                            .unwrap_or_else(|| {
                                state
                                    .binding
                                    .session
                                    .as_ref()
                                    .map(|s| s.execution_target.as_str().to_owned())
                                    .unwrap_or_else(|| owner.clone())
                            });
                        overview.activity.push(codex_activity(state, target));
                    }
                }
                CodexResultV1::Error { code } => overview.errors.push(format!(
                    "{owner}: activity unavailable ({code}); task state is unknown."
                )),
                _ => overview.errors.push(format!(
                    "{owner}: activity response unavailable; task state is unknown."
                )),
            }
            match records {
                CodexResultV1::Completions { completions, .. } => {
                    for item in completions {
                        let completion = item.completion;
                        let origin = completion.origin;
                        let status = match completion.status {
                            polar_station_inbox::CodexCompletionStatus::Completed => "completed",
                            polar_station_inbox::CodexCompletionStatus::Failed => "failed",
                            polar_station_inbox::CodexCompletionStatus::Interrupted => {
                                "interrupted"
                            }
                        };
                        overview.records.push(InboxOverviewRowV1 {
                            id: format!("codex:{}:{}", owner, completion.id.as_str()),
                            source: "codex".into(),
                            execution_target_id: origin.execution_target_id.clone(),
                            title: origin.thread_id,
                            task_status: status.into(),
                            task_label: status.into(),
                            observation_label: "Recorded app-server outcome".into(),
                            stale: false,
                            summary: completion.summary,
                            created_at_ms: completion.created_at_ms,
                            session: origin.tmux_session_id.and_then(|tmux_session_id| {
                                ExecutionTarget::new(origin.execution_target_id).ok().map(
                                    |execution_target| SessionKeyV1 {
                                        execution_target,
                                        tmux_session_id,
                                    },
                                )
                            }),
                            terminal_message_id: None,
                            codex_owner_id: Some(owner.clone()),
                            codex_binding_id: Some(origin.binding_id),
                            deliveries: item.deliveries,
                        });
                    }
                }
                CodexResultV1::Error { code } => overview
                    .errors
                    .push(format!("{owner}: history unavailable ({code}).")),
                _ => overview
                    .errors
                    .push(format!("{owner}: history response unavailable.")),
            }
        }
    }
}

#[cfg(feature = "codex-control")]
fn codex_activity(state: CodexStateV1, target: String) -> InboxOverviewRowV1 {
    let (status, label) = match state.activity {
        CodexActivityV1::Idle => ("idle", "Idle"),
        CodexActivityV1::Active => ("active", "Running"),
        CodexActivityV1::WaitingApproval => ("waiting_approval", "Waiting for approval"),
        CodexActivityV1::WaitingInput => ("waiting_input", "Waiting for input"),
        CodexActivityV1::Completed => ("completed", "Completed"),
        CodexActivityV1::Failed => ("failed", "Failed"),
        CodexActivityV1::Interrupted => ("interrupted", "Interrupted"),
    };
    let stale = state.connection != CodexConnectionStateV1::Connected;
    let presentation = state.presentation.as_ref();
    InboxOverviewRowV1 {
        id: format!(
            "codex:{}:{}",
            state.binding.owner_host_id, state.binding.binding_id
        ),
        source: "codex".into(),
        execution_target_id: target,
        title: state
            .thread_name
            .unwrap_or_else(|| state.binding.thread_id.clone()),
        task_status: status.into(),
        task_label: presentation
            .map(|p| p.task_label.clone())
            .unwrap_or_else(|| label.into()),
        observation_label: if stale {
            "Observation disconnected; last known task state".into()
        } else {
            presentation
                .map(|p| p.connection_label.clone())
                .unwrap_or_else(|| "Observing app-server".into())
        },
        stale,
        summary: presentation
            .and_then(|p| p.detail.clone())
            .unwrap_or_default(),
        created_at_ms: 0,
        session: state.binding.session,
        terminal_message_id: None,
        codex_owner_id: Some(state.binding.owner_host_id),
        codex_binding_id: Some(state.binding.binding_id),
        deliveries: vec![],
    }
}


