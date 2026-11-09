use polar_identity::{BindingPublicKey, NodeName, UnixTimestamp};
use polar_identity::{NodeId, SignatureBytes};
use polar_transport::AccessPlaneEndpointHints;

use crate::canonical::signable::{
    trust_decision_signable_with_plane_endpoints, trust_request_signable, TrustRequestSignableParts,
};
use crate::decision::{TrustDecision, TrustDecisionKind};
use crate::errors::TrustError;
use crate::request::TrustRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTrustRequest {
    pub request_id: String,
    pub sender_node_id: NodeId,
    pub sender_node_name: NodeName,
    pub sender_binding_pubkey: BindingPublicKey,
    pub receiver_node_id: NodeId,
    pub sender_plane: String,
    pub receiver_plane: String,
    pub timestamp: UnixTimestamp,
    pub peer_ca_der: Option<Vec<u8>>,
    pub peer_session_pubkey: Option<[u8; 32]>,
    pub plane_endpoints: AccessPlaneEndpointHints,
    pub admission_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTrustDecision {
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
}

pub fn verify_trust_request(req: &TrustRequest) -> Result<VerifiedTrustRequest, TrustError> {
    let signable = trust_request_signable(TrustRequestSignableParts {
        request_id: &req.request_id,
        sender_node_id: &req.sender_node_id,
        sender_node_name: &req.sender_node_name,
        sender_binding_pubkey: &req.sender_binding_pubkey,
        receiver_node_id: &req.receiver_node_id,
        sender_plane: &req.sender_plane,
        receiver_plane: &req.receiver_plane,
        timestamp: req.timestamp,
        peer_ca_der: req.peer_ca_der.as_deref(),
        peer_session_pubkey: req.peer_session_pubkey.as_ref(),
        plane_endpoints: &req.plane_endpoints,
        admission_token: req.admission_token.as_deref(),
    });
    verify_signature(&req.sender_node_id, &signable, &req.sig)?;
    Ok(VerifiedTrustRequest {
        request_id: req.request_id.clone(),
        sender_node_id: req.sender_node_id,
        sender_node_name: req.sender_node_name.clone(),
        sender_binding_pubkey: req.sender_binding_pubkey,
        receiver_node_id: req.receiver_node_id,
        sender_plane: req.sender_plane.clone(),
        receiver_plane: req.receiver_plane.clone(),
        timestamp: req.timestamp,
        peer_ca_der: req.peer_ca_der.clone(),
        peer_session_pubkey: req.peer_session_pubkey,
        plane_endpoints: req.plane_endpoints.clone(),
        admission_token: req.admission_token.clone(),
    })
}

pub fn verify_trust_decision(
    decision: &TrustDecision,
) -> Result<VerifiedTrustDecision, TrustError> {
    let signable = trust_decision_signable_with_plane_endpoints(
        &decision.from_node_id,
        &decision.to_node_id,
        &decision.sender_node_name,
        &decision.sender_binding_pubkey,
        &decision.sender_plane,
        &decision.receiver_plane,
        decision.kind,
        decision.timestamp,
        decision.peer_ca_der.as_deref(),
        decision.peer_session_pubkey.as_ref(),
        &decision.plane_endpoints,
    );
    verify_signature(&decision.from_node_id, &signable, &decision.sig)?;
    Ok(VerifiedTrustDecision {
        from_node_id: decision.from_node_id,
        to_node_id: decision.to_node_id,
        sender_node_name: decision.sender_node_name.clone(),
        sender_binding_pubkey: decision.sender_binding_pubkey,
        sender_plane: decision.sender_plane.clone(),
        receiver_plane: decision.receiver_plane.clone(),
        kind: decision.kind,
        timestamp: decision.timestamp,
        peer_ca_der: decision.peer_ca_der.clone(),
        peer_session_pubkey: decision.peer_session_pubkey,
        plane_endpoints: decision.plane_endpoints.clone(),
    })
}

fn verify_signature(
    node_id: &NodeId,
    signable: &[u8],
    sig: &SignatureBytes,
) -> Result<(), TrustError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let verifying_key =
        VerifyingKey::from_bytes(node_id.as_bytes()).map_err(|_| TrustError::InvalidSignature)?;
    let signature = Signature::from_bytes(sig.as_bytes());
    verifying_key
        .verify(signable, &signature)
        .map_err(|_| TrustError::InvalidSignature)
}


