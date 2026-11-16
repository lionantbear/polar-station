//! Pairing proofs, directional session keys, and encrypted application frames.
//!
//! Pairing handshakes are text JSON. Once both proofs have been verified, all
//! application messages are carried by [`EncryptedFrameWriter`] and
//! [`EncryptedFrameReader`]. A reader is deliberately stateful: only its exact
//! next sequence number can be opened.

use std::error::Error;
use std::fmt;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::MAX_APPLICATION_PAYLOAD_BYTES;

const FRAME_MAGIC: &[u8; 4] = b"PTM1";
const FRAME_HEADER_BYTES: usize = 13;
const AEAD_TAG_BYTES: usize = 16;
const LEGACY_CLIENT_PROOF_DOMAIN: &str = "leotmux-companion-v1";
const PRODUCTION_PAIRING_DOMAIN: &str = "polar-tmux-pairing-v1";
const PRODUCTION_SESSION_DOMAIN: &str = "polar-tmux-session-v1";

type HmacSha256 = Hmac<Sha256>;

/// Errors that make a pairing attempt invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingCryptoError {
    InvalidChallenge,
    InvalidDeviceId,
    InvalidHostId,
    InvalidNonce,
    InvalidProof,
    KeyDerivation,
}

impl fmt::Display for PairingCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidChallenge => "pairing challenge is invalid",
            Self::InvalidDeviceId => "pairing device ID is invalid",
            Self::InvalidHostId => "pairing Host ID is invalid",
            Self::InvalidNonce => "pairing nonce is not canonical 32-byte base64",
            Self::InvalidProof => "pairing proof did not authenticate",
            Self::KeyDerivation => "session key derivation failed",
        };
        formatter.write_str(message)
    }
}

impl Error for PairingCryptoError {}

/// Produce the client proof preserved from the legacy Companion protocol.
///
/// UUID text is used exactly as supplied. Callers must not parse and
/// re-serialize it before calling this function because the compatibility
/// fixture intentionally fixes its casing.
pub fn legacy_client_proof(
    root_secret: &[u8; 32],
    challenge_id: &str,
    nonce_base64: &str,
    device_id: &str,
) -> Result<[u8; 32], PairingCryptoError> {
    validate_identifier(challenge_id, PairingCryptoError::InvalidChallenge)?;
    validate_identifier(device_id, PairingCryptoError::InvalidDeviceId)?;
    decode_canonical_nonce(nonce_base64)?;

    let payload = format!("{LEGACY_CLIENT_PROOF_DOMAIN}|{challenge_id}|{nonce_base64}|{device_id}");
    Ok(hmac_sha256(root_secret, payload.as_bytes()))
}

/// The immutable transcript bound into every production pairing proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTranscript {
    pub protocol_version: u32,
    pub host_id: String,
    pub device_id: String,
    pub challenge_id: String,
    pub client_nonce_base64: String,
    pub server_nonce_base64: String,
}

impl PairingTranscript {
    pub fn validate(&self) -> Result<(), PairingCryptoError> {
        if self.protocol_version != crate::PROTOCOL_VERSION {
            return Err(PairingCryptoError::InvalidChallenge);
        }
        validate_identifier(&self.host_id, PairingCryptoError::InvalidHostId)?;
        validate_identifier(&self.device_id, PairingCryptoError::InvalidDeviceId)?;
        validate_identifier(&self.challenge_id, PairingCryptoError::InvalidChallenge)?;
        decode_canonical_nonce(&self.client_nonce_base64)?;
        decode_canonical_nonce(&self.server_nonce_base64)?;
        Ok(())
    }

    fn proof_payload(&self, direction: PairingProofDirection) -> String {
        format!(
            "{PRODUCTION_PAIRING_DOMAIN}|{}|{}|{}|{}|{}|{}|{}",
            self.protocol_version,
            self.host_id,
            self.device_id,
            self.challenge_id,
            self.client_nonce_base64,
            self.server_nonce_base64,
            direction.as_str()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingProofDirection {
    HostChallenge,
    ClientProof,
    HostAccepted,
}

impl PairingProofDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HostChallenge => "host_challenge",
            Self::ClientProof => "client_proof",
            Self::HostAccepted => "host_accepted",
        }
    }
}

/// Produce a production mutual-authentication proof.
///
/// This is deliberately distinct from [`legacy_client_proof`]. The legacy
/// function exists only for the compatibility vector and does not authenticate
/// a production protocol-v1 connection.
pub fn production_pairing_proof(
    root_secret: &[u8; 32],
    transcript: &PairingTranscript,
    direction: PairingProofDirection,
) -> Result<[u8; 32], PairingCryptoError> {
    transcript.validate()?;
    Ok(hmac_sha256(
        root_secret,
        transcript.proof_payload(direction).as_bytes(),
    ))
}

