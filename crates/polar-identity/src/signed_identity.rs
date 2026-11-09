use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::canonical::signable::signed_node_identity_signable;
use crate::{
    IdentityError, NodeId, NodeIdentity, SignatureBytes, SignatureVerifier, SyncSigner,
    UnixTimestamp,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedNodeIdentity {
    pub identity: NodeIdentity,
    pub timestamp: UnixTimestamp,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedNodeIdentity {
    pub identity: NodeIdentity,
    pub timestamp: UnixTimestamp,
}

impl SignedNodeIdentity {
    pub fn sign(
        identity: NodeIdentity,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, IdentityError> {
        if signer.node_id() != identity.node_id {
            return Err(IdentityError::IdentityMismatch);
        }
        let signable = signed_node_identity_signable(
            &identity.node_id,
            &identity.node_name,
            &identity.binding_pubkey,
            timestamp,
        );
        Ok(Self {
            identity,
            timestamp,
            sig: signer.sign(&signable),
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NodeIdSignatureVerifier;

impl SignatureVerifier for NodeIdSignatureVerifier {
    fn verify_signature(
        &self,
        node_id: &NodeId,
        signable: &[u8],
        sig: &SignatureBytes,
    ) -> Result<(), IdentityError> {
        let verifying_key = VerifyingKey::from_bytes(node_id.as_bytes())
            .map_err(|_| IdentityError::InvalidSignature)?;
        let signature = Signature::from_bytes(sig.as_bytes());
        verifying_key
            .verify(signable, &signature)
            .map_err(|_| IdentityError::InvalidSignature)
    }
}

pub fn verify_signed_node_identity(
    signed: &SignedNodeIdentity,
) -> Result<VerifiedNodeIdentity, IdentityError> {
    let signable = signed_node_identity_signable(
        &signed.identity.node_id,
        &signed.identity.node_name,
        &signed.identity.binding_pubkey,
        signed.timestamp,
    );
    NodeIdSignatureVerifier.verify_signature(&signed.identity.node_id, &signable, &signed.sig)?;
    Ok(VerifiedNodeIdentity {
        identity: signed.identity.clone(),
        timestamp: signed.timestamp,
    })
}

pub(crate) fn verify_signature(
    node_id: &NodeId,
    signable: &[u8],
    sig: &SignatureBytes,
) -> Result<(), IdentityError> {
    NodeIdSignatureVerifier.verify_signature(node_id, signable, sig)
}


