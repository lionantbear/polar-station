use polar_identity::{
    BindingPublicKey, NodeId, NodeName, SignatureBytes, SyncSigner, UnixTimestamp,
};
use polar_transport::AccessPlaneEndpointHints;

use crate::canonical::signable::trust_decision_signable_with_plane_endpoints;
use crate::errors::TrustError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecisionKind {
    Approve,
    Reject,
    Revoke,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDecision {
    pub from_node_id: NodeId,
    pub to_node_id: NodeId,
    pub sender_node_name: NodeName,
    pub sender_binding_pubkey: BindingPublicKey,
    pub sender_plane: String,
    pub receiver_plane: String,
    pub kind: TrustDecisionKind,
    pub timestamp: UnixTimestamp,
    pub peer_ca_der: Option<Vec<u8>>,
    pub peer_session_pubkey: Option<[u8; 32]>,
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub sig: SignatureBytes,
}

impl TrustDecision {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        from_node_id: NodeId,
        to_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        sender_plane: String,
        receiver_plane: String,
        kind: TrustDecisionKind,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material(
            from_node_id,
            to_node_id,
            sender_node_name,
            sender_binding_pubkey,
            sender_plane,
            receiver_plane,
            kind,
            timestamp,
            None,
            None,
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_material(
        from_node_id: NodeId,
        to_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        sender_plane: String,
        receiver_plane: String,
        kind: TrustDecisionKind,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material_and_endpoints(
            from_node_id,
            to_node_id,
            sender_node_name,
            sender_binding_pubkey,
            sender_plane,
            receiver_plane,
            kind,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            AccessPlaneEndpointHints::new(),
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_material_and_endpoints(
        from_node_id: NodeId,
        to_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        sender_plane: String,
        receiver_plane: String,
        kind: TrustDecisionKind,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        plane_endpoints: AccessPlaneEndpointHints,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        if signer.node_id() != from_node_id {
            return Err(TrustError::IdentityMismatch);
        }
        let signable = trust_decision_signable_with_plane_endpoints(
            &from_node_id,
            &to_node_id,
            &sender_node_name,
            &sender_binding_pubkey,
            &sender_plane,
            &receiver_plane,
            kind,
            timestamp,
            peer_ca_der.as_deref(),
            peer_session_pubkey.as_ref(),
            &plane_endpoints,
        );
        Ok(Self {
            from_node_id,
            to_node_id,
            sender_node_name,
            sender_binding_pubkey,
            sender_plane,
            receiver_plane,
            kind,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            plane_endpoints,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    #[allow(clippy::too_many_arguments)]
    pub async fn sign_with_peer_material_and_endpoints_async(
        from_node_id: NodeId,
        to_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        sender_plane: String,
        receiver_plane: String,
        kind: TrustDecisionKind,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        plane_endpoints: AccessPlaneEndpointHints,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, TrustError> {
        if signer.descriptor().node_id != from_node_id {
            return Err(TrustError::IdentityMismatch);
        }
        let signable = trust_decision_signable_with_plane_endpoints(
            &from_node_id,
            &to_node_id,
            &sender_node_name,
            &sender_binding_pubkey,
            &sender_plane,
            &receiver_plane,
            kind,
            timestamp,
            peer_ca_der.as_deref(),
            peer_session_pubkey.as_ref(),
            &plane_endpoints,
        );
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::TrustDecisionV1,
            signable,
        )?;
        let sig = request
            .sign_with(from_node_id, signer, polar_identity::SignOptions::default())
            .await?;
        Ok(Self {
            from_node_id,
            to_node_id,
            sender_node_name,
            sender_binding_pubkey,
            sender_plane,
            receiver_plane,
            kind,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            plane_endpoints,
            sig,
        })
    }
}
