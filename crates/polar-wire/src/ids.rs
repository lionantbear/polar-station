use serde::{Deserialize, Serialize};

pub type NodeId = String;
pub type StreamId = String;
pub type ContextId = String;
pub type SignatureBytes = Vec<u8>;
pub type HashBytes = [u8; 32];
pub type AccessPlaneRef = [u8; 16];
pub type StreamIdRef = [u8; 16];
pub type DeliveryLaneRef = [u8; 16];

pub fn access_plane_ref_for_id(canonical_access_plane_id: &str) -> AccessPlaneRef {
    blake3_128(canonical_access_plane_id.trim().as_bytes())
}

pub fn stream_id_ref_for_id(canonical_stream_id: &str) -> StreamIdRef {
    blake3_128(canonical_stream_id.as_bytes())
}

pub fn delivery_lane_ref_for_parts(
    sender_node_id: &str,
    receiver_node_id: Option<&str>,
    access_plane_ref: &AccessPlaneRef,
) -> DeliveryLaneRef {
    let mut hasher = blake3::Hasher::new();
    hasher.update(sender_node_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(receiver_node_id.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    hasher.update(access_plane_ref);
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest.as_bytes()[..16]);
    out
}

pub(crate) fn blake3_128(bytes: &[u8]) -> [u8; 16] {
    let digest = blake3::hash(bytes);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest.as_bytes()[..16]);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketLookupKey {
    pub local_node_id: NodeId,
    pub peer_node_id: NodeId,
    pub access_plane_id: String,
    pub stream_id: StreamId,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketRange {
    pub local_node_id: NodeId,
    pub peer_node_id: NodeId,
    pub access_plane_id: String,
    pub stream_id: StreamId,
    pub from_seq: u64,
    pub max_packets: u32,
}
