use ed25519_dalek::VerifyingKey;
use polar_identity::{NodeId, SignatureBytes, SyncSigner, UnixTimestamp};

use crate::canonical::signable::{
    signed_lgp_route_packet_signable, verify_signature_with_key, LgpRoutePacketSignable,
};
use crate::errors::ProtocolError;
use crate::DmAckSegment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LgpRouteKind {
    Post,
    Direct,
    Relay,
}

impl LgpRouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Direct => "direct",
            Self::Relay => "relay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedLgpRoutePacket {
    pub route_kind: LgpRouteKind,
    pub sender_node_id: NodeId,
    pub target_node_id: Option<NodeId>,
    pub sender_plane: Option<String>,
    pub receiver_plane: Option<String>,
    pub owner_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub message_kind: String,
    pub intent: String,
    pub payload_family: String,
    pub payload_encoding: String,
    pub payload_compression: String,
    pub sealed_nonce_hex: String,
    pub sealed_ciphertext_hex: String,
    pub channel_owner_node_id: Option<NodeId>,
    pub partner_node_id: Option<NodeId>,
    pub original_sender_node_id: Option<NodeId>,
    pub channel_title: Option<String>,
    pub thread_tag: Option<String>,
    pub created_at: UnixTimestamp,
    pub relay_ts: Option<UnixTimestamp>,
    pub dm_peer_receipt: Option<DmAckSegment>,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedLgpRoutePacket {
    pub route_kind: LgpRouteKind,
    pub sender_node_id: NodeId,
    pub target_node_id: Option<NodeId>,
    pub sender_plane: Option<String>,
    pub receiver_plane: Option<String>,
    pub owner_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub message_kind: String,
    pub intent: String,
    pub payload_family: String,
    pub payload_encoding: String,
    pub payload_compression: String,
    pub sealed_nonce_hex: String,
    pub sealed_ciphertext_hex: String,
    pub channel_owner_node_id: Option<NodeId>,
    pub partner_node_id: Option<NodeId>,
    pub original_sender_node_id: Option<NodeId>,
    pub channel_title: Option<String>,
    pub thread_tag: Option<String>,
    pub created_at: UnixTimestamp,
    pub relay_ts: Option<UnixTimestamp>,
    pub dm_peer_receipt: Option<DmAckSegment>,
}

pub struct LgpRoutePacketSigningInput {
    pub route_kind: LgpRouteKind,
    pub sender_node_id: NodeId,
    pub target_node_id: Option<NodeId>,
    pub sender_plane: Option<String>,
    pub receiver_plane: Option<String>,
    pub owner_node_id: NodeId,
    pub channel_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub message_kind: String,
    pub intent: String,
    pub payload_family: String,
    pub payload_encoding: String,
    pub payload_compression: String,
    pub sealed_nonce_hex: String,
    pub sealed_ciphertext_hex: String,
    pub channel_owner_node_id: Option<NodeId>,
    pub partner_node_id: Option<NodeId>,
    pub original_sender_node_id: Option<NodeId>,
    pub channel_title: Option<String>,
    pub thread_tag: Option<String>,
    pub created_at: UnixTimestamp,
    pub relay_ts: Option<UnixTimestamp>,
    pub dm_peer_receipt: Option<DmAckSegment>,
}

