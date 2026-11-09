use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use polar_identity::{
    BindingPublicKey, NodeId, NodeIdentity, NodeName, SignatureBytes, UnixTimestamp,
};
use sha2::{Digest, Sha256};

use crate::errors::ProtocolError;
use crate::session::SessionPublicKey;

pub(crate) fn signed_discovery_signable(
    identity: &NodeIdentity,
    timestamp: UnixTimestamp,
    intent: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 32 + 32 + 8 + 32);
    out.extend_from_slice(identity.node_id.as_bytes());
    out.extend_from_slice(&sha256(identity.node_name.as_str().as_bytes()));
    out.extend_from_slice(identity.binding_pubkey.as_bytes());
    out.extend_from_slice(&timestamp.as_secs().to_be_bytes());
    out.extend_from_slice(&sha256(intent.as_bytes()));
    out
}

pub(crate) fn signed_heartbeat_signable(
    node_id: &NodeId,
    node_name: &NodeName,
    binding_pubkey: &BindingPublicKey,
    timestamp: UnixTimestamp,
    status: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 32 + 32 + 8 + 32);
    out.extend_from_slice(node_id.as_bytes());
    out.extend_from_slice(&sha256(node_name.as_str().as_bytes()));
    out.extend_from_slice(binding_pubkey.as_bytes());
    out.extend_from_slice(&timestamp.as_secs().to_be_bytes());
    out.extend_from_slice(&sha256(status.as_bytes()));
    out
}

pub(crate) fn session_key_update_signable(
    node_id: &NodeId,
    peer_node_id: &NodeId,
    epoch: u64,
    ephemeral_pubkey: &SessionPublicKey,
    expires_at: UnixTimestamp,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 32 + 8 + 32 + 8);
    out.extend_from_slice(node_id.as_bytes());
    out.extend_from_slice(peer_node_id.as_bytes());
    out.extend_from_slice(&epoch.to_be_bytes());
    out.extend_from_slice(ephemeral_pubkey.as_bytes());
    out.extend_from_slice(&expires_at.as_secs().to_be_bytes());
    out
}

pub(crate) struct CapabilityDistributionSignable<'a> {
    pub sender_node_id: &'a NodeId,
    pub target_node_id: &'a NodeId,
    pub capability_name: &'a str,
    pub version: Option<&'a str>,
    pub timestamp: UnixTimestamp,
    pub force_install: bool,
    pub force_auth_overwrite: bool,
    pub auth_sha256_hex: &'a str,
}

pub(crate) fn capability_distribution_signable(
    input: CapabilityDistributionSignable<'_>,
) -> Vec<u8> {
    let auth_sha256 = hex::decode(input.auth_sha256_hex).expect("validated auth sha256");
    let mut out = Vec::with_capacity(32 + 32 + 32 + 32 + 8 + 1 + 1 + 32);
    out.extend_from_slice(input.sender_node_id.as_bytes());
    out.extend_from_slice(input.target_node_id.as_bytes());
    out.extend_from_slice(&sha256(input.capability_name.as_bytes()));
    out.extend_from_slice(&sha256(input.version.unwrap_or("").as_bytes()));
    out.extend_from_slice(&input.timestamp.as_secs().to_be_bytes());
    out.push(u8::from(input.force_install));
    out.push(u8::from(input.force_auth_overwrite));
    out.extend_from_slice(&auth_sha256);
    out
}

pub(crate) struct LgpRoutePacketSignable<'a> {
    pub route_kind: &'a str,
    pub sender_node_id: &'a NodeId,
    pub target_node_id: Option<&'a NodeId>,
    pub sender_plane: Option<&'a str>,
    pub receiver_plane: Option<&'a str>,
    pub owner_node_id: &'a NodeId,
    pub channel_id: &'a str,
    pub thread_id: &'a str,
    pub message_id: &'a str,
    pub message_kind: &'a str,
    pub intent: &'a str,
    pub payload_family: &'a str,
    pub payload_encoding: &'a str,
    pub payload_compression: &'a str,
    pub sealed_nonce_hex: &'a str,
    pub sealed_ciphertext_hex: &'a str,
    pub channel_owner_node_id: Option<&'a NodeId>,
    pub partner_node_id: Option<&'a NodeId>,
    pub original_sender_node_id: Option<&'a NodeId>,
    pub channel_title: Option<&'a str>,
    pub thread_tag: Option<&'a str>,
    pub created_at: UnixTimestamp,
    pub relay_ts: Option<UnixTimestamp>,
    pub dm_peer_receipt: Option<crate::DmAckSegment>,
}

