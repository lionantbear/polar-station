//! Fixed-recipient operation handles. Long-term private bytes are not arguments.
use crate::{BindingSealedMessage, CryptoError};
use async_trait::async_trait;
use polar_identity::BindingPublicKey;
use std::{fmt, sync::Arc};
use thiserror::Error;
use zeroize::Zeroizing;

pub struct SecretBytes(Zeroizing<Vec<u8>>);
impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn from_zeroizing(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self(bytes)
    }
    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}
impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}

pub struct BindingOpenRequest {
    pub sealed: BindingSealedMessage,
    pub context: Arc<[u8]>,
}

#[derive(Debug, Error)]
pub enum PrivateOperationError {
    #[error("private operation handle closed")]
    Closed,
    #[error("private operation not implemented by this backend")]
    Unsupported,
    #[error("private operation rejected")]
    Rejected,
    #[error("private operation backend: {0}")]
    Backend(#[from] polar_identity::SignerError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

#[async_trait]
pub trait BindingOpener: Send + Sync {
    fn public_key(&self) -> BindingPublicKey;
    async fn open(&self, request: BindingOpenRequest)
        -> Result<SecretBytes, PrivateOperationError>;
}
