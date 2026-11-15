//! Host orchestration for the Polar Station Inbox domain.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use polar_station_inbox::{
    AttentionConfidence, AttentionKind, AttentionSource, DeliveryStatus, Inbox, InboxMessage,
    InboxMessageId, MessageInput, ObserveOutcome, TerminalOrigin,
};
use polar_station_store_sqlite::SqliteInboxStore;
use polar_tmux_protocol::{
    AiInvocationProjectionV1, ApplicationStateApiV1, DeliveryReceiptStateV1, InboxApiV1,
    InboxCommandV1, InboxDraftStatusV1, InboxMessageIdV1, InboxMessageProjectionV1, InboxPageV1,
    InboxRequestEnvelopeV1, InboxResponseEnvelopeV1, InboxResultV1, ProtocolError,
    SessionAttentionConfidenceV1, SessionAttentionSourceV1, SessionAttentionStateV1,
    SessionSummaryFreshnessV1, TerminalOriginHandleV1, PROTOCOL_VERSION,
};

use super::{application_state::ApplicationStatePublisher, session_registry::SessionRegistry};

#[derive(Clone)]
pub(crate) struct InboxService {
    inbox: Arc<Mutex<Inbox>>,
    store: Arc<Mutex<SqliteInboxStore>>,
    pub(super) sessions: SessionRegistry,
    #[cfg(feature = "codex-control")]
    pub(super) codex: Arc<std::sync::OnceLock<Arc<super::codex_control::CodexControlService>>>,
    events: ApplicationStatePublisher,
}

impl InboxService {
    #[cfg(feature = "codex-control")]
    pub(crate) fn attach_codex(&self, codex: Arc<super::codex_control::CodexControlService>) {
        assert!(self.codex.set(codex).is_ok());
    }

    pub(crate) fn open(
        store: SqliteInboxStore,
        sessions: SessionRegistry,
        events: ApplicationStatePublisher,
    ) -> Result<Self, String> {
        let mut inbox = Inbox::default();
        store.prune(now_ms()).map_err(|error| error.to_string())?;
        for message in store.page(None, 200).map_err(|error| error.to_string())? {
            inbox.restore(message);
        }
        Ok(Self {
            inbox: Arc::new(Mutex::new(inbox)),
            store: Arc::new(Mutex::new(store)),
            sessions,
            #[cfg(feature = "codex-control")]
            codex: Arc::new(std::sync::OnceLock::new()),
            events,
        })
    }

    pub(crate) async fn synchronize_origin(&self, native_origin: TerminalOriginHandleV1) {
        let snapshot = self.sessions.snapshot().await;
        for session in snapshot.sessions.iter().filter(|session| {
            session.key.execution_target.as_str() == native_origin.execution_target_id
                && session.key.tmux_session_id == native_origin.tmux_session_id
        }) {
            if self.sessions.protocol_owns_attention(&session.key).await {
                continue;
            }
            let Some(attention) = &session.attention else {
                continue;
            };
            let Some(kind) = inbox_attention_kind(attention.state, attention.freshness) else {
                continue;
            };
            let Some(attention_since_ms) = attention.attention_since_ms else {
                continue;
            };
            if attention.summary_sections.is_empty() {
                continue;
            }
            let input = MessageInput {
                origin: TerminalOrigin {
                    workspace_id: native_origin.workspace_id.clone(),
                    tab_id: native_origin.tab_id.clone(),
                    pane_id: native_origin.pane_id.clone(),
                    execution_target_id: native_origin.execution_target_id.clone(),
                    tmux_session_id: native_origin.tmux_session_id.clone(),
                    consumer_generation: native_origin.consumer_generation,
                    native_recovery_token: native_origin.native_recovery_token.clone(),
                },
                session_label: session.display_name.clone(),
                attention: kind,
                source: inbox_attention_source(attention.primary_source),
                confidence: match attention.confidence {
                    SessionAttentionConfidenceV1::Direct => AttentionConfidence::Direct,
                    SessionAttentionConfidenceV1::Inferred => AttentionConfidence::Inferred,
                },
                source_event_id: attention.source_event_id.clone(),
                summary: attention.summary_sections.join(" · "),
                source_revision: attention.summary_revision,
                occurred_at_ms: attention_since_ms,
                ai_invocation: None,
            };
            let outcome = self
                .inbox
                .lock()
                .ok()
                .and_then(|mut inbox| inbox.observe(input).ok());
            let changed = match outcome {
                Some(ObserveOutcome::Created(id) | ObserveOutcome::Updated(id)) => Some(id),
                Some(ObserveOutcome::Unchanged) | None => None,
            };
            if let Some(id) = changed {
                if let Some(message) = self
                    .inbox
                    .lock()
                    .ok()
                    .and_then(|inbox| inbox.message(&id).cloned())
                {
                    let _ = self
                        .store
                        .lock()
                        .map_err(|_| ())
                        .and_then(|store| store.save(&message).map_err(|_| ()));
                }
                self.events.publish(ApplicationStateApiV1::InboxV1);
            }
        }
    }

