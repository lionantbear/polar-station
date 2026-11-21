use crate::{
    sha256_bytes, signable::SignablePacket, ClearSignedBody, PacketKind, PeerSessionSealedBody,
    PolarWireHeader, PolarWirePacket, RealmSealedBody, RecipientKeySealedBody, WireBodyBox,
    WireError, WireResult, COMPRESSION_NONE, ENCODING_CBOR, WIRE_PACKET_DIGEST_VERSION,
    WIRE_PACKET_VERSION,
};

const PWP2_MAGIC: &[u8; 4] = b"PWP2";
const PWP2_PRELUDE_LEN: usize = 15;
// PWP2 packet kinds in this fixed binary codec: FullData, StreamContextOpen,
// CompactData. StreamContextOpen records packet_stream_context state outside
// CompactData; CompactData omits access_plane_ref and resolves it by context.

pub fn encode_packet(packet: &PolarWirePacket) -> WireResult<Vec<u8>> {
    encode_packet_parts(
        packet.packet_kind,
        &packet.header,
        &packet.body_box,
        Some(&packet.packet_signature),
    )
}

pub fn decode_packet(bytes: &[u8]) -> WireResult<PolarWirePacket> {
    if bytes.len() < PWP2_PRELUDE_LEN {
        return Err(WireError::Codec("PWP2 packet shorter than prelude".into()));
    }
    if &bytes[..4] != PWP2_MAGIC {
        return Err(WireError::Codec("missing PWP2 magic".into()));
    }
    let mut reader = Reader::new(&bytes[4..]);
    let version = reader.u16()?;
    if !matches!(version, WIRE_PACKET_VERSION | WIRE_PACKET_DIGEST_VERSION) {
        return Err(WireError::Codec(format!(
            "unsupported PWP2 version {version}"
        )));
    }
    let packet_kind = PacketKind::from_code(reader.u8()?)?;
    if packet_kind == PacketKind::AdmissionControl
        && bytes.len() > crate::MAX_ADMISSION_PACKET_BYTES
    {
        return Err(WireError::Codec("admission packet exceeds limit".into()));
    }
    let header_len = reader.u16()? as usize;
    let body_len = reader.u32()? as usize;
    let sig_len = reader.u16()? as usize;
    let expected_len = PWP2_PRELUDE_LEN
        .checked_add(header_len)
        .and_then(|len| len.checked_add(body_len))
        .and_then(|len| len.checked_add(sig_len))
        .ok_or_else(|| WireError::Codec("PWP2 packet length overflow".into()))?;
    if bytes.len() != expected_len {
        return Err(WireError::Codec(format!(
            "PWP2 packet length mismatch expected={expected_len} actual={}",
            bytes.len()
        )));
    }
    let header_start = PWP2_PRELUDE_LEN;
    let body_start = header_start + header_len;
    let sig_start = body_start + body_len;
    let mut header = decode_header(packet_kind, &bytes[header_start..body_start])?;
    header.version = version;
    let body_box = decode_body_box(&bytes[body_start..sig_start])?;
    if packet_kind == PacketKind::AdmissionControl {
        validate_admission_body(&body_box)?;
        header.protection_mode_code = body_box.protection_mode_code();
        if version != WIRE_PACKET_DIGEST_VERSION || sig_len != 64 {
            return Err(WireError::Codec(
                "invalid admission signature encoding".into(),
            ));
        }
    }
    if packet_kind.is_compact_data() {
        header.body_kind_code = body_box.body_kind_code();
        header.protection_mode_code = body_box.protection_mode_code();
        header.encoding_code = body_box.encoding_code();
        header.compression_code = body_box.compression_code();
    }
    if let WireBodyBox::WindowSealed(body) = &body_box {
        body.validate_header(packet_kind,&header)?;
        if sig_len != 64 || encode_header(packet_kind,&header)? != bytes[header_start..body_start] {
            return Err(WireError::Codec("non-canonical directed window packet".into()));
        }
    } else if header.protection_mode_code == crate::PROTECTION_WINDOW_SEALED {
        return Err(WireError::Codec("window header and body disagree".into()));
    }
    let packet_signature = bytes[sig_start..].to_vec();
    let packet = PolarWirePacket::unsigned(header, body_box)?
        .with_packet_kind(packet_kind)
        .with_signature(packet_signature);
    Ok(packet)
}

