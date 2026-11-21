//! Pure CPU backend for an approved directed receive window. Only an authenticated
//! proof for this receiving identity can open ciphertext with its Binding key.
use crate::CryptoError;
use chacha20poly1305::{
    aead::{Aead, Payload},
    ChaCha20Poly1305, KeyInit,
};
use hkdf::Hkdf;
use polar_identity::{
    signed_identity::NodeIdSignatureVerifier, IdentityKeypair, NodeId, SignatureBytes,
    SignatureVerifier, SigningPurpose,
};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub struct DirectedWindow {
    pub sender: [u8; 32],
    pub recipient: [u8; 32],
    pub association: [u8; 32],
    pub plane: [u8; 16],
    pub lane: [u8; 16],
    pub binding: [u8; 32],
    pub window: u64,
}

impl DirectedWindow {
    pub fn scope(&self) -> [u8; 168] {
        let mut scope = [0; 168];
        scope[..32].copy_from_slice(&self.sender);
        scope[32..64].copy_from_slice(&self.recipient);
        scope[64..96].copy_from_slice(&self.association);
        scope[96..112].copy_from_slice(&self.plane);
        scope[112..128].copy_from_slice(&self.lane);
        scope[128..160].copy_from_slice(&self.binding);
        scope[160..].copy_from_slice(&self.window.to_be_bytes());
        scope
    }
    pub fn from_scope(scope: &[u8; 168]) -> Self {
        Self {
            sender: scope[..32].try_into().unwrap(), recipient: scope[32..64].try_into().unwrap(),
            association: scope[64..96].try_into().unwrap(), plane: scope[96..112].try_into().unwrap(),
            lane: scope[112..128].try_into().unwrap(), binding: scope[128..160].try_into().unwrap(),
            window: u64::from_be_bytes(scope[160..].try_into().unwrap()),
        }
    }

    fn valid(&self, seq: u64, first: u8, count: u8) -> bool {
        self.sender != self.recipient
            && self.association != [0; 32]
            && self.binding != [0; 32]
            && self.window <= u64::MAX / 32
            && count > 0
            && u16::from(first) + u16::from(count) <= 32
            && seq >= u64::from(count)
            && self.lane
                == polar_wire::delivery_lane_ref_for_parts(
                    &NodeId::from_bytes(self.sender).to_hex(),
                    Some(&NodeId::from_bytes(self.recipient).to_hex()),
                    &self.plane,
                )
    }
    fn salt(&self) -> [u8; 32] {
        Sha256::new()
            .chain_update(b"PWKX0001")
            .chain_update(self.scope())
            .finalize()
            .into()
    }
    fn aad(&self, seq: u64, first: u8, count: u8) -> [u8; 178] {
        let mut aad = [0; 178];
        aad[..168].copy_from_slice(&self.scope());
        aad[168..176].copy_from_slice(&seq.to_be_bytes());
        aad[176] = first;
        aad[177] = count;
        aad
    }
    fn shared_window_key(&self, private: &StaticSecret, public: [u8; 32]) -> Option<Zeroizing<[u8;32]>> {
        let shared = private.diffie_hellman(&PublicKey::from(public));
        if !shared.was_contributory() {
            return None;
        }
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(&self.salt()), shared.as_bytes())
            .expand(b"PWRK0001", key.as_mut())
            .ok()?;
        Some(key)
    }

    pub fn seal(
        &self,
        sender: &IdentityKeypair,
        seq: u64,
        first: u8,
        count: u8,
        plain: &[u8],
    ) -> Result<polar_wire::WindowSealedBody, CryptoError> {
        let (ephemeral, key) = self.sender_key(sender)?;
        self.seal_with_window_key(&key, ephemeral, seq, first, count, plain)
    }

    /// Returns only material for this scope, with the sender fixed by the provider.
    pub fn sender_key(&self, sender: &IdentityKeypair) -> Result<([u8;32], Zeroizing<[u8;32]>), CryptoError> {
        if sender.node_id().as_bytes() != &self.sender || !self.valid(1, 0, 1) {
            return Err(CryptoError::EncryptFailed);
        }
        let private = sender.directed_window_ephemeral(&self.salt());
        let key = self.shared_window_key(&private, self.binding).ok_or(CryptoError::EncryptFailed)?;
        Ok((*PublicKey::from(&private).as_bytes(), key))
    }

    pub fn seal_with_window_key(
        &self,
        key: &[u8; 32],
        ephemeral: [u8; 32],
        seq: u64,
        first: u8,
        count: u8,
        plain: &[u8],
    ) -> Result<polar_wire::WindowSealedBody, CryptoError> {
        self.seal_with_cipher(
            &ChaCha20Poly1305::new(key.into()),
            ephemeral,
            seq,
            first,
            count,
            plain,
        )
    }

    fn seal_with_cipher(
        &self,
        cipher: &ChaCha20Poly1305,
        ephemeral: [u8; 32],
        seq: u64,
        first: u8,
        count: u8,
        plain: &[u8],
    ) -> Result<polar_wire::WindowSealedBody, CryptoError> {
        if !self.valid(seq, first, count)
            || plain.len() > polar_wire::window::WINDOW_MAX_PLAIN
            || polar_wire::decode_message_batch(plain)
                .map_or(true, |items| items.len() != usize::from(count))
        {
            return Err(CryptoError::EncryptFailed);
        }
        let cipher_text = cipher
            .encrypt(
                (&nonce(seq)).into(),
                Payload {
                    msg: plain,
                    aad: &self.aad(seq, first, count),
                },
            )
            .map_err(|_| CryptoError::EncryptFailed)?;
        polar_wire::WindowSealedBody::new(self.window, first, count, ephemeral, cipher_text)
            .map_err(|_| CryptoError::EncryptFailed)
    }
}