impl SignedLgpRoutePacket {
    pub fn sign(
        input: LgpRoutePacketSigningInput,
        signer: &impl SyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.node_id() != input.sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let normalized = NormalizedWireEnvelope::from_input(input)?;
        let signable = normalized.signable();
        Ok(Self {
            route_kind: normalized.route_kind,
            sender_node_id: normalized.sender_node_id,
            target_node_id: normalized.target_node_id,
            sender_plane: normalized.sender_plane,
            receiver_plane: normalized.receiver_plane,
            owner_node_id: normalized.owner_node_id,
            channel_id: normalized.channel_id,
            thread_id: normalized.thread_id,
            message_id: normalized.message_id,
            message_kind: normalized.message_kind,
            intent: normalized.intent,
            payload_family: normalized.payload_family,
            payload_encoding: normalized.payload_encoding,
            payload_compression: normalized.payload_compression,
            sealed_nonce_hex: normalized.sealed_nonce_hex,
            sealed_ciphertext_hex: normalized.sealed_ciphertext_hex,
            channel_owner_node_id: normalized.channel_owner_node_id,
            partner_node_id: normalized.partner_node_id,
            original_sender_node_id: normalized.original_sender_node_id,
            channel_title: normalized.channel_title,
            thread_tag: normalized.thread_tag,
            created_at: normalized.created_at,
            relay_ts: normalized.relay_ts,
            dm_peer_receipt: normalized.dm_peer_receipt.clone(),
            sig: signer.sign(&signable),
        })
    }

    /// Asynchronous private-key operation over the existing canonical format.
    /// A rejected or mismatched backend response never becomes a signed object.
    pub async fn sign_async(
        input: LgpRoutePacketSigningInput,
        signer: &dyn polar_identity::AsyncSigner,
    ) -> Result<Self, ProtocolError> {
        if signer.descriptor().node_id != input.sender_node_id {
            return Err(ProtocolError::IdentityMismatch);
        }
        let normalized = NormalizedWireEnvelope::from_input(input)?;
        let signable = normalized.signable();
        let request = polar_identity::SignRequest::new(
            polar_identity::SigningPurpose::LegacyLgpRouteV1,
            signable,
        )?;
        let sig = request
            .sign_with(
                normalized.sender_node_id,
                signer,
                polar_identity::SignOptions::default(),
            )
            .await?;
        Ok(Self {
            route_kind: normalized.route_kind,
            sender_node_id: normalized.sender_node_id,
            target_node_id: normalized.target_node_id,
            sender_plane: normalized.sender_plane,
            receiver_plane: normalized.receiver_plane,
            owner_node_id: normalized.owner_node_id,
            channel_id: normalized.channel_id,
            thread_id: normalized.thread_id,
            message_id: normalized.message_id,
            message_kind: normalized.message_kind,
            intent: normalized.intent,
            payload_family: normalized.payload_family,
            payload_encoding: normalized.payload_encoding,
            payload_compression: normalized.payload_compression,
            sealed_nonce_hex: normalized.sealed_nonce_hex,
            sealed_ciphertext_hex: normalized.sealed_ciphertext_hex,
            channel_owner_node_id: normalized.channel_owner_node_id,
            partner_node_id: normalized.partner_node_id,
            original_sender_node_id: normalized.original_sender_node_id,
            channel_title: normalized.channel_title,
            thread_tag: normalized.thread_tag,
            created_at: normalized.created_at,
            relay_ts: normalized.relay_ts,
            dm_peer_receipt: normalized.dm_peer_receipt.clone(),
            sig,
        })
    }
}

pub fn verify_signed_lgp_route_packet(
    envelope: &SignedLgpRoutePacket,
) -> Result<VerifiedLgpRoutePacket, ProtocolError> {
    let verifying_key = VerifyingKey::from_bytes(envelope.sender_node_id.as_bytes())
        .map_err(|_| ProtocolError::InvalidSignature)?;
    verify_signed_lgp_route_packet_with_key(envelope, &verifying_key)
}