pub(crate) fn encode_signable(signable: &SignablePacket<'_>) -> WireResult<Vec<u8>> {
    if let WireBodyBox::WindowSealed(body) = signable.body_box {
        return Ok(crate::window::window_commitment(signable.packet_kind,signable.header,body)?.to_vec());
    }
    encode_packet_parts(
        signable.packet_kind,
        signable.header,
        signable.body_box,
        None,
    )
}

fn encode_packet_parts(
    packet_kind: PacketKind,
    header: &PolarWireHeader,
    body_box: &WireBodyBox,
    signature: Option<&[u8]>,
) -> WireResult<Vec<u8>> {
    if !matches!(
        header.version,
        WIRE_PACKET_VERSION | WIRE_PACKET_DIGEST_VERSION
    ) {
        return Err(WireError::Codec(format!(
            "unsupported PWP2 version {}",
            header.version
        )));
    }
    if packet_kind == PacketKind::AdmissionControl {
        validate_admission_body(body_box)?;
        if header.body_kind_code != body_box.body_kind_code()
            || header.protection_mode_code != body_box.protection_mode_code()
        {
            return Err(WireError::Codec("admission header and body disagree".into()));
        }
        if header.version != WIRE_PACKET_DIGEST_VERSION {
            return Err(WireError::Codec(
                "admission requires bounded digest signing".into(),
            ));
        }
    }
    if let WireBodyBox::WindowSealed(body)=body_box {
        body.validate_header(packet_kind,header)?;
        if signature.is_none_or(|sig|sig.len()!=64) {return Err(WireError::Codec("window packet requires a 64-byte signature".into()));}
    }
    let header_bytes = encode_header(packet_kind, header)?;
    let body_bytes = encode_body_box(body_box)?;
    let sig_len = signature.map_or(0, <[u8]>::len);
    if packet_kind == PacketKind::AdmissionControl
        && PWP2_PRELUDE_LEN + header_bytes.len() + body_bytes.len() + sig_len.max(64)
            > crate::MAX_ADMISSION_PACKET_BYTES
    {
        return Err(WireError::Codec("admission packet exceeds limit".into()));
    }
    let prefix = prelude(header.version,packet_kind,header_bytes.len(),body_bytes.len(),sig_len)?;
    let mut out = Vec::with_capacity(PWP2_PRELUDE_LEN+header_bytes.len()+body_bytes.len()+sig_len);
    out.extend_from_slice(&prefix);
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&body_bytes);
    if let Some(signature) = signature {
        out.extend_from_slice(signature);
    }
    Ok(out)
}

pub(crate) fn prelude(version:u16, kind:PacketKind, header:usize, body:usize, signature:usize) -> WireResult<[u8;15]> {
    let header=u16::try_from(header).map_err(|_|WireError::Codec("PWP2 header too large".into()))?;
    let body=u32::try_from(body).map_err(|_|WireError::Codec("PWP2 body too large".into()))?;
    let signature=u16::try_from(signature).map_err(|_|WireError::Codec("PWP2 signature too large".into()))?;
    let mut bytes=[0;15];
    bytes[..4].copy_from_slice(PWP2_MAGIC);
    bytes[4..6].copy_from_slice(&version.to_be_bytes()); bytes[6]=kind.code();
    bytes[7..9].copy_from_slice(&header.to_be_bytes());
    bytes[9..13].copy_from_slice(&body.to_be_bytes()); bytes[13..].copy_from_slice(&signature.to_be_bytes());
    Ok(bytes)
}