pub fn production_pairing_proof_base64(
    root_secret: &[u8; 32],
    transcript: &PairingTranscript,
    direction: PairingProofDirection,
) -> Result<String, PairingCryptoError> {
    Ok(BASE64_STANDARD.encode(production_pairing_proof(
        root_secret,
        transcript,
        direction,
    )?))
}

pub fn verify_production_pairing_proof(
    root_secret: &[u8; 32],
    transcript: &PairingTranscript,
    direction: PairingProofDirection,
    proof: &[u8],
) -> Result<(), PairingCryptoError> {
    transcript.validate()?;
    verify_proof(
        root_secret,
        transcript.proof_payload(direction).as_bytes(),
        proof,
    )
}

/// Verify a proof without exposing comparison timing through ordinary equality.
pub fn verify_proof(
    root_secret: &[u8; 32],
    payload: &[u8],
    proof: &[u8],
) -> Result<(), PairingCryptoError> {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(root_secret).expect("HMAC accepts a 32-byte key");
    mac.update(payload);
    mac.verify_slice(proof)
        .map_err(|_| PairingCryptoError::InvalidProof)
}

fn hmac_sha256(root_secret: &[u8; 32], payload: &[u8]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(root_secret).expect("HMAC accepts a 32-byte key");
    mac.update(payload);
    mac.finalize().into_bytes().into()
}

/// Independent directional keys derived after mutual proof.
///
/// The key bytes are zeroized when this value is dropped. Accessors return
/// references so ordinary callers do not accidentally leave extra key copies.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKeys {
    client_to_host: [u8; 32],
    host_to_client: [u8; 32],
}

impl SessionKeys {
    pub fn client_to_host(&self) -> &[u8; 32] {
        &self.client_to_host
    }

    pub fn host_to_client(&self) -> &[u8; 32] {
        &self.host_to_client
    }

    pub fn into_directional_ciphers(mut self) -> DirectionalFrameCiphers {
        let client_to_host = self.client_to_host;
        let host_to_client = self.host_to_client;
        self.zeroize();
        DirectionalFrameCiphers {
            client_writer: EncryptedFrameWriter::new(client_to_host, FrameDirection::ClientToHost),
            client_reader: EncryptedFrameReader::new(host_to_client, FrameDirection::HostToClient),
        }
    }
}

/// Derive production session keys from both fresh nonces and the full pairing
/// transcript.
pub fn derive_production_session_keys(
    root_secret: &[u8; 32],
    transcript: &PairingTranscript,
) -> Result<SessionKeys, PairingCryptoError> {
    transcript.validate()?;
    let client_nonce = decode_canonical_nonce(&transcript.client_nonce_base64)?;
    let server_nonce = decode_canonical_nonce(&transcript.server_nonce_base64)?;
    let mut salt_input = [0_u8; 64];
    salt_input[..32].copy_from_slice(&client_nonce);
    salt_input[32..].copy_from_slice(&server_nonce);
    let salt = Sha256::digest(salt_input);
    salt_input.zeroize();

    let hkdf = Hkdf::<Sha256>::new(Some(&salt), root_secret);
    let context = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        PRODUCTION_SESSION_DOMAIN,
        transcript.protocol_version,
        transcript.host_id,
        transcript.device_id,
        transcript.challenge_id,
        transcript.client_nonce_base64,
        transcript.server_nonce_base64,
    );
    let mut client_to_host = [0_u8; 32];
    let mut host_to_client = [0_u8; 32];
    hkdf.expand(
        format!("{context}|client_to_host").as_bytes(),
        &mut client_to_host,
    )
    .map_err(|_| PairingCryptoError::KeyDerivation)?;
    hkdf.expand(
        format!("{context}|host_to_client").as_bytes(),
        &mut host_to_client,
    )
    .map_err(|_| PairingCryptoError::KeyDerivation)?;
    Ok(SessionKeys {
        client_to_host,
        host_to_client,
    })
}

fn validate_identifier(value: &str, error: PairingCryptoError) -> Result<(), PairingCryptoError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(char::is_control)
        || value.contains('|')
    {
        return Err(error);
    }
    Ok(())
}

