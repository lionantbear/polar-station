//! UUID credentials. Authorities are trusted assembly handles, not wire DTOs.
use crate::PolarVault;
use anyhow::{ensure, Result};
use async_trait::async_trait;
use polar_identity::NodeId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

mod device;
pub use device::Ed25519SecretExecutor;
mod cache;
pub use cache::{SecretUseSession, SecretUseTiming};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct SecretId(#[schemars(with = "String")] Uuid);
impl SecretId {
    pub fn from_uuid(id: Uuid) -> Result<Self> {
        ensure!(!id.is_nil(), "invalid secret UUID");
        Ok(Self(id))
    }
    pub fn uuid(self) -> Uuid {
        self.0
    }
}
impl std::fmt::Display for SecretId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[repr(u16)]
pub enum SecretKind {
    GitHttpBasic = 1,
    GitHttpBearer = 2,
    GitSshPrivateKey = 3,
}
impl TryFrom<u16> for SecretKind {
    type Error = anyhow::Error;
    fn try_from(value: u16) -> Result<Self> {
        match value {
            1 => Ok(Self::GitHttpBasic),
            2 => Ok(Self::GitHttpBearer),
            3 => Ok(Self::GitSshPrivateKey),
            _ => anyhow::bail!("unsupported credential kind"),
        }
    }
}

/// Bytes must arrive through a dedicated private input, never an LGP message.
/// No Serialize, Clone, Deref or content Debug implementation is provided.
pub struct SecretInput(Zeroizing<Vec<u8>>);
impl SecretInput {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let bytes = Zeroizing::new(bytes);
        ensure!(
            !bytes.is_empty() && bytes.len() <= 65_536,
            "invalid secret size"
        );
        Ok(Self(bytes))
    }
}
pub struct SecretLease {
    id: SecretId,
    version: u64,
    kind: SecretKind,
    bytes: Zeroizing<Vec<u8>>,
}
impl SecretLease {
    pub fn id(&self) -> SecretId {
        self.id
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn kind(&self) -> SecretKind {
        self.kind
    }
    /// Only the authorized consumer's transport endpoint may call this method.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::fmt::Debug for SecretLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretLease")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub id: SecretId,
    pub name: String,
    #[serde(with = "polar_trust::serde_value::node_id")]
    pub owner: NodeId,
    pub kind: SecretKind,
    pub version: u64,
    pub revision: u64,
    pub revoked: bool,
    /// Authenticated device-only use policy, never a host plaintext lease.
    #[serde(default)]
    pub device_only: bool,
}

/// Nonsecret discovery only; owner comes from the trusted authority handle.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SecretQuery {
    pub id: Option<SecretId>,
    pub name_contains: Option<String>,
    pub kind: Option<SecretKind>,
    pub include_revoked: bool,
    pub after: Option<SecretId>,
    pub limit: u16,
}
impl Default for SecretQuery {
    fn default() -> Self {
        Self {
            id: None,
            name_contains: None,
            kind: None,
            include_revoked: false,
            after: None,
            limit: 50,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretPage {
    pub items: Vec<SecretMetadata>,
    pub next_after: Option<SecretId>,
}
fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.trim() == name
            && name.chars().count() <= 128
            && !name.chars().any(char::is_control),
        "invalid credential name"
    );
    Ok(())
}

/// Internal persistence contract: contains ciphertext only. Not a request DTO.
pub struct StoredSecret {
    pub metadata: SecretMetadata,
    pub vault_id: Uuid,
    pub key_generation: u64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}
impl StoredSecret {
    pub fn authenticated_context(&self) -> Result<Vec<u8>> {
        ensure!(
            !self.vault_id.is_nil() && !self.metadata.id.uuid().is_nil(),
            "invalid secret identity"
        );
        ensure!(
            self.key_generation == 1 && self.metadata.version > 0,
            "unsupported secret generation"
        );
        let mut aad = if self.metadata.device_only {
            b"polar-vault-device-record-v1\0".to_vec()
        } else {
            b"polar-vault-record-v2\0".to_vec()
        };
        aad.extend_from_slice(self.vault_id.as_bytes());
        aad.extend_from_slice(self.metadata.id.uuid().as_bytes());
        aad.extend_from_slice(self.metadata.owner.as_bytes());
        aad.extend_from_slice(&(self.metadata.kind as u16).to_be_bytes());
        aad.extend_from_slice(&self.metadata.version.to_be_bytes());
        aad.extend_from_slice(&self.key_generation.to_be_bytes());
        Ok(aad)
    }
}

/// Construct ONLY after the caller has been authenticated and authorized by
/// the owning runtime. Never build this from an owner supplied in a request.
pub struct VaultAuthority {
    owner: NodeId,
}
impl VaultAuthority {
    pub(crate) fn owner(&self) -> NodeId {
        self.owner
    }
    pub fn for_authorized_owner(owner: NodeId) -> Self {
        Self { owner }
    }
}

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    fn subscribe_invalidation(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }
    async fn resolve_for_use(&self, id: SecretId) -> Result<SecretLease>;
    /// Session caches must validate revocation/version before each use without
    /// re-decrypting the value. Unknown resolvers fail closed for caching.
    async fn current_version(&self, _id: SecretId) -> Result<u64> {
        anyhow::bail!("credential resolver does not support session freshness")
    }
}
struct BoundCredentialResolver {
    vault: Arc<PolarVault>,
    owner: NodeId,
    kind: SecretKind,
}
#[async_trait]
impl CredentialResolver for BoundCredentialResolver {
    fn subscribe_invalidation(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(self.vault.secret_changes.subscribe())
    }
    async fn current_version(&self, id: SecretId) -> Result<u64> {
        let record = self.vault.authorized_secret(self.owner, id).await?;
        ensure!(
            !record.metadata.revoked
                && !record.metadata.device_only
                && record.metadata.kind == self.kind,
            "credential unavailable"
        );
        Ok(record.metadata.version)
    }
    async fn resolve_for_use(&self, id: SecretId) -> Result<SecretLease> {
        let record = self.vault.authorized_secret(self.owner, id).await?;
        ensure!(!record.metadata.revoked, "credential unavailable");
        ensure!(
            record.metadata.kind == self.kind,
            "credential kind mismatch"
        );
        self.vault.open_uuid_secret(record).await
    }
}