pub(crate) fn encode_header(packet_kind: PacketKind, header: &PolarWireHeader) -> WireResult<Vec<u8>> {
    if packet_kind == PacketKind::AdmissionControl {
        return encode_admission_header(header);
    }
    if header.admission.is_some() {
        return Err(WireError::Codec(
            "business header contains admission binding".into(),
        ));
    }
    let mut out = Vec::new();
    if packet_kind.is_compact_data() {
        out.extend_from_slice(&header.delivery_lane_id);
        out.extend_from_slice(&header.association_id);
        out.extend_from_slice(&header.stream_id_ref);
        match (&header.context_id, header.context_epoch) {
            (Some(context_id), Some(context_epoch)) => {
                push_string(&mut out, context_id)?;
                push_u32(&mut out, context_epoch);
            }
            _ => {
                return Err(WireError::Codec(
                    "compact packet requires context_id and context_epoch".into(),
                ));
            }
        }
        push_u64(&mut out, header.seq);
        push_u64(&mut out, header.previous_seq);
        if let Some(ack) = header.reverse_lane_ack_through_seq {
            out.push(1);
            push_u64(&mut out, ack);
        } else {
            out.push(0);
            push_u64(&mut out, 0);
        }
        return Ok(out);
    }
    out.extend_from_slice(&header.delivery_lane_id);
    out.extend_from_slice(&header.association_id);
    out.extend_from_slice(&header.stream_id_ref);
    push_string(&mut out, &header.stream_id)?;
    match (&header.context_id, header.context_epoch) {
        (Some(context_id), Some(context_epoch)) => {
            out.push(1);
            push_string(&mut out, context_id)?;
            push_u32(&mut out, context_epoch);
        }
        (None, None) => {
            out.push(0);
            push_u32(&mut out, 0);
        }
        _ => {
            return Err(WireError::Codec(
                "context_id and context_epoch must be set together".into(),
            ));
        }
    }
    push_u64(&mut out, header.seq);
    push_u64(&mut out, header.previous_seq);
    out.extend_from_slice(&node_bytes(&header.sender_node_id));
    if let Some(target_node_id) = &header.target_node_id {
        out.push(1);
        out.extend_from_slice(&node_bytes(target_node_id));
    } else {
        out.push(0);
        out.extend_from_slice(&[0u8; 32]);
    }
    out.extend_from_slice(&header.access_plane_ref);
    push_u16(&mut out, header.body_kind_code);
    out.push(header.protection_mode_code);
    out.push(header.encoding_code);
    out.push(header.compression_code);
    push_u64(&mut out, header.created_at_secs);
    if let Some(ack) = header.reverse_lane_ack_through_seq {
        out.push(1);
        push_u64(&mut out, ack);
    } else {
        out.push(0);
        push_u64(&mut out, 0);
    }
    Ok(out)
}

fn decode_header(packet_kind: PacketKind, bytes: &[u8]) -> WireResult<PolarWireHeader> {
    if packet_kind == PacketKind::AdmissionControl {
        return decode_admission_header(bytes);
    }
    let mut reader = Reader::new(bytes);
    let delivery_lane_id = reader.array16()?;
    let association_id = reader.array32()?;
    let stream_id_ref = reader.array16()?;
    if packet_kind.is_compact_data() {
        let context_id = reader.string()?;
        let context_epoch = reader.u32()?;
        let seq = reader.u64()?;
        let previous_seq = reader.u64()?;
        let has_ack = reader.u8()?;
        let ack = reader.u64()?;
        let reverse_lane_ack_through_seq = match has_ack {
            0 => None,
            1 => Some(ack),
            _ => {
                return Err(WireError::Codec(format!(
                    "invalid PWP2 reverse ACK presence flag {has_ack}"
                )));
            }
        };
        reader.finish()?;
        return Ok(PolarWireHeader {
            version: WIRE_PACKET_VERSION,
            delivery_lane_id,
            association_id,
            admission: None,
            stream_id: hex::encode(stream_id_ref),
            stream_id_ref,
            context_id: Some(context_id),
            context_epoch: Some(context_epoch),
            seq,
            previous_seq,
            sender_node_id: String::new(),
            target_node_id: None,
            access_plane_ref: [0u8; 16],
            body_kind_code: 0,
            encoding_code: ENCODING_CBOR,
            compression_code: COMPRESSION_NONE,
            protection_mode_code: 0,
            created_at_secs: 0,
            reverse_lane_ack_through_seq,
        });
    }
    let stream_id = reader.string()?;
    let has_context = reader.u8()?;
    let (context_id, context_epoch) = match has_context {
        0 => {
            let reserved_epoch = reader.u32()?;
            if reserved_epoch != 0 {
                return Err(WireError::Codec(
                    "context_epoch must be zero when context is absent".into(),
                ));
            }
            (None, None)
        }
        1 => {
            let context_id = reader.string()?;
            let context_epoch = reader.u32()?;
            (Some(context_id), Some(context_epoch))
        }
        _ => {
            return Err(WireError::Codec(format!(
                "invalid PWP2 context presence flag {has_context}"
            )));
        }
    };
    let seq = reader.u64()?;
    let previous_seq = reader.u64()?;
    let sender_node_id = hex::encode(reader.array32()?);
    let has_target = reader.u8()?;
    let target_bytes = reader.array32()?;
    let target_node_id = match has_target {
        0 => None,
        1 => Some(hex::encode(target_bytes)),
        _ => {
            return Err(WireError::Codec(format!(
                "invalid PWP2 target presence flag {has_target}"
            )));
        }
    };
    let access_plane_ref = reader.array16()?;
    let body_kind_code = reader.u16()?;
    let protection_mode_code = reader.u8()?;
    let encoding_code = reader.u8()?;
    let compression_code = reader.u8()?;
    let created_at_secs = reader.u64()?;
    let has_ack = reader.u8()?;
    let ack = reader.u64()?;
    let reverse_lane_ack_through_seq = match has_ack {
        0 => None,
        1 => Some(ack),
        _ => {
            return Err(WireError::Codec(format!(
                "invalid PWP2 reverse ACK presence flag {has_ack}"
            )));
        }
    };
    reader.finish()?;
    Ok(PolarWireHeader {
        version: WIRE_PACKET_VERSION,
        delivery_lane_id,
        association_id,
        admission: None,
        stream_id,
        stream_id_ref,
        context_id,
        context_epoch,
        seq,
        previous_seq,
        sender_node_id,
        target_node_id,
        access_plane_ref,
        body_kind_code,
        encoding_code,
        compression_code,
        protection_mode_code,
        created_at_secs,
        reverse_lane_ack_through_seq,
    })
}

