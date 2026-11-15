use crate::canonical::signable::binding_rebind_request_signable;
use crate::signed_identity::verify_signature;
use crate::{BindingPublicKey, IdentityError, NodeId, SignatureBytes, SyncSigner, UnixTimestamp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRebindRequest {
    pub node_id: NodeId,
    pub old_binding: BindingPublicKey,
    pub new_binding: BindingPublicKey,
    pub timestamp: UnixTimestamp,
    pub sig: SignatureBytes,
}

impl BindingRebindRequest {
    pub fn sign(
        node_id: NodeId,
        old_binding: BindingPublicKey,
        new_binding: BindingPublicKey,
        timestamp: UnixTimestamp,
        signer: &impl SyncSigner,
    ) -> Result<Self, IdentityError> {
        if signer.node_id() != node_id {
            return Err(IdentityError::IdentityMismatch);
        }
        let signable =
            binding_rebind_request_signable(&node_id, &old_binding, &new_binding, timestamp);
        Ok(Self {
            node_id,
            old_binding,
            new_binding,
            timestamp,
            sig: signer.sign(&signable),
        })
    }
}

pub fn verify_binding_rebind_request(req: &BindingRebindRequest) -> Result<(), IdentityError> {
    let signable = binding_rebind_request_signable(
        &req.node_id,
        &req.old_binding,
        &req.new_binding,
        req.timestamp,
    );
    verify_signature(&req.node_id, &signable, &req.sig)
}


