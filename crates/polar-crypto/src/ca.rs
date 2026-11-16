//! Fixed CA key operations. Separate from the NodeId Ed25519 signer and from
//! TPM sealing: a hardware implementation signs inside its selected device.
use async_trait::async_trait;
use sha2::{Digest, Sha256, Sha384};

use crate::PrivateOperationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaAlgorithm {
    P256Sha256,
    P384Sha384,
}

/// A structurally checked SEC1 uncompressed public point, never private key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaPublicKey {
    algorithm: CaAlgorithm,
    sec1: Vec<u8>,
}
impl CaPublicKey {
    /// Checks representation only; PKI verifies signatures against the curve point before
    /// accepting a certificate.
    pub fn from_sec1(algorithm: CaAlgorithm, sec1: Vec<u8>) -> Result<Self, PrivateOperationError> {
        let size = match algorithm {
            CaAlgorithm::P256Sha256 => 65,
            CaAlgorithm::P384Sha384 => 97,
        };
        if sec1.len() != size || sec1.first() != Some(&4) {
            return Err(PrivateOperationError::Rejected);
        }
        Ok(Self { algorithm, sec1 })
    }
    pub fn algorithm(&self) -> CaAlgorithm {
        self.algorithm
    }
    pub fn sec1(&self) -> &[u8] {
        &self.sec1
    }
}

/// Exactly one hash of the original TBS. Implementations MUST use ECDSA prehash
/// signing, never hash these bytes again or select a different key by purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaDigest {
    P256([u8; 32]),
    P384([u8; 48]),
}
impl CaDigest {
    pub fn from_tbs(algorithm: CaAlgorithm, tbs: &[u8]) -> Self {
        match algorithm {
            CaAlgorithm::P256Sha256 => Self::P256(Sha256::digest(tbs).into()),
            CaAlgorithm::P384Sha384 => Self::P384(Sha384::digest(tbs).into()),
        }
    }
    pub fn algorithm(&self) -> CaAlgorithm {
        match self {
            Self::P256(_) => CaAlgorithm::P256Sha256,
            Self::P384(_) => CaAlgorithm::P384Sha384,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::P256(bytes) => bytes,
            Self::P384(bytes) => bytes,
        }
    }
}

#[async_trait]
pub trait CaSigner: Send + Sync {
    /// The public key is immutable for this handle's entire lifetime.
    fn public_key(&self) -> &CaPublicKey;
    /// Return strict ASN.1 DER ECDSA (r, s), not a fixed Ed25519 signature.
    /// Reject algorithm mismatch. The caller verifies the result before use.
    async fn sign_digest(&self, digest: CaDigest) -> Result<Vec<u8>, PrivateOperationError>;
}