fn encode_body_box(body_box: &WireBodyBox) -> WireResult<Vec<u8>> {
    let mut out = Vec::new();
    match body_box {
        WireBodyBox::ClearSigned(body) => {
            out.push(0);
            push_u16(&mut out, body.body_kind_code);
            out.extend_from_slice(&body.body_hash);
            push_bytes(&mut out, &body.canonical_body_bytes)?;
        }
        WireBodyBox::RecipientKeySealed(body) => {
            out.push(1);
            push_u16(&mut out, body.body_kind_code);
            push_string(&mut out, &body.recipient_key_id)?;
            push_bytes(&mut out, &body.sender_ephemeral_pubkey)?;
            push_bytes(&mut out, &body.nonce)?;
            push_bytes(&mut out, &body.ciphertext)?;
            out.extend_from_slice(&body.plaintext_body_hash);
            out.extend_from_slice(&body.sealed_payload_hash);
        }
        WireBodyBox::PeerSessionSealed(body) => {
            out.push(2);
            push_u16(&mut out, body.body_kind_code);
            push_u64(&mut out, body.key_epoch);
            push_bytes(&mut out, &body.nonce)?;
            push_bytes(&mut out, &body.ciphertext)?;
            out.extend_from_slice(&body.plaintext_body_hash);
            out.extend_from_slice(&body.sealed_payload_hash);
        }
        WireBodyBox::WindowSealed(body) => {
            out.push(crate::PROTECTION_WINDOW_SEALED);
            push_u16(&mut out,crate::BODY_KIND_LGP_MESSAGE_BATCH);
            push_u64(&mut out,body.window); out.push(body.first); out.push(body.count);
            out.extend_from_slice(&body.ephemeral); out.extend_from_slice(&body.ciphertext);
        }
        WireBodyBox::RealmSealed(body) => {
            out.push(3);
            push_u16(&mut out, body.body_kind_code);
            push_string(&mut out, &body.realm_id)?;
            push_u64(&mut out, body.realm_key_epoch);
            push_bytes(&mut out, &body.nonce)?;
            push_bytes(&mut out, &body.ciphertext)?;
            out.extend_from_slice(&body.plaintext_body_hash);
            out.extend_from_slice(&body.sealed_payload_hash);
        }
    }
    Ok(out)
}

