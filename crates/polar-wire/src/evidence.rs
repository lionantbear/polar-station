use serde::{Deserialize, Serialize};

use crate::{HashBytes, NodeId, StreamId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketEvidenceStatement {
    pub sender_node_id: NodeId,
    pub receiver_node_id: NodeId,
    pub stream_id: StreamId,
    pub seq: u64,
    pub body_hash: HashBytes,
    pub packet_hash: HashBytes,
    pub created_at_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketPersistStatement {
    pub sender_node_id: NodeId,
    pub receiver_node_id: NodeId,
    pub stream_id: StreamId,
    pub persisted_seq: u64,
    pub source_packet_hash: HashBytes,
    pub body_hash: HashBytes,
    pub persisted_at_secs: u64,
}