pub fn verify_signed_lgp_route_packet_with_key(
    envelope: &SignedLgpRoutePacket,
    verifying_key: &VerifyingKey,
) -> Result<VerifiedLgpRoutePacket, ProtocolError> {
    let normalized = NormalizedWireEnvelope::from_signed(envelope)?;
    let signable = normalized.signable();
    verify_signature_with_key(verifying_key, &signable, &envelope.sig)?;
    Ok(VerifiedLgpRoutePacket {
        route_kind: normalized.route_kind,
        sender_node_id: normalized.sender_node_id,
        target_node_id: normalized.target_node_id,
        sender_plane: normalized.sender_plane,
        receiver_plane: normalized.receiver_plane,
        owner_node_id: normalized.owner_node_id,
        channel_id: normalized.channel_id,
        thread_id: normalized.thread_id,
        message_id: normalized.message_id,
        message_kind: normalized.message_kind,
        intent: normalized.intent,
        payload_family: normalized.payload_family,
        payload_encoding: normalized.payload_encoding,
        payload_compression: normalized.payload_compression,
        sealed_nonce_hex: normalized.sealed_nonce_hex,
        sealed_ciphertext_hex: normalized.sealed_ciphertext_hex,
        channel_owner_node_id: normalized.channel_owner_node_id,
        partner_node_id: normalized.partner_node_id,
        original_sender_node_id: normalized.original_sender_node_id,
        channel_title: normalized.channel_title,
        thread_tag: normalized.thread_tag,
        created_at: normalized.created_at,
        relay_ts: normalized.relay_ts,
        dm_peer_receipt: normalized.dm_peer_receipt.clone(),
    })
}

#[derive(Debug, Clone)]
struct NormalizedWireEnvelope {
    route_kind: LgpRouteKind,
    sender_node_id: NodeId,
    target_node_id: Option<NodeId>,
    sender_plane: Option<String>,
    receiver_plane: Option<String>,
    owner_node_id: NodeId,
    channel_id: String,
    thread_id: String,
    message_id: String,
    message_kind: String,
    intent: String,
    payload_family: String,
    payload_encoding: String,
    payload_compression: String,
    sealed_nonce_hex: String,
    sealed_ciphertext_hex: String,
    channel_owner_node_id: Option<NodeId>,
    partner_node_id: Option<NodeId>,
    original_sender_node_id: Option<NodeId>,
    channel_title: Option<String>,
    thread_tag: Option<String>,
    created_at: UnixTimestamp,
    relay_ts: Option<UnixTimestamp>,
    dm_peer_receipt: Option<DmAckSegment>,
}

impl NormalizedWireEnvelope {
    fn from_input(input: LgpRoutePacketSigningInput) -> Result<Self, ProtocolError> {
        Self::normalize(
            input.route_kind,
            input.sender_node_id,
            input.target_node_id,
            input.sender_plane,
            input.receiver_plane,
            input.owner_node_id,
            input.channel_id,
            input.thread_id,
            input.message_id,
            input.message_kind,
            input.intent,
            input.payload_family,
            input.payload_encoding,
            input.payload_compression,
            input.sealed_nonce_hex,
            input.sealed_ciphertext_hex,
            input.channel_owner_node_id,
            input.partner_node_id,
            input.original_sender_node_id,
            input.channel_title,
            input.thread_tag,
            input.created_at,
            input.relay_ts,
            input.dm_peer_receipt,
        )
    }

