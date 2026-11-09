use async_trait::async_trait;
use ed25519_dalek::SigningKey;

pub use crate::signing::AsyncSigner;
use crate::signing::{SignOptions, SignRequest, SignResponse, SignerDescriptor, SignerError};
use hkdf::Hkdf;
use rand_core::OsRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::binding::BindingPublicKey;
use crate::errors::IdentityError;
use crate::node_id::NodeId;
use crate::signature::SignatureBytes;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IdentitySeed([u8; 32]);

impl std::fmt::Debug for IdentitySeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdentitySeed([REDACTED])")
    }
}

impl IdentitySeed {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub trait SyncSigner {
    fn node_id(&self) -> NodeId;
    fn sign(&self, msg: &[u8]) -> SignatureBytes;
}

pub trait SignatureVerifier {
    fn verify_signature(
        &self,
        node_id: &NodeId,
        msg: &[u8],
        sig: &SignatureBytes,
    ) -> Result<(), IdentityError>;
}

#[derive(Clone)]
pub struct IdentityKeypair {
    signing_key: SigningKey,
    descriptor: SignerDescriptor,
}

impl IdentityKeypair {
    pub fn generate() -> Self {
        Self::from_signing_key(SigningKey::generate(&mut OsRng))
    }

    pub fn from_seed(seed: IdentitySeed) -> Self {
        Self::from_signing_key(SigningKey::from_bytes(seed.as_bytes()))
    }

    fn from_signing_key(signing_key: SigningKey) -> Self {
        let node_id = NodeId::from_bytes(*signing_key.verifying_key().as_bytes());
        let hk = Hkdf::<Sha256>::new(None, signing_key.as_bytes());
        let mut binding = [0u8; 32];
        hk.expand(b"polar-binding-v1", &mut binding)
            .expect("binding HKDF length");
        let secret = StaticSecret::from(binding);
        let public = X25519PublicKey::from(&secret);
        let descriptor =
            SignerDescriptor::software(node_id, BindingPublicKey::from_bytes(*public.as_bytes()));
        Self {
            signing_key,
            descriptor,
        }
    }

    pub fn node_id(&self) -> NodeId {
        NodeId::from_bytes(*self.signing_key.verifying_key().as_bytes())
    }

    pub fn binding_public_key(&self) -> BindingPublicKey {
        self.descriptor.binding_public_key
    }

    pub fn binding_secret_bytes(&self) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::new(None, self.signing_key.as_bytes());
        let mut key_bytes = [0u8; 32];
        hk.expand(b"polar-binding-v1", &mut key_bytes)
            .expect("HKDF expand for binding key");
        key_bytes
    }

    pub fn sign(&self, msg: &[u8]) -> SignatureBytes {
        use ed25519_dalek::Signer as _;
        SignatureBytes::from_bytes(self.signing_key.sign(msg).to_bytes())
    }
}

impl SyncSigner for IdentityKeypair {
    fn node_id(&self) -> NodeId {
        self.node_id()
    }

    fn sign(&self, msg: &[u8]) -> SignatureBytes {
        self.sign(msg)
    }
}

#[async_trait]
impl AsyncSigner for IdentityKeypair {
    fn descriptor(&self) -> &SignerDescriptor {
        &self.descriptor
    }

    async fn sign(
        &self,
        request: SignRequest,
        options: SignOptions,
    ) -> Result<SignResponse, SignerError> {
        self.descriptor.admit(&request)?;
        options.check_deadline()?;
        Ok(SignResponse {
            request_id: request.id(),
            signature: self.sign(request.canonical()),
        })
    }
}