fn nonce(seq: u64) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

pub fn open_signed_window(
    identity: &IdentityKeypair,
    proof: &[u8; polar_wire::window::WINDOW_PROOF_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let packet = VerifiedWindow::verify(
        identity.node_id(),
        *identity.binding_public_key().as_bytes(),
        proof,
        ciphertext,
    )?;
    let private = StaticSecret::from(identity.binding_secret_bytes());
    let cipher = packet
        .window
        .shared_window_key(&private, proof[186..218].try_into().unwrap())
        .ok_or(CryptoError::DecryptFailed)?;
    packet.open_with_window_key(&cipher)
}

/// Key-release operation at the fixed recipient. The opening host separately verifies actual ciphertext.
pub fn recipient_key(identity: &IdentityKeypair, proof: &[u8;346]) -> Result<Zeroizing<[u8;32]>, CryptoError> {
    let (window, _, _, _) = signed_scope(identity.node_id(), *identity.binding_public_key().as_bytes(), proof)?;
    let private = StaticSecret::from(identity.binding_secret_bytes());
    window.shared_window_key(&private, proof[186..218].try_into().unwrap()).ok_or(CryptoError::DecryptFailed)
}

fn signed_scope(recipient: NodeId, binding: [u8;32], proof: &[u8;346]) -> Result<(DirectedWindow,u64,u8,u8),CryptoError> {
        let window = DirectedWindow {
            sender: proof[8..40].try_into().unwrap(),
            recipient: proof[40..72].try_into().unwrap(),
            association: proof[72..104].try_into().unwrap(),
            plane: proof[104..120].try_into().unwrap(),
            lane: proof[120..136].try_into().unwrap(),
            binding: proof[154..186].try_into().unwrap(),
            window: u64::from_be_bytes(proof[144..152].try_into().unwrap()),
        };
        let seq = u64::from_be_bytes(proof[136..144].try_into().unwrap());
        let first = proof[152];
        let count = proof[153];
        if &proof[..8] != b"PWRW0001"
            || window.recipient != *recipient.as_bytes()
            || window.binding != binding
            || !window.valid(seq, first, count)
        {
            return Err(CryptoError::DecryptFailed);
        }
        let input = SigningPurpose::WirePacketDigestV3
            .digest_preimage(&proof[..282])
            .map_err(|_| CryptoError::DecryptFailed)?;
        NodeIdSignatureVerifier
            .verify_signature(
                &NodeId::from_bytes(window.sender),
                &input,
                &SignatureBytes::from_bytes(proof[282..].try_into().unwrap()),
            )
            .map_err(|_| CryptoError::DecryptFailed)?;
    Ok((window,seq,first,count))
}

/// Keeps the authenticated proof bound to the same ciphertext until AEAD opening.
pub struct VerifiedWindow<'a> {
    window: DirectedWindow,
    seq: u64,
    first: u8,
    count: u8,
    ciphertext: &'a [u8],
}
impl<'a> VerifiedWindow<'a> {
    pub fn verify(
        recipient: NodeId,
        binding: [u8; 32],
        proof: &[u8; polar_wire::window::WINDOW_PROOF_LEN],
        ciphertext: &'a [u8],
    ) -> Result<Self, CryptoError> {
        if !(16..=polar_wire::window::WINDOW_MAX_PLAIN + 16).contains(&ciphertext.len())
            || polar_wire::sha256_bytes(ciphertext) != proof[250..282] {
            return Err(CryptoError::DecryptFailed);
        }
        let (window, seq, first, count) = signed_scope(recipient, binding, proof)?;
        Ok(Self {
            window,
            seq,
            first,
            count,
            ciphertext,
        })
    }

    pub fn open_with_window_key(&self, key: &[u8; 32]) -> Result<Vec<u8>, CryptoError> {
        self.open(&ChaCha20Poly1305::new(key.into()))
    }

    fn open(&self, cipher: &ChaCha20Poly1305) -> Result<Vec<u8>, CryptoError> {
        let Self {
            ref window,
            seq,
            first,
            count,
            ciphertext,
        } = *self;
        let plain = cipher
            .decrypt(
                (&nonce(seq)).into(),
                Payload {
                    msg: ciphertext,
                    aad: &window.aad(seq, first, count),
                },
            )
            .map_err(|_| CryptoError::DecryptFailed)?;
        if polar_wire::decode_message_batch(&plain).map_or(true, |v| v.len() != usize::from(count))
        {
            return Err(CryptoError::DecryptFailed);
        }
        Ok(plain)
    }
}


