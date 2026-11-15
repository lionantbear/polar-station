//! Generation projection binds Inhouse lifecycle to existing full LGP message keys.
use polar_inhouse_model::{ReplyGenerationId, ReplyGenerationState};
pub use polar_lgp_model::LgpMessageKey as ReplyMessageKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyGenerationView {
    pub schema_version: u8,
    pub generation_id: ReplyGenerationId,
    pub owner: String,
    pub channel: String,
    pub thread: String,
    pub request: ReplyMessageKey,
    pub actor_id: String,
    pub actor_name: String,
    pub created_at_secs: u64,
    pub sequence: u64,
    pub state: ReplyGenerationState,
    pub answer: String,
    pub status_text: String,
    pub phase: String,
    #[serde(default)]
    pub presentation: Option<polar_lgp_presentation::ActivityPresentation>,
    #[serde(default)]
    pub tools: Vec<polar_inhouse_model::ReplyToolActivity>,
    pub truncated: bool,
    pub final_message: Option<ReplyMessageKey>,
}

impl ReplyGenerationView {
    pub fn refresh_presentation(&mut self) {
        self.presentation = Some(polar_lgp_presentation::ActivityPresentation::project(
            self.state, &self.phase, &self.status_text,
        ));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageActivityQuery {
    pub owner_node_id_hex: String,
    pub thread_id: String,
    pub key: ReplyMessageKey,
    pub after_sequence: Option<i64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolActivityObservation {
    pub generation_id: ReplyGenerationId,
    pub sequence: i64,
    pub observed_at_secs: i64,
    pub state_label: String,
    pub tool: polar_inhouse_model::ReplyToolActivity,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageActivityPage {
    pub observations: Vec<ToolActivityObservation>,
    pub next_cursor: Option<i64>,
    /// False when no correlated observations are available; not proof of no tools.
    pub available: bool,
}