    fn from_signed(envelope: &SignedLgpRoutePacket) -> Result<Self, ProtocolError> {
        Self::normalize(
            envelope.route_kind,
            envelope.sender_node_id,
            envelope.target_node_id,
            envelope.sender_plane.clone(),
            envelope.receiver_plane.clone(),
            envelope.owner_node_id,
            envelope.channel_id.clone(),
            envelope.thread_id.clone(),
            envelope.message_id.clone(),
            envelope.message_kind.clone(),
            envelope.intent.clone(),
            envelope.payload_family.clone(),
            envelope.payload_encoding.clone(),
            envelope.payload_compression.clone(),
            envelope.sealed_nonce_hex.clone(),
            envelope.sealed_ciphertext_hex.clone(),
            envelope.channel_owner_node_id,
            envelope.partner_node_id,
            envelope.original_sender_node_id,
            envelope.channel_title.clone(),
            envelope.thread_tag.clone(),
            envelope.created_at,
            envelope.relay_ts,
            envelope.dm_peer_receipt.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn normalize(
        route_kind: LgpRouteKind,
        sender_node_id: NodeId,
        target_node_id: Option<NodeId>,
        sender_plane: Option<String>,
        receiver_plane: Option<String>,
        owner_node_id: NodeId,
        channel_id: String,
        thread_id: String,
        message_id: String,
        message_kind: String,
        intent: String,
        payload_family: String,
        payload_encoding: String,
        payload_compression: String,
        sealed_nonce_hex: String,
        sealed_ciphertext_hex: String,
        channel_owner_node_id: Option<NodeId>,
        partner_node_id: Option<NodeId>,
        original_sender_node_id: Option<NodeId>,
        channel_title: Option<String>,
        thread_tag: Option<String>,
        created_at: UnixTimestamp,
        relay_ts: Option<UnixTimestamp>,
        dm_peer_receipt: Option<DmAckSegment>,
    ) -> Result<Self, ProtocolError> {
        let channel_id = validate_channel_id(&channel_id)?;
        let sender_plane = validate_optional_presentation(sender_plane.as_deref())?;
        let receiver_plane = validate_optional_presentation(receiver_plane.as_deref())?;
        let thread_id = validate_identifier(&thread_id)?;
        let message_id = validate_identifier(&message_id)?;
        let message_kind = validate_message_kind(&message_kind)?;
        let intent = validate_text(&intent)?;
        let payload_family = validate_payload_family(&payload_family)?;
        let payload_encoding = validate_payload_encoding(&payload_encoding)?;
        let payload_compression = validate_payload_compression(&payload_compression)?;
        let sealed_nonce_hex = validate_hex_bytes(&sealed_nonce_hex, 12)?;
        let sealed_ciphertext_hex = validate_hex_ciphertext(&sealed_ciphertext_hex)?;
        let channel_title = validate_optional_presentation(channel_title.as_deref())?;
        let thread_tag = validate_optional_presentation(thread_tag.as_deref())?;

        match route_kind {
            LgpRouteKind::Post => {
                if target_node_id.is_some()
                    || channel_owner_node_id.is_some()
                    || partner_node_id.is_some()
                    || original_sender_node_id.is_some()
                    || relay_ts.is_some()
                    || channel_title.is_some()
                    || thread_tag.is_some()
                    || dm_peer_receipt.is_some()
                    || sender_plane.is_some()
                    || receiver_plane.is_some()
                {
                    return Err(ProtocolError::InvalidMessage);
                }
            }
            LgpRouteKind::Direct => {
                let channel_owner = channel_owner_node_id.ok_or(ProtocolError::InvalidMessage)?;
                let partner = partner_node_id.ok_or(ProtocolError::InvalidMessage)?;
                if target_node_id.is_none()
                    || original_sender_node_id.is_some()
                    || relay_ts.is_some()
                {
                    return Err(ProtocolError::InvalidMessage);
                }
                if !is_valid_direct_message_channel_id(&channel_id, channel_owner, partner) {
                    return Err(ProtocolError::InvalidMessage);
                }
                if sender_plane.as_deref().is_some_and(|plane| {
                    direct_message_channel_access_plane(&channel_id) != Some(plane)
                }) {
                    return Err(ProtocolError::InvalidMessage);
                }
            }
            LgpRouteKind::Relay => {
                if target_node_id.is_none()
                    || original_sender_node_id.is_none()
                    || relay_ts.is_none()
                    || dm_peer_receipt.is_some()
                    || sender_plane.is_some()
                    || receiver_plane.is_some()
                {
                    return Err(ProtocolError::InvalidMessage);
                }
                if channel_owner_node_id.is_some_and(|channel_owner| channel_owner != owner_node_id)
                {
                    return Err(ProtocolError::InvalidMessage);
                }
            }
        }

        Ok(Self {
            route_kind,
            sender_node_id,
            target_node_id,
            sender_plane,
            receiver_plane,
            owner_node_id,
            channel_id,
            thread_id,
            message_id,
            message_kind,
            intent,
            payload_family,
            payload_encoding,
            payload_compression,
            sealed_nonce_hex,
            sealed_ciphertext_hex,
            channel_owner_node_id,
            partner_node_id,
            original_sender_node_id,
            channel_title,
            thread_tag,
            created_at,
            relay_ts,
            dm_peer_receipt,
        })
    }

    fn signable(&self) -> Vec<u8> {
        signed_lgp_route_packet_signable(LgpRoutePacketSignable {
            route_kind: self.route_kind.as_str(),
            sender_node_id: &self.sender_node_id,
            target_node_id: self.target_node_id.as_ref(),
            sender_plane: self.sender_plane.as_deref(),
            receiver_plane: self.receiver_plane.as_deref(),
            owner_node_id: &self.owner_node_id,
            channel_id: &self.channel_id,
            thread_id: &self.thread_id,
            message_id: &self.message_id,
            message_kind: &self.message_kind,
            intent: &self.intent,
            payload_family: &self.payload_family,
            payload_encoding: &self.payload_encoding,
            payload_compression: &self.payload_compression,
            sealed_nonce_hex: &self.sealed_nonce_hex,
            sealed_ciphertext_hex: &self.sealed_ciphertext_hex,
            channel_owner_node_id: self.channel_owner_node_id.as_ref(),
            partner_node_id: self.partner_node_id.as_ref(),
            original_sender_node_id: self.original_sender_node_id.as_ref(),
            channel_title: self.channel_title.as_deref(),
            thread_tag: self.thread_tag.as_deref(),
            created_at: self.created_at,
            relay_ts: self.relay_ts,
            dm_peer_receipt: self.dm_peer_receipt.clone(),
        })
    }
}

fn is_valid_direct_message_channel_id(
    channel_id: &str,
    channel_owner: NodeId,
    partner: NodeId,
) -> bool {
    // Strict v1.5 DM channel format: dm/{access_plane}/{owner_node_id}/{partner_node_id}.
    let Some(rest) = channel_id.strip_prefix("dm/") else {
        return false;
    };
    let parts = rest.split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return false;
    }
    let access_plane_id = parts[0];
    !access_plane_id.is_empty()
        && access_plane_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
        && parts[1] == channel_owner.to_hex()
        && parts[2] == partner.to_hex()
}

fn direct_message_channel_access_plane(channel_id: &str) -> Option<&str> {
    let rest = channel_id.strip_prefix("dm/")?;
    let mut parts = rest.split('/');
    let plane = parts.next()?;
    (!plane.is_empty()).then_some(plane)
}

fn validate_identifier(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 128 {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(trimmed.to_string())
}

fn validate_channel_id(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 256 {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(trimmed.to_string())
}

fn validate_text(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 128 {
        return Err(ProtocolError::InvalidIntent);
    }
    Ok(trimmed.to_string())
}

fn validate_message_kind(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    match trimmed {
        "post" | "task" => Ok(trimmed.to_string()),
        _ => Err(ProtocolError::InvalidMessage),
    }
}

fn validate_payload_family(value: &str) -> Result<String, ProtocolError> {
    validate_known_value(value, &["text", "json", "binary"])
}

fn validate_payload_encoding(value: &str) -> Result<String, ProtocolError> {
    validate_known_value(value, &["utf8", "json", "binary"])
}

fn validate_payload_compression(value: &str) -> Result<String, ProtocolError> {
    validate_known_value(value, &["none", "zstd"])
}

fn validate_known_value(value: &str, allowed: &[&str]) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    if allowed.contains(&trimmed) {
        Ok(trimmed.to_string())
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}

fn validate_hex_bytes(value: &str, expected_len: usize) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    let bytes = hex::decode(trimmed).map_err(|_| ProtocolError::InvalidMessage)?;
    if bytes.len() != expected_len {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(trimmed.to_string())
}

fn validate_hex_ciphertext(value: &str) -> Result<String, ProtocolError> {
    let trimmed = value.trim();
    let bytes = hex::decode(trimmed).map_err(|_| ProtocolError::InvalidMessage)?;
    if bytes.is_empty() {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(trimmed.to_string())
}

fn validate_optional_presentation(value: Option<&str>) -> Result<Option<String>, ProtocolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().any(|c| c.is_control()) || trimmed.len() > 256 {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(Some(trimmed.to_string()))
}


