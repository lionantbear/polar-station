use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand_core::{OsRng, RngCore};

use crate::{CryptoError, SessionKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedMessage {
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

pub fn seal_message(key: &SessionKey, plaintext: &[u8]) -> Result<SealedMessage, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| CryptoError::EncryptFailed)?;
    Ok(SealedMessage { nonce, ciphertext })
}

pub fn open_message(key: &SessionKey, sealed: &SealedMessage) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    cipher
        .decrypt(Nonce::from_slice(&sealed.nonce), sealed.ciphertext.as_ref())
        .map_err(|_| CryptoError::DecryptFailed)
}
