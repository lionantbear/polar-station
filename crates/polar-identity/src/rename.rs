use crate::canonical::signable::rename_request_signable;
use crate::signed_identity::verify_signature;
use crate::{IdentityError, NodeId, NodeName, SignatureBytes, SyncSigner, UnixTimestamp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRequest {
    pub node_id: NodeId,
    pub new_name: NodeName,
    pub timestamp: UnixTimestamp,
    pub sig: SignatureBytes,
}

impl RenameRequest {
    pub fn sign(
        node_id: NodeId,
        new_name: NodeName,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, IdentityError> {
        if signer.node_id() != node_id {
            return Err(IdentityError::IdentityMismatch);
        }
        let signable = rename_request_signable(&node_id, &new_name, timestamp);
        Ok(Self {
            node_id,
            new_name,
            timestamp,
            sig: signer.sign(&signable),
        })
    }
}

pub fn verify_rename_request(req: &RenameRequest) -> Result<(), IdentityError> {
    let signable = rename_request_signable(&req.node_id, &req.new_name, req.timestamp);
    verify_signature(&req.node_id, &signable, &req.sig)
}


