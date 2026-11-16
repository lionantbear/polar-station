use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use polar_identity::BindingPublicKey;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::CryptoError;
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingSealedMessage {
    pub ephemeral_pubkey: [u8; 32],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

pub fn seal_to_binding(
    recipient: &BindingPublicKey,
    plaintext: &[u8],
    context: &[u8],
) -> Result<BindingSealedMessage, CryptoError> {
    let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let ephemeral_pubkey = *PublicKey::from(&ephemeral_secret).as_bytes();
    let recipient_public = PublicKey::from(*recipient.as_bytes());
    let shared = ephemeral_secret.diffie_hellman(&recipient_public);
    if !shared.was_contributory() {
        return Err(CryptoError::EncryptFailed);
    }
    let key_bytes = Zeroizing::new(derive_binding_key(shared.as_bytes(), context));
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes[..]));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| CryptoError::EncryptFailed)?;
    Ok(BindingSealedMessage {
        ephemeral_pubkey,
        nonce,
        ciphertext,
    })
}

pub fn open_from_binding(
    local_binding_secret: [u8; 32],
    sealed: &BindingSealedMessage,
    context: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let local_secret = StaticSecret::from(local_binding_secret);
    let remote_public = PublicKey::from(sealed.ephemeral_pubkey);
    let shared = local_secret.diffie_hellman(&remote_public);
    if !shared.was_contributory() {
        return Err(CryptoError::DecryptFailed);
    }
    let key_bytes = Zeroizing::new(derive_binding_key(shared.as_bytes(), context));
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes[..]));
    cipher
        .decrypt(Nonce::from_slice(&sealed.nonce), sealed.ciphertext.as_ref())
        .map_err(|_| CryptoError::DecryptFailed)
}

fn derive_binding_key(shared: &[u8], context: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(Some(context), shared);
    let mut out = [0u8; 32];
    hkdf.expand(b"polar-binding-envelope-v1", &mut out)
        .expect("hkdf expand for binding envelope");
    out
}