pub(crate) fn signed_lgp_route_packet_signable(input: LgpRoutePacketSignable<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 * 6 + 32 * 8 + 18);
    out.extend_from_slice(&sha256(input.route_kind.as_bytes()));
    out.extend_from_slice(input.sender_node_id.as_bytes());
    extend_optional_node_id(&mut out, input.target_node_id);
    extend_optional_hash(&mut out, input.sender_plane);
    extend_optional_hash(&mut out, input.receiver_plane);
    out.extend_from_slice(input.owner_node_id.as_bytes());
    out.extend_from_slice(&sha256(input.channel_id.as_bytes()));
    out.extend_from_slice(&sha256(input.thread_id.as_bytes()));
    out.extend_from_slice(&sha256(input.message_id.as_bytes()));
    out.extend_from_slice(&sha256(input.message_kind.as_bytes()));
    out.extend_from_slice(&sha256(input.intent.as_bytes()));
    out.extend_from_slice(&sha256(input.payload_family.as_bytes()));
    out.extend_from_slice(&sha256(input.payload_encoding.as_bytes()));
    out.extend_from_slice(&sha256(input.payload_compression.as_bytes()));
    out.extend_from_slice(&sha256(input.sealed_nonce_hex.as_bytes()));
    out.extend_from_slice(&sha256(input.sealed_ciphertext_hex.as_bytes()));
    extend_optional_node_id(&mut out, input.channel_owner_node_id);
    extend_optional_node_id(&mut out, input.partner_node_id);
    extend_optional_node_id(&mut out, input.original_sender_node_id);
    extend_optional_hash(&mut out, input.channel_title);
    extend_optional_hash(&mut out, input.thread_tag);
    out.extend_from_slice(&input.created_at.as_secs().to_be_bytes());
    extend_optional_timestamp(&mut out, input.relay_ts);
    dm_peer_receipt_signable(&mut out, input.dm_peer_receipt.as_ref());
    out
}

pub(crate) fn dm_peer_receipt_signable(out: &mut Vec<u8>, ack: Option<&crate::DmAckSegment>) {
    match ack {
        Some(ack) => {
            out.push(1);
            out.extend_from_slice(&sha256(ack.previous_acked_message_id.as_bytes()));
            out.extend_from_slice(&sha256(ack.acked_message_id.as_bytes()));
            out.extend_from_slice(&ack.received_count.to_be_bytes());
        }
        None => out.push(0),
    }
}

pub(crate) fn verify_signature(
    node_id: &NodeId,
    signable: &[u8],
    sig: &SignatureBytes,
) -> Result<(), ProtocolError> {
    let verifying_key = VerifyingKey::from_bytes(node_id.as_bytes())
        .map_err(|_| ProtocolError::InvalidSignature)?;
    verify_signature_with_key(&verifying_key, signable, sig)
}

pub(crate) fn verify_signature_with_key(
    verifying_key: &VerifyingKey,
    signable: &[u8],
    sig: &SignatureBytes,
) -> Result<(), ProtocolError> {
    let signature = Signature::from_bytes(sig.as_bytes());
    verifying_key
        .verify(signable, &signature)
        .map_err(|_| ProtocolError::InvalidSignature)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn extend_optional_hash(out: &mut Vec<u8>, value: Option<&str>) {
    out.push(u8::from(value.is_some()));
    out.extend_from_slice(&sha256(value.unwrap_or("").as_bytes()));
}

fn extend_optional_node_id(out: &mut Vec<u8>, value: Option<&NodeId>) {
    out.push(u8::from(value.is_some()));
    match value {
        Some(node_id) => out.extend_from_slice(node_id.as_bytes()),
        None => out.extend_from_slice(&[0u8; 32]),
    }
}

fn extend_optional_timestamp(out: &mut Vec<u8>, value: Option<UnixTimestamp>) {
    out.push(u8::from(value.is_some()));
    out.extend_from_slice(
        &value
            .unwrap_or(UnixTimestamp::from_secs(0))
            .as_secs()
            .to_be_bytes(),
    );
}
