use polar_identity::{NodeId, UnixTimestamp};

/// The durable discussion identity participating in a Thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadParticipantKind {
    Human,
    InhousePolar,
}

impl ThreadParticipantKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::InhousePolar => "inhouse_polar",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "inhouse_polar" => Some(Self::InhousePolar),
            _ => None,
        }
    }
}

/// Durable membership and identity for one Thread participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadParticipantRecord {
    pub owner_id: NodeId,
    pub thread_id: String,
    pub participant_kind: ThreadParticipantKind,
    pub participant_id: String,
    pub display_name: String,
    pub joined_at: UnixTimestamp,
    pub active: bool,
}
