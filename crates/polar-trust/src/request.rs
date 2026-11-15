use polar_identity::{
    BindingPublicKey, NodeId, NodeName, SignatureBytes, SyncSigner, UnixTimestamp,
};
use polar_transport::AccessPlaneEndpointHints;

use crate::canonical::signable::{trust_request_signable, TrustRequestSignableParts};
use crate::errors::TrustError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRequest {
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
    pub sig: SignatureBytes,
}

impl TrustRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material(
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            None,
            None,
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_ca(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material(
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            peer_ca_der,
            None,
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_material(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material_and_endpoints(
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            AccessPlaneEndpointHints::new(),
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_material_and_endpoints(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        plane_endpoints: AccessPlaneEndpointHints,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        Self::sign_with_peer_material_endpoints_and_admission(
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            plane_endpoints,
            None,
            signer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_peer_material_endpoints_and_admission(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        plane_endpoints: AccessPlaneEndpointHints,
        admission_token: Option<String>,
        signer: &impl SyncSigner,
    ) -> Result<Self, TrustError> {
        if signer.node_id() != sender_node_id {
            return Err(TrustError::IdentityMismatch);
        }
        if request_id.trim().is_empty() {
            return Err(TrustError::InvalidRequest);
        }
        let signable = trust_request_signable(TrustRequestSignableParts {
            request_id: &request_id,
            sender_node_id: &sender_node_id,
            sender_node_name: &sender_node_name,
            sender_binding_pubkey: &sender_binding_pubkey,
            receiver_node_id: &receiver_node_id,
            sender_plane: &sender_plane,
            receiver_plane: &receiver_plane,
            timestamp,
            peer_ca_der: peer_ca_der.as_deref(),
            peer_session_pubkey: peer_session_pubkey.as_ref(),
            plane_endpoints: &plane_endpoints,
            admission_token: admission_token.as_deref(),
        });
        Ok(Self {
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            plane_endpoints,
            admission_token,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    #[allow(clippy::too_many_arguments)]
    pub async fn sign_with_peer_material_endpoints_and_admission_async(
        request_id: String,
        sender_node_id: NodeId,
        sender_node_name: NodeName,
        sender_binding_pubkey: BindingPublicKey,
        receiver_node_id: NodeId,
        sender_plane: String,
        receiver_plane: String,
        timestamp: UnixTimestamp,
        peer_ca_der: Option<Vec<u8>>,
        peer_session_pubkey: Option<[u8; 32]>,
        plane_endpoints: AccessPlaneEndpointHints,
        admission_token: Option<String>,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, TrustError> {
        if signer.descriptor().node_id != sender_node_id {
            return Err(TrustError::IdentityMismatch);
        }
        if request_id.trim().is_empty() {
            return Err(TrustError::InvalidRequest);
        }
        let signable = trust_request_signable(TrustRequestSignableParts {
            request_id: &request_id,
            sender_node_id: &sender_node_id,
            sender_node_name: &sender_node_name,
            sender_binding_pubkey: &sender_binding_pubkey,
            receiver_node_id: &receiver_node_id,
            sender_plane: &sender_plane,
            receiver_plane: &receiver_plane,
            timestamp,
            peer_ca_der: peer_ca_der.as_deref(),
            peer_session_pubkey: peer_session_pubkey.as_ref(),
            plane_endpoints: &plane_endpoints,
            admission_token: admission_token.as_deref(),
        });
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::TrustRequestV1,
            signable,
        )?;
        let sig = request
            .sign_with(
                sender_node_id,
                signer,
                polar_identity::SignOptions::default(),
            )
            .await?;
        Ok(Self {
            request_id,
            sender_node_id,
            sender_node_name,
            sender_binding_pubkey,
            receiver_node_id,
            sender_plane,
            receiver_plane,
            timestamp,
            peer_ca_der,
            peer_session_pubkey,
            plane_endpoints,
            admission_token,
            sig,
        })
    }
}
