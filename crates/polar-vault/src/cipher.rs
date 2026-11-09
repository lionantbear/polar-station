//! Legacy record cryptography, independent of persistence and bootstrap.
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use polar_identity::IdentitySeed;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

/// One startup-bound identity. Implementations must authenticate before
/// returning plaintext; there is no key export or identity replacement API.
#[async_trait]
pub trait VaultCipher: Send + Sync {
    async fn seal_bound(&self, _plain: &[u8], _context: &[u8]) -> Result<([u8; 12], Vec<u8>)> {
        anyhow::bail!("BoundV2 unsupported by Vault cipher")
    }
    async fn open_bound(
        &self,
        _nonce: &[u8],
        _ciphertext: &[u8],
        _context: &[u8],
    ) -> Result<Vec<u8>> {
        anyhow::bail!("BoundV2 unsupported by Vault cipher")
    }
    async fn seal(&self, plain: &[u8]) -> Result<([u8; 12], Vec<u8>)>;
    async fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>>;
    /// Authenticate the complete input before returning any plaintext. Device
    /// implementations batch within their negotiated capacity, never fall back
    /// to one device request per row.
    async fn open_batch(&self, records: &[VaultCiphertext<'_>]) -> Result<Vec<Vec<u8>>> {
        let mut values = Vec::with_capacity(records.len());
        for record in records {
            values.push(Zeroizing::new(
                self.open(record.nonce, record.ciphertext).await?,
            ));
        }
        Ok(values.into_iter().map(|value| value.to_vec()).collect())
    }
}

pub struct VaultCiphertext<'a> {
    pub nonce: &'a [u8],
    pub ciphertext: &'a [u8],
}

/// Original v1 HKDF and IETF ChaCha20-Poly1305, with no record AAD.
pub struct SoftwareVaultCipher(ChaCha20Poly1305);
impl SoftwareVaultCipher {
    pub fn seal_bound_record(&self, plain: &[u8], context: &[u8]) -> Result<([u8; 12], Vec<u8>)> {
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: context,
                },
            )
            .map_err(|_| anyhow!("encrypt secret: authentication cipher failed"))?;
        Ok((nonce, ciphertext))
    }
    pub fn open_bound_record(
        &self,
        nonce: &[u8],
        ciphertext: &[u8],
        context: &[u8],
    ) -> Result<Vec<u8>> {
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| anyhow!("decrypt secret: invalid nonce length"))?;
        self.0
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad: context,
                },
            )
            .map_err(|_| anyhow!("decrypt secret: authentication failed"))
    }
    pub fn new(seed: &IdentitySeed) -> Result<Self> {
        let hk = Hkdf::<Sha256>::new(None, seed.as_bytes());
        let mut bytes = Zeroizing::new([0; 32]);
        hk.expand(b"polar-vault-v1", bytes.as_mut())
            .map_err(|_| anyhow!("vault hkdf expand failed"))?;
        Ok(Self(ChaCha20Poly1305::new(Key::from_slice(bytes.as_ref()))))
    }
    pub fn seal_record(&self, plain: &[u8]) -> Result<([u8; 12], Vec<u8>)> {
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .0
            .encrypt(Nonce::from_slice(&nonce), plain)
            .map_err(|_| anyhow!("encrypt secret: authentication cipher failed"))?;
        Ok((nonce, ciphertext))
    }
    pub fn open_record(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| anyhow!("decrypt secret: invalid nonce length"))?;
        self.0
            .decrypt(Nonce::from_slice(&nonce), ciphertext)
            .map_err(|_| anyhow!("decrypt secret: authentication failed"))
    }
}
#[async_trait]
impl VaultCipher for SoftwareVaultCipher {
    async fn seal_bound(&self, plain: &[u8], context: &[u8]) -> Result<([u8; 12], Vec<u8>)> {
        self.seal_bound_record(plain, context)
    }
    async fn open_bound(&self, nonce: &[u8], ciphertext: &[u8], context: &[u8]) -> Result<Vec<u8>> {
        self.open_bound_record(nonce, ciphertext, context)
    }
    async fn seal(&self, plain: &[u8]) -> Result<([u8; 12], Vec<u8>)> {
        self.seal_record(plain)
    }
    async fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.open_record(nonce, ciphertext)
    }
}
