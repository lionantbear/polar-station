use std::collections::BTreeMap;

use polar_tmux_protocol::{
    AttentionBadgeProjectionV1, SessionAttentionProjectionV1, SessionAttentionStateV1,
};

use crate::SessionKey;

pub type SessionAttentionMap = BTreeMap<SessionKey, SessionAttentionProjectionV1>;

pub fn default_attention(identity: &SessionKey) -> SessionAttentionProjectionV1 {
    SessionAttentionProjectionV1 {
        execution_target_id: identity.execution_target_id.to_string(),
        tmux_session_id: identity.tmux_session_id.to_string(),
        ..SessionAttentionProjectionV1::default()
    }
}

pub fn badge_for<'a>(
    attentions: impl IntoIterator<Item = &'a SessionAttentionProjectionV1>,
) -> AttentionBadgeProjectionV1 {
    let mut badge = AttentionBadgeProjectionV1::default();
    for attention in attentions {
        if attention.state.is_red() {
            badge.unresolved_red = badge.unresolved_red.saturating_add(1);
        } else if attention.state.is_orange() && attention.unread {
            badge.unread_orange = badge.unread_orange.saturating_add(1);
        }
        if attention_rank(attention.state) > attention_rank(badge.highest_state) {
            badge.highest_state = attention.state;
        }
    }
    badge
}

fn attention_rank(state: SessionAttentionStateV1) -> u8 {
    match state {
        SessionAttentionStateV1::Idle => 0,
        SessionAttentionStateV1::Unknown => 1,
        SessionAttentionStateV1::Working => 2,
        SessionAttentionStateV1::ReviewReady | SessionAttentionStateV1::NeedsAttention => 3,
        SessionAttentionStateV1::ApprovalRequired => 4,
        SessionAttentionStateV1::DecisionRequired => 5,
        SessionAttentionStateV1::PermissionRequired => 6,
        SessionAttentionStateV1::Failed => 7,
    }
}

use polar_tmux_protocol::{
    InboxMessageProjectionV1, SessionSummaryFreshnessV1, TerminalOriginHandleV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Flat native binding facts, not a layout model. Host still validates observations.
#[derive(Debug, Deserialize)]
pub struct AttentionObservation {
    pub origin: TerminalOriginHandleV1,
    pub state: SessionAttentionStateV1,
    pub freshness: SessionSummaryFreshnessV1,
    pub attention_since_ms: Option<u64>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttentionInput {
    Reconnect,
    Observe {
        observations: Vec<AttentionObservation>,
    },
    Messages {
        messages: Vec<InboxMessageProjectionV1>,
        visible_workspaces: Vec<String>,
        enabled: bool,
    },
}
#[derive(Debug, Default, Serialize)]
pub struct AttentionEffects {
    pub observe: Vec<TerminalOriginHandleV1>,
    pub notify: Vec<AttentionNotification>,
}
#[derive(Debug, Serialize)]
pub struct AttentionNotification {
    pub workspace_id: String,
    pub message_id: String,
    pub count: usize,
}
type OriginKey = (String, String, String, String, String);
type AttentionEdge = (SessionAttentionStateV1, u64);

/// Transient desktop projection only: no delivery queue, retry clock or Inbox store.
#[derive(Debug, Default)]
pub struct AttentionEngine {
    edges: BTreeMap<OriginKey, AttentionEdge>,
    initialized: bool,
    watermark: u64,
    watermark_ids: BTreeSet<String>,
}
impl AttentionEngine {
    pub fn dispatch(&mut self, input: AttentionInput) -> AttentionEffects {
        let mut effects = AttentionEffects::default();
        match input {
            AttentionInput::Reconnect => {
                self.initialized = false;
                self.edges.clear();
            }
            AttentionInput::Observe { observations } => {
                let mut current = BTreeMap::new();
                for observation in observations {
                    let Some(since) = observation.attention_since_ms else {
                        continue;
                    };
                    if !(observation.state.is_red() || observation.state.is_orange())
                        || matches!(
                            observation.freshness,
                            SessionSummaryFreshnessV1::Analyzing
                                | SessionSummaryFreshnessV1::Unavailable
                        )
                    {
                        continue;
                    }
                    let origin = observation.origin;
                    let key = (
                        origin.workspace_id.clone(),
                        origin.tab_id.clone(),
                        origin.pane_id.clone(),
                        origin.execution_target_id.clone(),
                        origin.tmux_session_id.clone(),
                    );
                    let edge = (observation.state, since);
                    if self.edges.get(&key) != Some(&edge) && current.get(&key) != Some(&edge) {
                        effects.observe.push(origin);
                    }
                    current.insert(key, edge);
                }
                self.edges = current;
            }
            AttentionInput::Messages {
                messages,
                visible_workspaces,
                enabled,
            } => {
                let visible: BTreeSet<_> = visible_workspaces.into_iter().collect();
                let mut notices: BTreeMap<String, AttentionNotification> = BTreeMap::new();
                let mut batch = BTreeSet::new();
                for message in &messages {
                    let id = message.id.as_str();
                    let fresh = message.created_at_ms > self.watermark
                        || (message.created_at_ms == self.watermark
                            && !self.watermark_ids.contains(id));
                    // Red only. Reading, settings changes and visibility changes still
                    // consume the edge, so they cannot replay an old notification.
                    let red = matches!(
                        message.attention.as_str(),
                        "approvalrequired"
                            | "decisionrequired"
                            | "permissionrequired"
                            | "approval_required"
                            | "decision_required"
                            | "permission_required"
                            | "failed"
                    );
                    if self.initialized
                        && fresh
                        && batch.insert(id)
                        && enabled
                        && red
                        && message.unread
                        && !visible.contains(&message.origin.workspace_id)
                    {
                        let notice = notices
                            .entry(message.origin.workspace_id.clone())
                            .or_insert_with(|| AttentionNotification {
                                workspace_id: message.origin.workspace_id.clone(),
                                message_id: id.to_owned(),
                                count: 0,
                            });
                        notice.count += 1;
                    }
                }
                if let Some(latest) = messages.iter().map(|m| m.created_at_ms).max() {
                    if latest > self.watermark {
                        self.watermark = latest;
                        self.watermark_ids.clear();
                    }
                    self.watermark_ids.extend(
                        messages
                            .iter()
                            .filter(|m| m.created_at_ms == self.watermark)
                            .map(|m| m.id.as_str().to_owned()),
                    );
                }
                self.initialized = true;
                effects.notify = notices.into_values().collect();
            }
        }
        effects
    }
}


