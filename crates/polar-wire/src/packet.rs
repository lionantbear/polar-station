use serde::{Deserialize, Serialize};

use crate::{
    decode_packet, encode_packet, encode_signable, sha256_bytes, signable::SignablePacket,
    HashBytes, PolarWireHeader, SignatureBytes, WireBodyBox, WireError, WireResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PacketKind {
    FullData = 1,
    StreamContextOpen = 2,
    CompactData = 3,
    AdmissionControl = 4,
}

impl PacketKind {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> WireResult<Self> {
        match code {
            1 => Ok(Self::FullData),
            2 => Ok(Self::StreamContextOpen),
            3 => Ok(Self::CompactData),
            4 => Ok(Self::AdmissionControl),
            _ => Err(WireError::Codec(format!("unknown PWP2 packet_kind {code}"))),
        }
    }

    pub fn is_compact_data(self) -> bool {
        matches!(self, Self::CompactData)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolarWirePacket {
    pub packet_kind: PacketKind,
    pub header: PolarWireHeader,
    pub body_box: WireBodyBox,
    pub packet_signature: SignatureBytes,
}

impl PolarWirePacket {
    pub fn new(
        header: PolarWireHeader,
        body_box: WireBodyBox,
        packet_signature: SignatureBytes,
    ) -> WireResult<Self> {
        if header.body_kind_code != body_box.body_kind_code() {
            return Err(WireError::BodyKindMismatch {
                header: header.body_kind_code,
                body_box: body_box.body_kind_code(),
            });
        }
        Ok(Self {
            packet_kind: PacketKind::FullData,
            header,
            body_box,
            packet_signature,
        })
    }

    pub fn unsigned(header: PolarWireHeader, body_box: WireBodyBox) -> WireResult<Self> {
        Self::new(header, body_box, Vec::new())
    }

    pub fn with_signature(mut self, signature: impl Into<SignatureBytes>) -> Self {
        self.packet_signature = signature.into();
        self
    }

    pub fn with_packet_kind(mut self, packet_kind: PacketKind) -> Self {
        self.packet_kind = packet_kind;
        self
    }

    pub fn signable_bytes(&self) -> WireResult<Vec<u8>> {
        encode_signable(&SignablePacket {
            packet_kind: self.packet_kind,
            header: &self.header,
            body_box: &self.body_box,
        })
    }

    pub fn signable_hash(&self) -> WireResult<HashBytes> {
        Ok(sha256_bytes(&self.signable_bytes()?))
    }

    pub fn encode(&self) -> WireResult<Vec<u8>> {
        encode_packet(self)
    }

    pub fn decode(bytes: &[u8]) -> WireResult<Self> {
        decode_packet(bytes)
    }

    pub fn packet_hash(&self) -> WireResult<HashBytes> {
        Ok(sha256_bytes(&self.encode()?))
    }

    pub fn body_hash(&self) -> HashBytes {
        self.body_box.body_hash()
    }

    pub fn sealed_payload_hash(&self) -> Option<HashBytes> {
        self.body_box.sealed_payload_hash()
    }
}


