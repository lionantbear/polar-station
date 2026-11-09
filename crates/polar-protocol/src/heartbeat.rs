use polar_identity::{
    BindingPublicKey, NodeId, NodeName, SignatureBytes, SyncSigner, UnixTimestamp,
};

use crate::canonical::signable::signed_heartbeat_signable;
use crate::errors::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedHeartbeat {
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub binding_pubkey: BindingPublicKey,
    pub timestamp: UnixTimestamp,
    pub status: String,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedHeartbeat {
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub binding_pubkey: BindingPublicKey,
    pub timestamp: UnixTimestamp,
    pub status: String,
}

impl SignedHeartbeat {
    pub fn sign(
        node_id: NodeId,
        node_name: NodeName,
        binding_pubkey: BindingPublicKey,
        timestamp: UnixTimestamp,
        status: impl Into<String>,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let status = validate_status(status.into())?;
        let signable =
            signed_heartbeat_signable(&node_id, &node_name, &binding_pubkey, timestamp, &status);
        Ok(Self {
            node_id,
            node_name,
            binding_pubkey,
            timestamp,
            status,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        node_id: NodeId,
        node_name: NodeName,
        binding_pubkey: BindingPublicKey,
        timestamp: UnixTimestamp,
        status: impl Into<String>,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let status = validate_status(status.into())?;
        let signable =
            signed_heartbeat_signable(&node_id, &node_name, &binding_pubkey, timestamp, &status);
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::HeartbeatV1,
            signable,
        )?;
        let sig = request
            .sign_with(node_id, signer, polar_identity::SignOptions::default())
            .await?;
        Ok(Self {
            node_id,
            node_name,
            binding_pubkey,
            timestamp,
            status,
            sig,
        })
    }
}

pub fn verify_signed_heartbeat(
    heartbeat: &SignedHeartbeat,
) -> Result<ValidatedHeartbeat, ProtocolError> {
    let status = validate_status(heartbeat.status.clone())?;
    let signable = signed_heartbeat_signable(
        &heartbeat.node_id,
        &heartbeat.node_name,
        &heartbeat.binding_pubkey,
        heartbeat.timestamp,
        &status,
    );
    super::canonical::signable::verify_signature(&heartbeat.node_id, &signable, &heartbeat.sig)?;
    Ok(ValidatedHeartbeat {
        node_id: heartbeat.node_id,
        node_name: heartbeat.node_name.clone(),
        binding_pubkey: heartbeat.binding_pubkey,
        timestamp: heartbeat.timestamp,
        status,
    })
}

fn validate_status(status: String) -> Result<String, ProtocolError> {
    let trimmed = status.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return Err(ProtocolError::InvalidStatus);
    }
    Ok(trimmed.to_string())
}


