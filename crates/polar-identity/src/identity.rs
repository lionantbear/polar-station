use crate::{BindingPublicKey, NodeId, NodeName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentity {
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub binding_pubkey: BindingPublicKey,
}