fn decode_canonical_nonce(value: &str) -> Result<[u8; 32], PairingCryptoError> {
    let bytes = BASE64_STANDARD
        .decode(value)
        .map_err(|_| PairingCryptoError::InvalidNonce)?;
    let nonce: [u8; 32] = bytes
        .try_into()
        .map_err(|_| PairingCryptoError::InvalidNonce)?;
    if BASE64_STANDARD.encode(nonce) != value {
        return Err(PairingCryptoError::InvalidNonce);
    }
    Ok(nonce)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameDirection {
    ClientToHost = 1,
    HostToClient = 2,
}

impl FrameDirection {
    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptedFrameError {
    PayloadTooLarge,
    FrameTooShort,
    InvalidMagic,
    WrongDirection { expected: u8, actual: u8 },
    UnexpectedSequence { expected: u64, actual: u64 },
    AuthenticationFailed,
    SequenceExhausted,
}

impl fmt::Display for EncryptedFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("application payload exceeds 1 MiB"),
            Self::FrameTooShort => formatter.write_str("encrypted application frame is too short"),
            Self::InvalidMagic => {
                formatter.write_str("encrypted application frame magic is invalid")
            }
            Self::WrongDirection { expected, actual } => {
                write!(
                    formatter,
                    "frame direction {actual} does not match expected {expected}"
                )
            }
            Self::UnexpectedSequence { expected, actual } => {
                write!(
                    formatter,
                    "frame sequence {actual} does not match expected {expected}"
                )
            }
            Self::AuthenticationFailed => {
                formatter.write_str("encrypted application frame authentication failed")
            }
            Self::SequenceExhausted => formatter.write_str("encrypted frame sequence is exhausted"),
        }
    }
}

impl Error for EncryptedFrameError {}

/// Stateful sender for one traffic direction.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct EncryptedFrameWriter {
    key: [u8; 32],
    #[zeroize(skip)]
    direction: FrameDirection,
    next_sequence: u64,
}

impl EncryptedFrameWriter {
    pub fn new(key: [u8; 32], direction: FrameDirection) -> Self {
        Self {
            key,
            direction,
            next_sequence: 0,
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, EncryptedFrameError> {
        if plaintext.len() > MAX_APPLICATION_PAYLOAD_BYTES {
            return Err(EncryptedFrameError::PayloadTooLarge);
        }
        if self.next_sequence == u64::MAX {
            return Err(EncryptedFrameError::SequenceExhausted);
        }

        let sequence = self.next_sequence;
        let header = frame_header(self.direction, sequence);
        let nonce = frame_nonce(self.direction, sequence);
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &header,
                },
            )
            .map_err(|_| EncryptedFrameError::AuthenticationFailed)?;
        let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + ciphertext.len());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&ciphertext);
        self.next_sequence += 1;
        Ok(frame)
    }
}

/// Stateful receiver for one traffic direction.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct EncryptedFrameReader {
    key: [u8; 32],
    #[zeroize(skip)]
    direction: FrameDirection,
    next_sequence: u64,
}

impl EncryptedFrameReader {
    pub fn new(key: [u8; 32], direction: FrameDirection) -> Self {
        Self {
            key,
            direction,
            next_sequence: 0,
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, EncryptedFrameError> {
        if frame.len() < FRAME_HEADER_BYTES + AEAD_TAG_BYTES {
            return Err(EncryptedFrameError::FrameTooShort);
        }
        if &frame[..FRAME_MAGIC.len()] != FRAME_MAGIC {
            return Err(EncryptedFrameError::InvalidMagic);
        }
        let actual_direction = frame[4];
        let expected_direction = self.direction.wire_value();
        if actual_direction != expected_direction {
            return Err(EncryptedFrameError::WrongDirection {
                expected: expected_direction,
                actual: actual_direction,
            });
        }
        let actual_sequence = u64::from_be_bytes(
            frame[5..FRAME_HEADER_BYTES]
                .try_into()
                .expect("fixed frame sequence slice"),
        );
        if actual_sequence != self.next_sequence {
            return Err(EncryptedFrameError::UnexpectedSequence {
                expected: self.next_sequence,
                actual: actual_sequence,
            });
        }
        if self.next_sequence == u64::MAX {
            return Err(EncryptedFrameError::SequenceExhausted);
        }
        let ciphertext = &frame[FRAME_HEADER_BYTES..];
        if ciphertext.len().saturating_sub(AEAD_TAG_BYTES) > MAX_APPLICATION_PAYLOAD_BYTES {
            return Err(EncryptedFrameError::PayloadTooLarge);
        }

        let header: [u8; FRAME_HEADER_BYTES] = frame[..FRAME_HEADER_BYTES]
            .try_into()
            .expect("validated frame header length");
        let nonce = frame_nonce(self.direction, actual_sequence);
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad: &header,
                },
            )
            .map_err(|_| EncryptedFrameError::AuthenticationFailed)?;
        self.next_sequence += 1;
        Ok(plaintext)
    }
}

/// The pair needed by an iOS client after key derivation.
pub struct DirectionalFrameCiphers {
    pub client_writer: EncryptedFrameWriter,
    pub client_reader: EncryptedFrameReader,
}

fn frame_header(direction: FrameDirection, sequence: u64) -> [u8; FRAME_HEADER_BYTES] {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    header[..4].copy_from_slice(FRAME_MAGIC);
    header[4] = direction.wire_value();
    header[5..].copy_from_slice(&sequence.to_be_bytes());
    header
}

fn frame_nonce(direction: FrameDirection, sequence: u64) -> [u8; 12] {
    let mut nonce = [0_u8; 12];
    nonce[3] = direction.wire_value();
    nonce[4..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}