    pub(crate) async fn dispatch(
        &self,
        request: InboxRequestEnvelopeV1,
    ) -> InboxResponseEnvelopeV1 {
        let request_id = request.request_id.clone();
        let result = self
            .dispatch_command(request.command)
            .await
            .unwrap_or_else(|message| InboxResultV1::Error {
                error: ProtocolError::new("inbox_request_failed", &message, true),
                message: None,
            });
        InboxResponseEnvelopeV1 {
            protocol_version: PROTOCOL_VERSION,
            api: InboxApiV1::InboxV1,
            request_id,
            event_revision: now_ms(),
            result,
        }
    }

    async fn dispatch_command(&self, command: InboxCommandV1) -> Result<InboxResultV1, String> {
        let command = match command {
            InboxCommandV1::ObserveAttention { origin } => {
                self.synchronize_origin(origin).await;
                InboxCommandV1::GetPage {
                    before_created_at_ms: None,
                    limit: 200,
                }
            }
            other => other,
        };
        match command {
            InboxCommandV1::ObserveAttention { .. } => unreachable!("normalized above"),
            InboxCommandV1::GetPage {
                before_created_at_ms,
                limit,
            } => {
                let messages = self
                    .store
                    .lock()
                    .map_err(|_| "Inbox store is unavailable".to_owned())?
                    .page(before_created_at_ms, usize::from(limit))
                    .map_err(|error| error.to_string())?;
                let unread_count = messages.iter().filter(|message| message.unread).count() as u32;
                let next = messages.last().map(|message| message.created_at_ms);
                let messages: Vec<_> = messages.iter().map(project).collect();
                let overview = self.overview(&messages).await;
                Ok(InboxResultV1::Page {
                    page: InboxPageV1 {
                        messages,
                        overview,
                        next_before_created_at_ms: next,
                        unread_count,
                    },
                })
            }
            InboxCommandV1::Reply {
                message_id,
                expected_revision,
                reply,
            } => {
                let id = domain_id(&message_id)?;
                let (revision, summary) = {
                    let mut inbox = self
                        .inbox
                        .lock()
                        .map_err(|_| "Inbox is unavailable".to_owned())?;
                    let revision = inbox
                        .reply(&id, expected_revision, &reply)
                        .map_err(|error| error.to_string())?;
                    let message = inbox
                        .message(&id)
                        .ok_or_else(|| "Inbox message was not found".to_owned())?;
                    self.save(message)?;
                    (revision, message.summary.clone())
                };
                self.generate_draft(id, revision, summary, reply).await
            }
            InboxCommandV1::Regenerate {
                message_id,
                expected_revision,
            } => {
                let id = domain_id(&message_id)?;
                let (summary, reply) = {
                    let inbox = self
                        .inbox
                        .lock()
                        .map_err(|_| "Inbox is unavailable".to_owned())?;
                    let message = inbox
                        .message(&id)
                        .ok_or_else(|| "Inbox message was not found".to_owned())?;
                    if message.revision != expected_revision {
                        return Err(
                            "Inbox revision changed; refresh before regenerating".to_owned()
                        );
                    }
                    (
                        message.summary.clone(),
                        message
                            .reply
                            .clone()
                            .ok_or_else(|| "Reply before regenerating a draft".to_owned())?,
                    )
                };
                self.generate_draft(id, expected_revision, summary, reply)
                    .await
            }
            InboxCommandV1::EditDraft {
                message_id,
                expected_revision,
                draft,
            } => self
                .mutate_message(&message_id, |inbox, id| {
                    inbox.edit_draft(id, expected_revision, &draft)
                })
                .map(|message| InboxResultV1::Message { message }),
            InboxCommandV1::Acknowledge {
                message_id,
                expected_revision,
            } => self
                .mutate_message(&message_id, |inbox, id| {
                    inbox.acknowledge(id, expected_revision)
                })
                .map(|message| InboxResultV1::Message { message }),
            InboxCommandV1::Approve {
                message_id,
                expected_revision,
                delivery_mode,
                approval_nonce,
            } => {
                let id = domain_id(&message_id)?;
                let (authorization, message) = {
                    let mut inbox = self
                        .inbox
                        .lock()
                        .map_err(|_| "Inbox is unavailable".to_owned())?;
                    let authorization = inbox
                        .approve(&id, expected_revision, delivery_mode, &approval_nonce)
                        .map_err(|error| error.to_string())?;
                    let message = inbox
                        .message(&id)
                        .cloned()
                        .ok_or_else(|| "Inbox message was not found".to_owned())?;
                    (authorization, message)
                };
                self.save(&message)?;
                self.events.publish(ApplicationStateApiV1::InboxV1);
                Ok(InboxResultV1::DeliveryRequested {
                    message_id,
                    origin: origin(&authorization.origin),
                    text: authorization.text,
                    delivery_mode,
                    approval_nonce,
                })
            }
            InboxCommandV1::RecordDelivery {
                message_id,
                expected_revision,
                state,
            } => {
                let domain_state = match state {
                    DeliveryReceiptStateV1::Restoring => DeliveryStatus::Restoring,
                    DeliveryReceiptStateV1::Delivered => DeliveryStatus::Delivered,
                    DeliveryReceiptStateV1::FailedBeforeWrite => DeliveryStatus::FailedBeforeWrite,
                    DeliveryReceiptStateV1::DeliveryUnknown => DeliveryStatus::DeliveryUnknown,
                };
                self.mutate_message(&message_id, |inbox, id| {
                    inbox.record_delivery(id, expected_revision, domain_state)
                })?;
                Ok(InboxResultV1::DeliveryReceipt {
                    message_id,
                    state,
                    error: None,
                })
            }
        }
    }

