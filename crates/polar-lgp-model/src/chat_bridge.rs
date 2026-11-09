use polar_identity::{NodeId, UnixTimestamp};
use serde::{Deserialize, Serialize};

use crate::TransportPlane;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRouteStatus {
    Attached,
    Disabled,
}

impl ChatRouteStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attached => "attached",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "attached" => Some(Self::Attached),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRouteRecord {
    pub route_id: String,
    pub source_owner_id: NodeId,
    pub source_channel_id: String,
    pub source_thread_id: String,
    pub target_owner_id: NodeId,
    pub target_channel_id: String,
    pub target_thread_id: String,
    pub target_inhouse_polar_id: String,
    pub target_transport_plane: TransportPlane,
    pub status: ChatRouteStatus,
    pub created_at: UnixTimestamp,
    pub updated_at: UnixTimestamp,
    pub metadata_json: String,
}


