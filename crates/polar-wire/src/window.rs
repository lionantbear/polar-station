//! Directed PWP2 protection 4. Binding and expanded stream context are obtained
//! from the authenticated association and are not serialized in the ciphertext.
use crate::*;

pub const WINDOW_PROOF_LEN: usize = 346;
pub const WINDOW_COMMITMENT_LEN: usize = WINDOW_PROOF_LEN - 64;
pub const WINDOW_BODY_HEADER: usize = 45;
pub const WINDOW_MAX_PLAIN: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct WindowSealedBody {
    pub window: u64,
    pub first: u8,
    pub count: u8,
    pub ephemeral: [u8; 32],
    pub ciphertext: Vec<u8>,
    #[serde(skip)]
    pub(crate) binding: Option<[u8; 32]>,
}

impl WindowSealedBody {
    /// The recipient key resolved from the current association, absent immediately after decoding.
    pub fn recipient_binding(&self) -> Option<[u8; 32]> {
        self.binding
    }

    pub fn new(
        window: u64,
        first: u8,
        count: u8,
        ephemeral: [u8; 32],
        ciphertext: Vec<u8>,
    ) -> WireResult<Self> {
        let body = Self {
            window,
            first,
            count,
            ephemeral,
            ciphertext,
            binding: None,
        };
        body.validate()?;
        Ok(body)
    }

    pub(crate) fn validate(&self) -> WireResult<()> {
        if self.count == 0
            || u16::from(self.first) + u16::from(self.count) > 32
            || self.window > u64::MAX / 32
            || self.ephemeral == [0; 32]
            || !(16..=WINDOW_MAX_PLAIN + 16).contains(&self.ciphertext.len())
        {
            return Err(WireError::Codec("invalid directed window body".into()));
        }
        Ok(())
    }

    pub(crate) fn validate_header(&self, kind: PacketKind, h: &PolarWireHeader) -> WireResult<()> {
        self.validate()?;
        if !matches!(
            kind,
            PacketKind::CompactData | PacketKind::StreamContextOpen
        ) || h.version != WIRE_PACKET_DIGEST_VERSION
            || h.body_kind_code != BODY_KIND_LGP_MESSAGE_BATCH
            || h.encoding_code != ENCODING_BINARY
            || h.compression_code != COMPRESSION_NONE
            || h.protection_mode_code != PROTECTION_WINDOW_SEALED
            || h.association_id == [0; 32]
            || h.seq.checked_sub(h.previous_seq) != Some(self.count.into())
            || h.context_epoch.is_none()
            || !h
                .context_id
                .as_deref()
                .is_some_and(|s| s.parse::<u64>().is_ok_and(|n| n.to_string() == s))
        {
            return Err(WireError::Codec("invalid directed window header".into()));
        }
        Ok(())
    }
}

impl PolarWirePacket {
    /// Supply the identities, plane and Binding key verified for this stream.
    pub fn bind_window_context(
        &mut self,
        sender: &[u8; 32],
        recipient: &[u8; 32],
        plane: AccessPlaneRef,
        binding: [u8; 32],
    ) -> WireResult<()> {
        let WireBodyBox::WindowSealed(body) = &mut self.body_box else {
            return Err(WireError::Codec("window ciphertext required".into()));
        };
        let h = &mut self.header;
        let sender = hex::encode(sender);
        let recipient = hex::encode(recipient);
        if binding == [0; 32]
            || sender == recipient
            || h.delivery_lane_id != delivery_lane_ref_for_parts(&sender, Some(&recipient), &plane)
            || (self.packet_kind == PacketKind::StreamContextOpen
                && (h.sender_node_id != sender
                    || h.target_node_id.as_ref() != Some(&recipient)
                    || h.access_plane_ref != plane
                    || h.stream_id_ref != stream_id_ref_for_id(&h.stream_id)))
        {
            return Err(WireError::Codec(
                "window association identity or lane mismatch".into(),
            ));
        }
        body.validate_header(self.packet_kind, h)?;
        h.sender_node_id = sender;
        h.target_node_id = Some(recipient);
        h.access_plane_ref = plane;
        body.binding = Some(binding);
        Ok(())
    }

    pub fn window_proof(&self) -> WireResult<[u8; WINDOW_PROOF_LEN]> {
        let WireBodyBox::WindowSealed(body) = &self.body_box else {
            return Err(WireError::Codec("window ciphertext required".into()));
        };
        let signature: &[u8; 64] = self
            .packet_signature
            .as_slice()
            .try_into()
            .map_err(|_| WireError::Codec("window signature must be 64 bytes".into()))?;
        let mut proof = [0; WINDOW_PROOF_LEN];
        proof[..WINDOW_COMMITMENT_LEN].copy_from_slice(&window_commitment(
            self.packet_kind,
            &self.header,
            body,
        )?);
        proof[WINDOW_COMMITMENT_LEN..].copy_from_slice(signature);
        Ok(proof)
    }
}

pub(crate) fn window_commitment(
    kind: PacketKind,
    h: &PolarWireHeader,
    body: &WindowSealedBody,
) -> WireResult<[u8; WINDOW_COMMITMENT_LEN]> {
    body.validate_header(kind, h)?;
    let binding = body
        .binding
        .ok_or_else(|| WireError::Codec("window recipient Binding is unresolved".into()))?;
    let mut proof = [0u8; WINDOW_COMMITMENT_LEN];
    proof[..8].copy_from_slice(b"PWRW0001");
    let invalid_id = || WireError::Codec("invalid window identity".into());
    hex::decode_to_slice(&h.sender_node_id, &mut proof[8..40]).map_err(|_| invalid_id())?;
    hex::decode_to_slice(
        h.target_node_id.as_ref().ok_or_else(invalid_id)?,
        &mut proof[40..72],
    )
    .map_err(|_| invalid_id())?;
    proof[72..104].copy_from_slice(&h.association_id);
    proof[104..120].copy_from_slice(&h.access_plane_ref);
    proof[120..136].copy_from_slice(&h.delivery_lane_id);
    proof[136..144].copy_from_slice(&h.seq.to_be_bytes());
    proof[144..152].copy_from_slice(&body.window.to_be_bytes());
    proof[152] = body.first;
    proof[153] = body.count;
    proof[154..186].copy_from_slice(&binding);
    proof[186..218].copy_from_slice(&body.ephemeral);
    let header = crate::codec::encode_header(kind, h)?;
    let prelude = crate::codec::prelude(
        h.version,
        kind,
        header.len(),
        WINDOW_BODY_HEADER + body.ciphertext.len(),
        64,
    )?;
    use sha2::{Digest, Sha256};
    proof[218..250].copy_from_slice(
        &Sha256::new()
            .chain_update(prelude)
            .chain_update(header)
            .finalize(),
    );
    proof[250..].copy_from_slice(&sha256_bytes(&body.ciphertext));
    Ok(proof)
}
