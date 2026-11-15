use polar_identity::{NodeId, SignatureBytes, SyncSigner, UnixTimestamp};

use crate::errors::ProtocolError;

pub const DM_ACK_GENESIS_MESSAGE_ID: &str = "__polar_dm_ack_genesis__";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmAckSegment {
    pub previous_acked_message_id: String,
    pub acked_message_id: String,
    pub received_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmReplayRequest {
    pub sender_node_id: NodeId,
    pub target_node_id: NodeId,
    pub access_plane_id: String,
    pub ack_segment: DmAckSegment,
    pub timestamp: UnixTimestamp,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDmReplayRequest {
    pub sender_node_id: NodeId,
    pub target_node_id: NodeId,
    pub access_plane_id: String,
    pub ack_segment: DmAckSegment,
    pub timestamp: UnixTimestamp,
}

impl DmReplayRequest {
    pub fn signing_payload(
        sender_node_id: NodeId,
        target_node_id: NodeId,
        access_plane_id: &str,
        ack_segment: &DmAckSegment,
        timestamp: UnixTimestamp,
    ) -> Vec<u8> {
        format!(
            "polar:dm_replay_request:v2:{}:{}:{}:{}:{}:{}:{}",
            sender_node_id.to_hex(),
            target_node_id.to_hex(),
            access_plane_id,
            ack_segment.previous_acked_message_id,
            ack_segment.acked_message_id,
            ack_segment.received_count,
            timestamp.as_secs(),
        )
        .into_bytes()
    }

    pub fn sign(
        sender_node_id: NodeId,
        target_node_id: NodeId,
        access_plane_id: String,
        ack_segment: DmAckSegment,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let payload = Self::signing_payload(
            sender_node_id,
            target_node_id,
            &access_plane_id,
            &ack_segment,
            timestamp,
        );
        Ok(Self {
            sender_node_id,
            target_node_id,
            access_plane_id,
            ack_segment,
            timestamp,
            sig: signer.sign(&payload),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        sender_node_id: NodeId,
        target_node_id: NodeId,
        access_plane_id: String,
        ack_segment: DmAckSegment,
        timestamp: UnixTimestamp,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let payload = Self::signing_payload(
            sender_node_id,
            target_node_id,
            &access_plane_id,
            &ack_segment,
            timestamp,
        );
        let request =
            polar_identity::SignRequest::new(polar_identity::SigningPurpose::DmReplayV1, payload)?;
        let sig = request
            .sign_with(
                sender_node_id,
                signer,
                polar_identity::SignOptions::default(),
            )
            .await?;
        Ok(Self {
            sender_node_id,
            target_node_id,
            access_plane_id,
            ack_segment,
            timestamp,
            sig,
        })
    }
}

pub fn verify_dm_replay_request(
    request: &DmReplayRequest,
) -> Result<VerifiedDmReplayRequest, ProtocolError> {
    let payload = DmReplayRequest::signing_payload(
        request.sender_node_id,
        request.target_node_id,
        &request.access_plane_id,
        &request.ack_segment,
        request.timestamp,
    );
    crate::canonical::signable::verify_signature(&request.sender_node_id, &payload, &request.sig)?;

    Ok(VerifiedDmReplayRequest {
        sender_node_id: request.sender_node_id,
        target_node_id: request.target_node_id,
        access_plane_id: request.access_plane_id.clone(),
        ack_segment: request.ack_segment.clone(),
        timestamp: request.timestamp,
    })
}