impl PolarVault {
    pub async fn query_secrets(
        &self,
        authority: &VaultAuthority,
        query: SecretQuery,
    ) -> Result<SecretPage> {
        ensure!(
            (1..=100).contains(&query.limit),
            "invalid credential query limit"
        );
        if let Some(name) = &query.name_contains {
            validate_name(name)?;
        }
        ensure!(
            query.id.is_none_or(|id| !id.uuid().is_nil())
                && query.after.is_none_or(|id| !id.uuid().is_nil()),
            "invalid credential query UUID"
        );
        let limit = usize::from(query.limit);
        let mut items = self
            .store
            .query_uuid_secrets(authority.owner, query)
            .await?;
        ensure!(
            items.iter().all(|row| row.owner == authority.owner),
            "credential query owner mismatch"
        );
        let next_after = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|row| row.id)
        } else {
            None
        };
        Ok(SecretPage { items, next_after })
    }

    /// The owner/kind are fixed at trusted consumer assembly, not per Tool call.
    pub fn bind_credential_resolver(
        self: &Arc<Self>,
        authority: &VaultAuthority,
        kind: SecretKind,
    ) -> Arc<dyn CredentialResolver> {
        Arc::new(BoundCredentialResolver {
            vault: self.clone(),
            owner: authority.owner,
            kind,
        })
    }
    async fn authorized_secret(&self, owner: NodeId, id: SecretId) -> Result<StoredSecret> {
        ensure!(!id.uuid().is_nil(), "credential unavailable");
        let record = self
            .store
            .load_uuid_secret(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("credential unavailable"))?;
        ensure!(
            record.metadata.id == id && record.metadata.owner == owner,
            "credential unavailable"
        );
        ensure!(
            record.vault_id == self.store.uuid_vault_identity().await?,
            "credential unavailable"
        );
        Ok(record)
    }
    async fn open_uuid_secret(&self, record: StoredSecret) -> Result<SecretLease> {
        ensure!(
            !record.metadata.device_only,
            "device-only secret cannot become a host lease"
        );
        let context = record.authenticated_context()?;
        let bytes = Zeroizing::new(
            self.cipher
                .open_bound(&record.nonce, &record.ciphertext, &context)
                .await?,
        );
        ensure!(
            !bytes.is_empty() && bytes.len() <= 65_536,
            "invalid credential material"
        );
        Ok(SecretLease {
            id: record.metadata.id,
            version: record.metadata.version,
            kind: record.metadata.kind,
            bytes,
        })
    }
    pub async fn inspect_secret(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
    ) -> Result<SecretMetadata> {
        Ok(self.authorized_secret(authority.owner, id).await?.metadata)
    }
    pub async fn import_secret(
        &self,
        authority: &VaultAuthority,
        import_id: Uuid,
        name: &str,
        kind: SecretKind,
        input: SecretInput,
    ) -> Result<SecretId> {
        self.import_secret_as(authority, import_id, name, kind, input, None)
            .await
    }
    /// Explicit private initializer path. Normal use never returns this seed.
    pub async fn import_device_ed25519_seed(
        &self,
        authority: &VaultAuthority,
        import_id: Uuid,
        name: &str,
        input: SecretInput,
        executor: &dyn Ed25519SecretExecutor,
    ) -> Result<SecretId> {
        ensure!(
            input.0.len() == 32
                && executor.compute_location() == crate::protection::SecretCompute::Device,
            "device Ed25519 import requires a device executor and 32-byte seed"
        );
        self.import_secret_as(
            authority,
            import_id,
            name,
            SecretKind::GitSshPrivateKey,
            input,
            Some(executor),
        )
        .await
    }
    async fn import_secret_as(
        &self,
        authority: &VaultAuthority,
        import_id: Uuid,
        name: &str,
        kind: SecretKind,
        input: SecretInput,
        device: Option<&dyn Ed25519SecretExecutor>,
    ) -> Result<SecretId> {
        ensure!(!import_id.is_nil(), "invalid import UUID");
        validate_name(name)?;
        if let Some(id) = self
            .store
            .find_uuid_import(authority.owner, import_id)
            .await?
        {
            return self
                .check_import(authority, id, name, kind, &input, device)
                .await;
        }
        let id = SecretId(polar_identity::generation::new_uuid_v7());
        let mut record = StoredSecret {
            metadata: SecretMetadata {
                id,
                name: name.to_owned(),
                owner: authority.owner,
                kind,
                version: 1,
                revision: 1,
                revoked: false,
                device_only: device.is_some(),
            },
            vault_id: self.store.uuid_vault_identity().await?,
            key_generation: 1,
            nonce: Vec::new(),
            ciphertext: Vec::new(),
        };
        let (nonce, ciphertext) = self
            .cipher
            .seal_bound(&input.0, &record.authenticated_context()?)
            .await?;
        record.nonce = nonce.to_vec();
        record.ciphertext = ciphertext;
        if let Some(executor) = device {
            verify_device_import(&record, &input, executor).await?;
        }
        let inserted = self.store.insert_uuid_secret(record, import_id).await?;
        if inserted != id {
            return self
                .check_import(authority, inserted, name, kind, &input, device)
                .await;
        }
        Ok(id)
    }
    async fn check_import(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        name: &str,
        kind: SecretKind,
        input: &SecretInput,
        device: Option<&dyn Ed25519SecretExecutor>,
    ) -> Result<SecretId> {
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !record.metadata.revoked
                && record.metadata.kind == kind
                && record.metadata.version == 1
                && record.metadata.name == name
                && record.metadata.device_only == device.is_some(),
            "secret import conflict"
        );
        if let Some(executor) = device {
            verify_device_import(&record, input, executor).await?;
            return Ok(id);
        }
        let lease = self.open_uuid_secret(record).await?;
        ensure!(
            lease.bytes.as_slice() == input.0.as_slice(),
            "secret import conflict"
        );
        Ok(id)
    }
    pub async fn rotate_secret(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        expected_version: u64,
        input: SecretInput,
    ) -> Result<u64> {
        let mut record = self.authorized_secret(authority.owner, id).await?;
        ensure!(!record.metadata.revoked, "credential unavailable");
        ensure!(
            !record.metadata.device_only || input.0.len() == 32,
            "device key must retain 32-byte Ed25519 encoding"
        );
        ensure!(
            record.metadata.version == expected_version,
            "secret version conflict"
        );
        let revision = record.metadata.revision;
        record.metadata.version = record
            .metadata
            .version
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| anyhow::anyhow!("secret version exhausted"))?;
        record.metadata.revision = revision
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| anyhow::anyhow!("secret revision exhausted"))?;
        let (nonce, ciphertext) = self
            .cipher
            .seal_bound(&input.0, &record.authenticated_context()?)
            .await?;
        record.nonce = nonce.to_vec();
        record.ciphertext = ciphertext;
        let version = record.metadata.version;
        ensure!(
            self.store.rotate_uuid_secret(record, revision).await?,
            "secret version conflict"
        );
        self.secret_changes.send_modify(|v| *v = v.wrapping_add(1));
        Ok(version)
    }
    pub async fn revoke_secret(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        expected_revision: u64,
    ) -> Result<()> {
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !record.metadata.revoked && record.metadata.revision == expected_revision,
            "secret revision conflict"
        );
        ensure!(
            self.store
                .revoke_uuid_secret(id, authority.owner, expected_revision)
                .await?,
            "secret revision conflict"
        );
        self.secret_changes.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }
}

/// Nonsecret header for the dedicated private-input transport.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretImportMetadata {
    pub import_id: Uuid,
    pub name: String,
    pub kind: SecretKind,
    #[serde(default)]
    pub device_only: bool,
}

async fn verify_device_import(
    record: &StoredSecret,
    input: &SecretInput,
    executor: &dyn Ed25519SecretExecutor,
) -> Result<()> {
    use polar_identity::{
        signed_identity::NodeIdSignatureVerifier, IdentityKeypair, IdentitySeed, SignatureVerifier,
    };
    let bytes = Zeroizing::new(
        <[u8; 32]>::try_from(input.0.as_slice())
            .map_err(|_| anyhow::anyhow!("invalid device key"))?,
    );
    let expected = IdentityKeypair::from_seed(IdentitySeed::from_bytes(*bytes)).node_id();
    drop(bytes);
    let proof = b"polar-device-secret-import-v1";
    let signature = executor.sign_record(record, expected, proof).await?;
    NodeIdSignatureVerifier.verify_signature(&expected, proof, &signature)?;
    Ok(())
}
