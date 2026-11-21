use serde::{Deserialize, Serialize};

use crate::{
    sha256_bytes, HashBytes, COMPRESSION_NONE, ENCODING_CBOR, PROTECTION_CLEAR_SIGNED,
    PROTECTION_PEER_SESSION_SEALED, PROTECTION_REALM_SEALED, PROTECTION_RECIPIENT_KEY_SEALED,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearSignedBody {
    pub body_kind_code: u16,
    pub canonical_body_bytes: Vec<u8>,
    pub body_hash: HashBytes,
}

impl ClearSignedBody {
    pub fn new(body_kind_code: u16, canonical_body_bytes: Vec<u8>) -> Self {
        let body_hash = sha256_bytes(&canonical_body_bytes);
        Self {
            body_kind_code,
            canonical_body_bytes,
            body_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipientKeySealedBody {
    pub body_kind_code: u16,
    pub recipient_key_id: String,
    pub sender_ephemeral_pubkey: Vec<u8>,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub plaintext_body_hash: HashBytes,
    pub sealed_payload_hash: HashBytes,
}

impl RecipientKeySealedBody {
    pub fn new(
        body_kind_code: u16,
        recipient_key_id: impl Into<String>,
        sender_ephemeral_pubkey: Vec<u8>,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
        plaintext_body_hash: HashBytes,
    ) -> Self {
        let sealed_payload_hash = sealed_payload_hash(&nonce, &ciphertext);
        Self {
            body_kind_code,
            recipient_key_id: recipient_key_id.into(),
            sender_ephemeral_pubkey,
            nonce,
            ciphertext,
            plaintext_body_hash,
            sealed_payload_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSessionSealedBody {
    pub body_kind_code: u16,
    pub key_epoch: u64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub plaintext_body_hash: HashBytes,
    pub sealed_payload_hash: HashBytes,
}

impl PeerSessionSealedBody {
    pub fn new(
        body_kind_code: u16,
        key_epoch: u64,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
        plaintext_body_hash: HashBytes,
    ) -> Self {
        let sealed_payload_hash = sealed_payload_hash(&nonce, &ciphertext);
        Self {
            body_kind_code,
            key_epoch,
            nonce,
            ciphertext,
            plaintext_body_hash,
            sealed_payload_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealmSealedBody {
    pub body_kind_code: u16,
    pub realm_id: String,
    pub realm_key_epoch: u64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub plaintext_body_hash: HashBytes,
    pub sealed_payload_hash: HashBytes,
}

impl RealmSealedBody {
    pub fn new(
        body_kind_code: u16,
        realm_id: impl Into<String>,
        realm_key_epoch: u64,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
        plaintext_body_hash: HashBytes,
    ) -> Self {
        let sealed_payload_hash = sealed_payload_hash(&nonce, &ciphertext);
        Self {
            body_kind_code,
            realm_id: realm_id.into(),
            realm_key_epoch,
            nonce,
            ciphertext,
            plaintext_body_hash,
            sealed_payload_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "body")]
pub enum WireBodyBox {
    ClearSigned(ClearSignedBody),
    RecipientKeySealed(RecipientKeySealedBody),
    PeerSessionSealed(PeerSessionSealedBody),
    RealmSealed(RealmSealedBody),
    WindowSealed(crate::WindowSealedBody),
}

impl WireBodyBox {
    pub fn body_kind_code(&self) -> u16 {
        match self {
            Self::ClearSigned(body) => body.body_kind_code,
            Self::RecipientKeySealed(body) => body.body_kind_code,
            Self::PeerSessionSealed(body) => body.body_kind_code,
            Self::RealmSealed(body) => body.body_kind_code,
            Self::WindowSealed(_) => crate::BODY_KIND_LGP_MESSAGE_BATCH,
        }
    }

    pub fn protection_mode_code(&self) -> u8 {
        match self {
            Self::ClearSigned(_) => PROTECTION_CLEAR_SIGNED,
            Self::RecipientKeySealed(_) => PROTECTION_RECIPIENT_KEY_SEALED,
            Self::PeerSessionSealed(_) => PROTECTION_PEER_SESSION_SEALED,
            Self::RealmSealed(_) => PROTECTION_REALM_SEALED,
            Self::WindowSealed(_) => crate::PROTECTION_WINDOW_SEALED,
        }
    }

    pub fn encoding_code(&self) -> u8 {
        if matches!(self, Self::WindowSealed(_)) {
            crate::ENCODING_BINARY
        } else {
            ENCODING_CBOR
        }
    }

    pub fn compression_code(&self) -> u8 {
        COMPRESSION_NONE
    }

    pub fn body_hash(&self) -> HashBytes {
        match self {
            Self::ClearSigned(body) => body.body_hash,
            Self::RecipientKeySealed(body) => body.plaintext_body_hash,
            Self::PeerSessionSealed(body) => body.plaintext_body_hash,
            Self::RealmSealed(body) => body.plaintext_body_hash,
            // The public commitment in protection 4 covers the ciphertext and tag.
            Self::WindowSealed(body) => sha256_bytes(&body.ciphertext),
        }
    }

    pub fn sealed_payload_hash(&self) -> Option<HashBytes> {
        match self {
            Self::ClearSigned(_) => None,
            Self::RecipientKeySealed(body) => Some(body.sealed_payload_hash),
            Self::PeerSessionSealed(body) => Some(body.sealed_payload_hash),
            Self::RealmSealed(body) => Some(body.sealed_payload_hash),
            Self::WindowSealed(body) => Some(sha256_bytes(&body.ciphertext)),
        }
    }

    pub fn dispatch_bytes_without_decrypt(&self) -> &[u8] {
        match self {
            Self::ClearSigned(body) => &body.canonical_body_bytes,
            Self::RecipientKeySealed(body) => &body.ciphertext,
            Self::PeerSessionSealed(body) => &body.ciphertext,
            Self::RealmSealed(body) => &body.ciphertext,
            Self::WindowSealed(body) => &body.ciphertext,
        }
    }
}

fn sealed_payload_hash(nonce: &[u8], ciphertext: &[u8]) -> HashBytes {
    let mut bytes = Vec::with_capacity(nonce.len() + ciphertext.len());
    bytes.extend_from_slice(nonce);
    bytes.extend_from_slice(ciphertext);
    sha256_bytes(&bytes)
}