    async fn generate_draft(
        &self,
        id: InboxMessageId,
        revision: u64,
        summary: String,
        reply: String,
    ) -> Result<InboxResultV1, String> {
        let started = Instant::now();
        match self.sessions.generate_reply_draft(&summary, &reply).await {
            Ok(draft) => {
                let mut inbox = self
                    .inbox
                    .lock()
                    .map_err(|_| "Inbox is unavailable".to_owned())?;
                inbox
                    .record_draft(
                        &id,
                        revision,
                        &draft,
                        "configured",
                        started.elapsed().as_millis() as u64,
                    )
                    .map_err(|error| error.to_string())?;
                let message = inbox
                    .message(&id)
                    .cloned()
                    .ok_or_else(|| "Inbox message was not found".to_owned())?;
                self.save(&message)?;
                self.events.publish(ApplicationStateApiV1::InboxV1);
                Ok(InboxResultV1::Message {
                    message: project(&message),
                })
            }
            Err(error) => {
                let mut inbox = self
                    .inbox
                    .lock()
                    .map_err(|_| "Inbox is unavailable".to_owned())?;
                inbox
                    .fail_draft(
                        &id,
                        revision,
                        &sanitize_error(&error),
                        started.elapsed().as_millis() as u64,
                    )
                    .map_err(|failure| failure.to_string())?;
                let message = inbox
                    .message(&id)
                    .cloned()
                    .ok_or_else(|| "Inbox message was not found".to_owned())?;
                self.save(&message)?;
                Ok(InboxResultV1::Message {
                    message: project(&message),
                })
            }
        }
    }

    fn mutate_message(
        &self,
        id: &InboxMessageIdV1,
        mutate: impl FnOnce(&mut Inbox, &InboxMessageId) -> Result<u64, polar_station_inbox::InboxError>,
    ) -> Result<InboxMessageProjectionV1, String> {
        let id = domain_id(id)?;
        let mut inbox = self
            .inbox
            .lock()
            .map_err(|_| "Inbox is unavailable".to_owned())?;
        mutate(&mut inbox, &id).map_err(|error| error.to_string())?;
        let message = inbox
            .message(&id)
            .cloned()
            .ok_or_else(|| "Inbox message was not found".to_owned())?;
        self.save(&message)?;
        self.events.publish(ApplicationStateApiV1::InboxV1);
        Ok(project(&message))
    }

