use polar_identity::{NodeId, SignatureBytes, SyncSigner, UnixTimestamp};

use crate::canonical::signable::session_key_update_signable;
use crate::errors::ProtocolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionPublicKey([u8; 32]);

impl SessionPublicKey {
    pub fn from_hex(s: &str) -> Result<Self, ProtocolError> {
        let trimmed = s.trim();
        let bytes = hex::decode(trimmed).map_err(|_| ProtocolError::InvalidSignature)?;
        let raw: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ProtocolError::InvalidSignature)?;
        Ok(Self(raw))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKeyUpdate {
    pub node_id: NodeId,
    pub peer_node_id: NodeId,
    pub epoch: u64,
    pub ephemeral_pubkey: SessionPublicKey,
    pub expires_at: UnixTimestamp,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSessionKeyUpdate {
    pub node_id: NodeId,
    pub peer_node_id: NodeId,
    pub epoch: u64,
    pub ephemeral_pubkey: SessionPublicKey,
    pub expires_at: UnixTimestamp,
}

impl SessionKeyUpdate {
    pub fn sign(
        node_id: NodeId,
        peer_node_id: NodeId,
        epoch: u64,
        ephemeral_pubkey: SessionPublicKey,
        expires_at: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        if epoch == 0 {
            return Err(ProtocolError::InvalidEpoch);
        }
        let signable = session_key_update_signable(
            &node_id,
            &peer_node_id,
            epoch,
            &ephemeral_pubkey,
            expires_at,
        );
        Ok(Self {
            node_id,
            peer_node_id,
            epoch,
            ephemeral_pubkey,
            expires_at,
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        node_id: NodeId,
        peer_node_id: NodeId,
        epoch: u64,
        ephemeral_pubkey: SessionPublicKey,
        expires_at: UnixTimestamp,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        if epoch == 0 {
            return Err(ProtocolError::InvalidEpoch);
        }
        let signable = session_key_update_signable(
            &node_id,
            &peer_node_id,
            epoch,
            &ephemeral_pubkey,
            expires_at,
        );
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::SessionKeyUpdateV1,
            signable,
        )?;
        let sig = request
            .sign_with(node_id, signer, polar_identity::SignOptions::default())
            .await?;
        Ok(Self {
            node_id,
            peer_node_id,
            epoch,
            ephemeral_pubkey,
            expires_at,
            sig,
        })
    }
}

pub fn verify_session_key_update(
    update: &SessionKeyUpdate,
) -> Result<VerifiedSessionKeyUpdate, ProtocolError> {
    if update.epoch == 0 {
        return Err(ProtocolError::InvalidEpoch);
    }
    let signable = session_key_update_signable(
        &update.node_id,
        &update.peer_node_id,
        update.epoch,
        &update.ephemeral_pubkey,
        update.expires_at,
    );
    super::canonical::signable::verify_signature(&update.node_id, &signable, &update.sig)?;
    Ok(VerifiedSessionKeyUpdate {
        node_id: update.node_id,
        peer_node_id: update.peer_node_id,
        epoch: update.epoch,
        ephemeral_pubkey: update.ephemeral_pubkey,
        expires_at: update.expires_at,
    })
}


