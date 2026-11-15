use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const REPLY_GENERATION_EVENT: &str = "inhouse.reply_generation.updated";
pub const REPLY_PREVIEW_BYTES: usize = 1024 * 1024;

/// Identity of an ephemeral execution, NEVER the identity of a Message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Uuid", into = "Uuid")]
pub struct ReplyGenerationId(Uuid);

impl ReplyGenerationId {
    pub fn new() -> Self {
        Self(polar_identity::generation::new_uuid_v7())
    }
}
impl Default for ReplyGenerationId {
    fn default() -> Self {
        Self::new()
    }
}
impl TryFrom<Uuid> for ReplyGenerationId {
    type Error = &'static str;
    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        if value.get_version_num() != 7 || value.get_variant() != uuid::Variant::RFC4122 {
            return Err("generation ID must be UUIDv7");
        }
        Ok(Self(value))
    }
}
impl From<ReplyGenerationId> for Uuid {
    fn from(value: ReplyGenerationId) -> Self {
        value.0
    }
}
impl std::fmt::Display for ReplyGenerationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyGenerationState {
    Waiting,
    Generating,
    Committing,
    Completed,
    Failed,
    Cancelled,
}
impl ReplyGenerationState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}


/// A provider call ID is unique only within its reply generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyToolActivity {
    pub call_id: String,
    pub tool_name: String,
    pub state: ReplyToolState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyToolState { Running, Completed, Failed, Cancelled, Interrupted }