fn decode_body_box(bytes: &[u8]) -> WireResult<WireBodyBox> {
    let mut reader = Reader::new(bytes);
    let mode = reader.u8()?;
    let body_box = match mode {
        0 => {
            let body_kind_code = reader.u16()?;
            let body_hash = reader.array32()?;
            let canonical_body_bytes = reader.bytes()?;
            if body_hash != sha256_bytes(&canonical_body_bytes) {
                return Err(WireError::Codec("clear-signed body hash mismatch".into()));
            }
            WireBodyBox::ClearSigned(ClearSignedBody {
                body_kind_code,
                canonical_body_bytes,
                body_hash,
            })
        }
        1 => {
            let body_kind_code = reader.u16()?;
            let recipient_key_id = reader.string()?;
            let sender_ephemeral_pubkey = reader.bytes()?;
            let nonce = reader.bytes()?;
            let ciphertext = reader.bytes()?;
            let plaintext_body_hash = reader.array32()?;
            let sealed_payload_hash = reader.array32()?;
            WireBodyBox::RecipientKeySealed(RecipientKeySealedBody {
                body_kind_code,
                recipient_key_id,
                sender_ephemeral_pubkey,
                nonce,
                ciphertext,
                plaintext_body_hash,
                sealed_payload_hash,
            })
        }
        2 => {
            let body_kind_code = reader.u16()?;
            let key_epoch = reader.u64()?;
            let nonce = reader.bytes()?;
            let ciphertext = reader.bytes()?;
            let plaintext_body_hash = reader.array32()?;
            let sealed_payload_hash = reader.array32()?;
            WireBodyBox::PeerSessionSealed(PeerSessionSealedBody {
                body_kind_code,
                key_epoch,
                nonce,
                ciphertext,
                plaintext_body_hash,
                sealed_payload_hash,
            })
        }
        3 => {
            let body_kind_code = reader.u16()?;
            let realm_id = reader.string()?;
            let realm_key_epoch = reader.u64()?;
            let nonce = reader.bytes()?;
            let ciphertext = reader.bytes()?;
            let plaintext_body_hash = reader.array32()?;
            let sealed_payload_hash = reader.array32()?;
            WireBodyBox::RealmSealed(RealmSealedBody {
                body_kind_code,
                realm_id,
                realm_key_epoch,
                nonce,
                ciphertext,
                plaintext_body_hash,
                sealed_payload_hash,
            })
        }
        crate::PROTECTION_WINDOW_SEALED => {
            if bytes.len() > crate::window::WINDOW_BODY_HEADER+crate::window::WINDOW_MAX_PLAIN+16 || reader.u16()? != crate::BODY_KIND_LGP_MESSAGE_BATCH {
                return Err(WireError::Codec("invalid directed window payload".into()));
            }
            let window=reader.u64()?; let first=reader.u8()?; let count=reader.u8()?; let ephemeral=reader.array32()?;
            let ciphertext=reader.take(bytes.len()-reader.pos)?.to_vec();
            WireBodyBox::WindowSealed(crate::WindowSealedBody::new(window,first,count,ephemeral,ciphertext)?)
        }
        _ => return Err(WireError::Codec(format!("unknown body box mode {mode}"))),
    };
    reader.finish()?;
    Ok(body_box)
}

fn node_bytes(node_id: &str) -> [u8; 32] {
    hex::decode(node_id)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .unwrap_or_else(|| sha256_bytes(node_id.as_bytes()))
}

fn push_string(out: &mut Vec<u8>, value: &str) -> WireResult<()> {
    push_bytes(out, value.as_bytes())
}

fn push_bytes(out: &mut Vec<u8>, value: &[u8]) -> WireResult<()> {
    if value.len() > u32::MAX as usize {
        return Err(WireError::Codec("PWP2 byte field too large".into()));
    }
    push_u32(out, value.len() as u32);
    out.extend_from_slice(value);
    Ok(())
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn u8(&mut self) -> WireResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> WireResult<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> WireResult<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> WireResult<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn array16(&mut self) -> WireResult<[u8; 16]> {
        Ok(self.take(16)?.try_into().expect("16 bytes"))
    }

    fn array32(&mut self) -> WireResult<[u8; 32]> {
        Ok(self.take(32)?.try_into().expect("32 bytes"))
    }

    fn bytes(&mut self) -> WireResult<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn string(&mut self) -> WireResult<String> {
        String::from_utf8(self.bytes()?)
            .map_err(|error| WireError::Codec(format!("PWP2 string is not UTF-8: {error}")))
    }

    fn finish(&self) -> WireResult<()> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(WireError::Codec(format!(
                "PWP2 decoder left {} trailing bytes",
                self.bytes.len() - self.pos
            )))
        }
    }

    fn take(&mut self, len: usize) -> WireResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| WireError::Codec("PWP2 read overflow".into()))?;
        if end > self.bytes.len() {
            return Err(WireError::Codec(format!(
                "PWP2 short read need={len} remaining={}",
                self.bytes.len().saturating_sub(self.pos)
            )));
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }
}

