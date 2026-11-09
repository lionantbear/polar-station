use crate::canonical::encode::hash_node_name;
use crate::{BindingPublicKey, NodeId, NodeName, UnixTimestamp};

pub(crate) fn signed_node_identity_signable(
    node_id: &NodeId,
    node_name: &NodeName,
    binding: &BindingPublicKey,
    timestamp: UnixTimestamp,
) -> [u8; 104] {
    let mut out = [0u8; 104];
    out[..32].copy_from_slice(node_id.as_bytes());
    out[32..64].copy_from_slice(&hash_node_name(node_name));
    out[64..96].copy_from_slice(binding.as_bytes());
    out[96..].copy_from_slice(&timestamp.as_secs().to_be_bytes());
    out
}

pub(crate) fn rename_request_signable(
    node_id: &NodeId,
    new_name: &NodeName,
    timestamp: UnixTimestamp,
) -> [u8; 72] {
    let mut out = [0u8; 72];
    out[..32].copy_from_slice(node_id.as_bytes());
    out[32..64].copy_from_slice(&hash_node_name(new_name));
    out[64..].copy_from_slice(&timestamp.as_secs().to_be_bytes());
    out
}

pub(crate) fn binding_rebind_request_signable(
    node_id: &NodeId,
    old_binding: &BindingPublicKey,
    new_binding: &BindingPublicKey,
    timestamp: UnixTimestamp,
) -> [u8; 104] {
    let mut out = [0u8; 104];
    out[..32].copy_from_slice(node_id.as_bytes());
    out[32..64].copy_from_slice(old_binding.as_bytes());
    out[64..96].copy_from_slice(new_binding.as_bytes());
    out[96..].copy_from_slice(&timestamp.as_secs().to_be_bytes());
    out
}
