use serde::{Deserialize, Serialize};

use crate::{
    access_plane_ref_for_id, delivery_lane_ref_for_parts, stream_id_ref_for_id, AccessPlaneRef,
    ContextId, DeliveryLaneRef, NodeId, StreamId, StreamIdRef, WIRE_PACKET_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolarWireHeader {
    pub version: u16,
    pub delivery_lane_id: DeliveryLaneRef,
    pub association_id: crate::AssociationId,
    pub admission: Option<crate::AdmissionBinding>,
    pub stream_id: StreamId,
    pub stream_id_ref: StreamIdRef,
    pub context_id: Option<ContextId>,
    pub context_epoch: Option<u32>,
    pub seq: u64,
    pub previous_seq: u64,
    pub sender_node_id: NodeId,
    pub target_node_id: Option<NodeId>,
    pub access_plane_ref: AccessPlaneRef,
    pub body_kind_code: u16,
    pub encoding_code: u8,
    pub compression_code: u8,
    pub protection_mode_code: u8,
    pub created_at_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse_lane_ack_through_seq: Option<u64>,
}

impl PolarWireHeader {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream_id: impl Into<StreamId>,
        seq: u64,
        previous_seq: u64,
        sender_node_id: impl Into<NodeId>,
        target_node_id: Option<NodeId>,
        access_plane_ref: AccessPlaneRef,
        body_kind_code: u16,
        protection_mode_code: u8,
        created_at_secs: u64,
    ) -> Self {
        let stream_id = stream_id.into();
        let sender_node_id = sender_node_id.into();
        let delivery_lane_id = delivery_lane_ref_for_parts(
            &sender_node_id,
            target_node_id.as_deref(),
            &access_plane_ref,
        );
        Self {
            version: WIRE_PACKET_VERSION,
            delivery_lane_id,
            association_id: [0; 32],
            admission: None,
            stream_id_ref: stream_id_ref_for_id(&stream_id),
            stream_id,
            context_id: None,
            context_epoch: None,
            seq,
            previous_seq,
            sender_node_id,
            target_node_id,
            access_plane_ref,
            body_kind_code,
            encoding_code: crate::ENCODING_CBOR,
            compression_code: crate::COMPRESSION_NONE,
            protection_mode_code,
            created_at_secs,
            reverse_lane_ack_through_seq: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_for_access_plane(
        stream_id: impl Into<StreamId>,
        seq: u64,
        previous_seq: u64,
        sender_node_id: impl Into<NodeId>,
        target_node_id: Option<NodeId>,
        canonical_access_plane_id: &str,
        body_kind_code: u16,
        protection_mode_code: u8,
        created_at_secs: u64,
    ) -> Self {
        Self::new(
            stream_id,
            seq,
            previous_seq,
            sender_node_id,
            target_node_id,
            access_plane_ref_for_id(canonical_access_plane_id),
            body_kind_code,
            protection_mode_code,
            created_at_secs,
        )
    }

    /// Bind to a pair of installation generations. Runtime admission must
    /// independently validate this identifier before admitting business traffic.
    pub fn with_association(mut self, association_id: crate::AssociationId) -> Self {
        self.association_id = association_id;
        self
    }

    pub fn with_reverse_lane_ack_through_seq(mut self, ack_through_seq: Option<u64>) -> Self {
        self.reverse_lane_ack_through_seq = ack_through_seq;
        self
    }

    pub fn with_stream_context(
        mut self,
        context_id: impl Into<ContextId>,
        context_epoch: u32,
    ) -> Self {
        self.context_id = Some(context_id.into());
        self.context_epoch = Some(context_epoch);
        self
    }
}