    fn save(&self, message: &InboxMessage) -> Result<(), String> {
        self.store
            .lock()
            .map_err(|_| "Inbox store is unavailable".to_owned())?
            .save(message)
            .map_err(|error| error.to_string())
    }
}

fn domain_id(id: &InboxMessageIdV1) -> Result<InboxMessageId, String> {
    InboxMessageId::new(id.as_str()).map_err(|error| error.to_string())
}
fn origin(value: &TerminalOrigin) -> TerminalOriginHandleV1 {
    TerminalOriginHandleV1 {
        workspace_id: value.workspace_id.clone(),
        tab_id: value.tab_id.clone(),
        pane_id: value.pane_id.clone(),
        execution_target_id: value.execution_target_id.clone(),
        tmux_session_id: value.tmux_session_id.clone(),
        consumer_generation: value.consumer_generation,
        native_recovery_token: value.native_recovery_token.clone(),
    }
}
fn project(value: &InboxMessage) -> InboxMessageProjectionV1 {
    InboxMessageProjectionV1 {
        id: InboxMessageIdV1::new(value.id.as_str()).expect("stored Inbox ID remains valid"),
        origin: origin(&value.origin),
        session_label: value.session_label.clone(),
        attention: format!("{:?}", value.attention).to_ascii_lowercase(),
        source: value.source,
        confidence: value.confidence,
        summary: value.summary.clone(),
        created_at_ms: value.created_at_ms,
        revision: value.revision,
        unread: value.unread,
        reply: value.reply.clone(),
        draft: value.draft.clone(),
        draft_status: match value.draft_status {
            polar_station_inbox::DraftStatus::None => InboxDraftStatusV1::None,
            polar_station_inbox::DraftStatus::Generating => InboxDraftStatusV1::Generating,
            polar_station_inbox::DraftStatus::Ready => InboxDraftStatusV1::Ready,
            polar_station_inbox::DraftStatus::Failed => InboxDraftStatusV1::Failed,
            polar_station_inbox::DraftStatus::Approved => InboxDraftStatusV1::Approved,
        },
        stale_context: false,
        ai_invocation: value
            .ai_invocation
            .as_ref()
            .map(|ai| AiInvocationProjectionV1 {
                provider: ai.provider.clone(),
                model: ai.model.clone(),
                duration_ms: ai.duration_ms,
                error: ai.error.clone(),
            }),
    }
}
fn inbox_attention_kind(
    value: SessionAttentionStateV1,
    freshness: SessionSummaryFreshnessV1,
) -> Option<AttentionKind> {
    if matches!(
        freshness,
        SessionSummaryFreshnessV1::Analyzing | SessionSummaryFreshnessV1::Unavailable
    ) {
        return None;
    }
    match value {
        SessionAttentionStateV1::Idle
        | SessionAttentionStateV1::Unknown
        | SessionAttentionStateV1::Working => None,
        SessionAttentionStateV1::ReviewReady => Some(AttentionKind::ReviewReady),
        SessionAttentionStateV1::NeedsAttention => Some(AttentionKind::NeedsAttention),
        SessionAttentionStateV1::ApprovalRequired => Some(AttentionKind::ApprovalRequired),
        SessionAttentionStateV1::DecisionRequired => Some(AttentionKind::DecisionRequired),
        SessionAttentionStateV1::PermissionRequired => Some(AttentionKind::PermissionRequired),
        SessionAttentionStateV1::Failed => Some(AttentionKind::Failed),
    }
}
fn inbox_attention_source(value: SessionAttentionSourceV1) -> AttentionSource {
    match value {
        SessionAttentionSourceV1::Unspecified => AttentionSource::Unspecified,
        SessionAttentionSourceV1::TmuxActivity => AttentionSource::TmuxActivity,
        SessionAttentionSourceV1::TmuxBell => AttentionSource::TmuxBell,
        SessionAttentionSourceV1::PaneTitle => AttentionSource::PaneTitle,
        SessionAttentionSourceV1::AgentHook => AttentionSource::AgentHook,
        SessionAttentionSourceV1::AiClassification => AttentionSource::AiClassification,
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn sanitize_error(value: &str) -> String {
    value
        .split_whitespace()
        .take(40)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}