// PacketKind::CompactData

fn validate_admission_body(body: &WireBodyBox) -> WireResult<()> {
    match body {
        WireBodyBox::ClearSigned(clear)
            if crate::is_admission_body_kind(clear.body_kind_code)
                && clear.canonical_body_bytes.len() <= crate::MAX_ADMISSION_BODY_BYTES
                && clear.body_hash == sha256_bytes(&clear.canonical_body_bytes) =>
        {
            Ok(())
        }
        WireBodyBox::RecipientKeySealed(sealed)
            if supports_sealed_admission(sealed.body_kind_code)
                && sealed.sender_ephemeral_pubkey.len() == 32
                && sealed.nonce.len() == 12
                && sealed.ciphertext.len() <= crate::MAX_ADMISSION_BODY_BYTES =>
        {
            Ok(())
        }
        _ => Err(WireError::Codec(
            "admission requires bounded public control or an allowed sealed control"
                .into(),
        )),
    }
}

fn supports_sealed_admission(body_kind_code: u16) -> bool {
    matches!(
        body_kind_code,
        crate::BODY_KIND_ACCESS_WORKFLOW_REQUEST
            | crate::BODY_KIND_TRANSPORT_QUERY
            | crate::BODY_KIND_TRANSPORT_SNAPSHOT
    )
}

fn encode_admission_header(header: &PolarWireHeader) -> WireResult<Vec<u8>> {
    let binding = header
        .admission
        .clone()
        .ok_or_else(|| WireError::Codec("missing admission binding".into()))?;
    let sender: [u8; 32] = hex::decode(&header.sender_node_id)
        .map_err(|_| WireError::Codec("invalid admission sender".into()))?
        .try_into()
        .map_err(|_| WireError::Codec("invalid admission sender length".into()))?;
    let target: [u8; 32] = hex::decode(
        header
            .target_node_id
            .as_deref()
            .ok_or_else(|| WireError::Codec("missing admission recipient".into()))?,
    )
    .map_err(|_| WireError::Codec("invalid admission recipient".into()))?
    .try_into()
    .map_err(|_| WireError::Codec("invalid admission recipient length".into()))?;
    let mut canonical = PolarWireHeader::admission(
        sender,
        target,
        binding.clone(),
        header.body_kind_code,
        header.created_at_secs,
    )?;
    if supports_sealed_admission(header.body_kind_code)
        && header.protection_mode_code == crate::PROTECTION_RECIPIENT_KEY_SEALED
    {
        canonical.protection_mode_code = header.protection_mode_code;
    }
    if header != &canonical {
        return Err(WireError::Codec(
            "admission header contains noncanonical business metadata".into(),
        ));
    }
    let mut out = Vec::with_capacity(crate::ADMISSION_HEADER_BYTES);
    out.extend_from_slice(&sender);
    out.extend_from_slice(&target);
    out.extend_from_slice(binding.sender_generation.as_bytes());
    out.extend_from_slice(binding.target_generation.as_bytes());
    out.extend_from_slice(&binding.admission_id);
    push_u16(&mut out, header.body_kind_code);
    push_u64(&mut out, header.created_at_secs);
    Ok(out)
}

fn decode_admission_header(bytes: &[u8]) -> WireResult<PolarWireHeader> {
    if bytes.len() != crate::ADMISSION_HEADER_BYTES {
        return Err(WireError::Codec("invalid admission header length".into()));
    }
    let mut reader = Reader::new(bytes);
    let sender = reader.array32()?;
    let target = reader.array32()?;
    let binding = crate::AdmissionBinding {
        sender_generation: crate::InstallationGeneration::from_bytes(reader.array16()?)?,
        target_generation: crate::InstallationGeneration::from_bytes(reader.array16()?)?,
        admission_id: reader.array16()?,
    };
    let kind = reader.u16()?;
    let created_at_secs = reader.u64()?;
    reader.finish()?;
    PolarWireHeader::admission(sender, target, binding, kind, created_at_secs)
}


