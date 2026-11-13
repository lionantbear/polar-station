//! One authenticated directory, loaded once; mutations use the existing Vault
//! adapter's compare-and-swap transaction. Signing never updates this directory.
use super::*;
use crate::{VaultCipher, VaultStore};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroizing;

pub struct StoredDirectory {
    pub revision: u64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolAssignment {
    pub key: SecretId,
    pub scope: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyPool {
    pub id: Uuid,
    pub algorithm: KeyAlgorithm,
    pub purpose: KeyPurpose,
    pub namespace: u32,
    pub keys: Vec<SecretId>,
    pub assignments: Vec<PoolAssignment>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDirectory {
    pub keys: Vec<KeyRecord>,
    pub pools: Vec<KeyPool>,
}

/// One source for SDK/CLI/UI rows. These are authenticated configuration views;
/// neither presence here nor an H/C3 label proves a live hardware capability.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyDirectoryView {
    pub revision: u64,
    pub keys: Vec<KeyView>,
    pub pools: Vec<KeyPoolView>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyConfigurationQuery {}

pub(crate) struct ManagedKeyDirectory {
    owner: NodeId,
    capabilities: ProtectionCapabilities,
    directory: Arc<KeyDirectory>,
}

impl crate::PolarVault {
    /// Shared credential-operation gate. Product assembly MUST authenticate
    /// managed_keys before publishing consumers. An unconfigured standalone
    /// Vault retains its existing authority model, not a new implicit key policy.
    pub async fn authorize_secret_operation(
        &self,
        authority: &crate::VaultAuthority,
        id: SecretId,
        operation: crate::policy::SecretOperation,
    ) -> Result<()> {
        if let Some(managed) = self.key_directory.get() {
            ensure!(
                managed.owner == authority.owner(),
                "key policy owner mismatch"
            );
            let state = managed.directory.state.lock().await;
            let snapshot = state
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
            if let Some(key) = snapshot.directory.keys.iter().find(|key| key.id == id) {
                key.policy.require(operation)?;
            }
        }
        Ok(())
    }
    /// Startup/authorized management assembly shares this single directory.
    /// Neither authority nor capabilities may come from an SDK request.
    pub async fn managed_keys(
        &self,
        authority: &crate::VaultAuthority,
        capabilities: ProtectionCapabilities,
    ) -> Result<Arc<KeyDirectory>> {
        let owner = authority.owner();
        let managed = self
            .key_directory
            .get_or_try_init(|| async {
                Ok::<_, anyhow::Error>(ManagedKeyDirectory {
                    owner,
                    capabilities,
                    directory: Arc::new(
                        KeyDirectory::open(
                            owner,
                            capabilities,
                            self.store.clone(),
                            self.cipher.clone(),
                        )
                        .await?,
                    ),
                })
            })
            .await?;
        ensure!(
            managed.owner == owner && managed.capabilities == capabilities,
            "fixed key directory owner/capability mismatch"
        );
        Ok(managed.directory.clone())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyView {
    pub record: KeyRecord,
    /// Primary copy first, then all declared backups, with no scalar score.
    pub copies: Vec<SecretCopyView>,
    pub pool: Option<PoolKeyView>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolKeyView {
    pub id: Uuid,
    /// None is a prepared, unassigned slot, never a reusable used slot.
    pub assigned_scope: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyPoolView {
    pub id: Uuid,
    pub algorithm: KeyAlgorithm,
    pub purpose: KeyPurpose,
    pub namespace: u32,
    pub assigned: usize,
    pub available: usize,
}
impl RecoveryDirectory {
    pub fn validate(&self, owner: NodeId, caps: ProtectionCapabilities) -> Result<()> {
        ensure!(
            self.keys.len() <= 128 && self.pools.len() <= 32,
            "directory exceeds capacity"
        );
        for (i, k) in self.keys.iter().enumerate() {
            k.validate(owner, caps)?;
            ensure!(
                !self.keys[..i].iter().any(|v| v.id == k.id
                    || (v.algorithm == k.algorithm && v.public_key == k.public_key)),
                "duplicate key identity"
            );
            if let Some(object) = k.hardware_object {
                ensure!(
                    !self.keys[..i]
                        .iter()
                        .any(|v| v.hardware_object == Some(object)),
                    "duplicate hardware key object"
                );
            }
            if let KeyRecovery::RootWordsAndPath { path } = &k.recovery {
                ensure!(
                    !self.keys[..i].iter().any(|v| matches!(
                        &v.recovery, KeyRecovery::RootWordsAndPath { path: other } if other == path
                    )),
                    "duplicate key recovery path"
                );
            }
        }
        for (index, pool) in self.pools.iter().enumerate() {
            ensure!(
                !pool.id.is_nil()
                    && !pool.keys.is_empty()
                    && pool.keys.len() <= 32
                    && pool.assignments.len() <= pool.keys.len(),
                "invalid pool capacity"
            );
            ensure!(
                !self.pools[..index].iter().any(|p| p.id == pool.id
                    || (p.algorithm == pool.algorithm
                        && p.purpose == pool.purpose
                        && p.namespace == pool.namespace)),
                "duplicate pool namespace"
            );
            for (j, id) in pool.keys.iter().enumerate() {
                ensure!(!pool.keys[..j].contains(id), "duplicate pool key");
                ensure!(
                    !self.pools[..index].iter().any(|p| p.keys.contains(id)),
                    "key belongs to another pool"
                );
                let record = self
                    .keys
                    .iter()
                    .find(|k| k.id == *id)
                    .ok_or_else(|| anyhow::anyhow!("missing pool key"))?;
                ensure!(
                    record.algorithm == pool.algorithm
                        && record.purpose == pool.purpose
                        && record.management == KeyManagement::DerivedImported,
                    "pool key domain mismatch"
                );
                let KeyRecovery::RootWordsAndPath { path } = &record.recovery else {
                    anyhow::bail!("pool key has no recovery path")
                };
                ensure!(
                    path.namespace == pool.namespace,
                    "pool path namespace mismatch"
                );
                for other in &pool.keys[..j] {
                    let other = self
                        .keys
                        .iter()
                        .find(|k| k.id == *other)
                        .expect("previous pool key checked");
                    ensure!(
                        other.recovery != record.recovery,
                        "duplicate pool derivation path"
                    );
                }
            }
            for (j, a) in pool.assignments.iter().enumerate() {
                ensure!(
                    pool.keys.contains(&a.key)
                        && !a.scope.is_empty()
                        && a.scope.len() <= 253
                        && !a.scope.contains('\0'),
                    "invalid pool assignment"
                );
                ensure!(
                    !pool.assignments[..j]
                        .iter()
                        .any(|v| v.key == a.key || v.scope == a.scope),
                    "duplicate pool assignment"
                );
            }
        }
        Ok(())
    }
}
struct Snapshot {
    revision: u64,
    directory: RecoveryDirectory,
}
pub struct KeyDirectory {
    owner: NodeId,
    caps: ProtectionCapabilities,
    store: Arc<dyn VaultStore>,
    cipher: Arc<dyn VaultCipher>,
    state: Mutex<Option<Snapshot>>,
}
fn context(owner: NodeId, revision: u64) -> Vec<u8> {
    let mut out = b"polar-key-directory-v1\0".to_vec();
    out.extend_from_slice(owner.as_bytes());
    out.extend_from_slice(&revision.to_be_bytes());
    out
}
impl KeyDirectory {
    /// Check the current authenticated RAM policy. No database or device I/O.
    /// Trusted callers must also retain their existing owner/purpose checks.
    pub async fn authorize_ssh_use(&self, id: SecretId, public: NodeId) -> Result<()> {
        self.authorize_ssh_grant(id, public, None).await
    }
    pub async fn authorize_ssh_configuration(&self, expected: &KeyRecord) -> Result<()> {
        let public = NodeId::from_bytes(
            expected
                .public_key
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid SSH verification key"))?,
        );
        self.authorize_ssh_grant(expected.id, public, Some(expected))
            .await
    }
    async fn authorize_ssh_grant(
        &self,
        id: SecretId,
        public: NodeId,
        expected: Option<&KeyRecord>,
    ) -> Result<()> {
        let state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let key = old
            .directory
            .keys
            .iter()
            .find(|k| k.id == id)
            .ok_or_else(|| anyhow::anyhow!("SSH key unavailable"))?;
        ensure!(
            key.owner == self.owner
                && key.purpose == KeyPurpose::Ssh
                && key.public_key == public.as_bytes(),
            "SSH policy identity mismatch"
        );
        ensure!(
            expected.is_none_or(|record| key.same_ssh_configuration(record)),
            "SSH execution configuration changed"
        );
        key.policy.require(crate::policy::SecretOperation::Use)
    }
    fn prepare_ssh_leaf(
        &self,
        old: &Snapshot,
        record: KeyRecord,
    ) -> Result<Option<RecoveryDirectory>> {
        ensure!(
            record.purpose == KeyPurpose::Ssh
                && record.algorithm == KeyAlgorithm::Ed25519
                && record.management == KeyManagement::VaultStored
                && record.recovery == (KeyRecovery::VaultBackup { id: record.id })
                && record.hardware_object.is_none(),
            "unsupported SSH leaf configuration"
        );
        use crate::protection::{
            SecretCompute, SecretProtection, SecretRetention, SecretStorage, VaultLocation,
        };
        let device_copy = record.protection.vault == Some(VaultLocation::Device);
        let allowed_backup = SecretProtection {
            storage: SecretStorage::Vault,
            compute: SecretCompute::Device,
            retention: SecretRetention::Operation,
            vault: Some(VaultLocation::Host),
        };
        ensure!(
            if device_copy {
                record.protection.compute == SecretCompute::Device
                    && record.other_copies == [allowed_backup]
            } else {
                record.other_copies.is_empty()
            },
            "SSH backup policy mismatch"
        );
        let mut next = old.directory.clone();
        ensure!(
            !next.pools.iter().any(|p| p.keys.contains(&record.id)),
            "cannot reconfigure pool key"
        );
        if let Some(existing) = next.keys.iter_mut().find(|key| key.id == record.id) {
            ensure!(
                existing.owner == record.owner
                    && existing.purpose == record.purpose
                    && existing.algorithm == record.algorithm
                    && existing.public_key == record.public_key
                    && existing.management == record.management
                    && existing.recovery == record.recovery
                    && (existing.other_copies == record.other_copies
                        || (existing.other_copies.is_empty() && device_copy))
                    && existing.hardware_object == record.hardware_object,
                "cannot replace SSH key identity or recovery"
            );
            if existing.policy == record.policy
                && existing.protection == record.protection
                && existing.scope == record.scope
                && existing.other_copies == record.other_copies
            {
                return Ok(None);
            }
            // Protection, backups and scope are policy changes too. Keeping the
            // permissions field identical must not bypass its own change lock.
            existing
                .policy
                .require(crate::policy::SecretOperation::PolicyChange)?;
            *existing = record;
        } else {
            next.keys.push(record);
        }
        next.validate(self.owner, self.caps)?;
        Ok(Some(next))
    }
    pub async fn preflight_ssh_leaf(
        &self,
        record: KeyRecord,
        expected_revision: u64,
    ) -> Result<()> {
        let state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(
            old.revision == expected_revision,
            "directory revision changed"
        );
        self.prepare_ssh_leaf(old, record)?;
        Ok(())
    }
    pub async fn configure_ssh_leaf(
        &self,
        record: KeyRecord,
        expected_revision: u64,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(
            old.revision == expected_revision,
            "directory revision changed"
        );
        if let Some(next) = self.prepare_ssh_leaf(old, record)? {
            self.commit(&mut state, next).await?;
        }
        Ok(())
    }
    /// An exact private-input retry can authenticate an uncertain committed
    /// directory without restarting the host. Never publish an empty/older or
    /// differently configured directory merely because a commit was unknown.
    pub(crate) async fn recover_created_ssh(
        &self,
        expected: &KeyRecord,
        previous_revision: u64,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.is_some() {
            return Ok(());
        }
        let minimum = previous_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("directory revision exhausted"))?;
        let mut fresh = Self::open(
            self.owner,
            self.caps,
            self.store.clone(),
            self.cipher.clone(),
        )
        .await?;
        let snapshot = fresh
            .state
            .get_mut()
            .take()
            .ok_or_else(|| anyhow::anyhow!("directory recovery unavailable"))?;
        ensure!(
            snapshot.revision >= minimum
                && snapshot.directory.keys.iter().any(|key| key == expected),
            "SSH creation recovery does not match committed policy"
        );
        *state = Some(snapshot);
        Ok(())
    }
    pub(crate) async fn create_ssh_leaf(
        &self,
        record: KeyRecord,
        secret: crate::StoredSecret,
        import: Uuid,
        expected: u64,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(old.revision == expected, "directory revision changed");
        ensure!(
            secret.metadata.id == record.id
                && secret.metadata.owner == self.owner
                && secret.metadata.kind == crate::SecretKind::GitSshPrivateKey
                && !old.directory.keys.iter().any(|k| k.id == record.id),
            "invalid new SSH credential"
        );
        let next = self
            .prepare_ssh_leaf(old, record)?
            .ok_or_else(|| anyhow::anyhow!("SSH creation must add a new key"))?;
        self.commit_with_secret(&mut state, next, Some((secret, import)))
            .await
    }
    pub async fn open(
        owner: NodeId,
        caps: ProtectionCapabilities,
        store: Arc<dyn VaultStore>,
        cipher: Arc<dyn VaultCipher>,
    ) -> Result<Self> {
        let (revision, directory) = match store.load_key_directory(owner).await? {
            Some(row) => {
                ensure!(
                    row.revision > 0 && row.ciphertext.len() <= 65536,
                    "invalid directory envelope"
                );
                let bytes = Zeroizing::new(
                    cipher
                        .open_bound(&row.nonce, &row.ciphertext, &context(owner, row.revision))
                        .await?,
                );
                (
                    row.revision,
                    serde_json::from_slice::<RecoveryDirectory>(&bytes)?,
                )
            }
            None => (0, RecoveryDirectory::default()),
        };
        directory.validate(owner, caps)?;
        Ok(Self {
            owner,
            caps,
            store,
            cipher,
            state: Mutex::new(Some(Snapshot {
                revision,
                directory,
            })),
        })
    }
    pub async fn snapshot(&self) -> Result<RecoveryDirectory> {
        let state = self.state.lock().await;
        Ok(state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?
            .directory
            .clone())
    }
    /// In-memory projection only: no database queries, decryptions, device
    /// operations or writes. Unknown commit state refuses this view too.
    pub async fn view(&self) -> Result<KeyDirectoryView> {
        let state = self.state.lock().await;
        let snapshot = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let directory = &snapshot.directory;
        let keys = directory
            .keys
            .iter()
            .map(|key| {
                let pool = directory.pools.iter().find(|p| p.keys.contains(&key.id));
                let assignment = pool.and_then(|p| p.assignments.iter().find(|a| a.key == key.id));
                let mut record = key.clone();
                if let Some(assignment) = assignment {
                    record.scope.clone_from(&assignment.scope);
                }
                KeyView {
                    record,
                    copies: std::iter::once(&key.protection)
                        .chain(&key.other_copies)
                        .map(SecretProtection::configured_copy)
                        .collect(),
                    pool: pool.map(|p| PoolKeyView {
                        id: p.id,
                        assigned_scope: assignment.map(|a| a.scope.clone()),
                    }),
                }
            })
            .collect();
        Ok(KeyDirectoryView {
            revision: snapshot.revision,
            keys,
            pools: directory
                .pools
                .iter()
                .map(|p| KeyPoolView {
                    id: p.id,
                    algorithm: p.algorithm,
                    purpose: p.purpose,
                    namespace: p.namespace,
                    assigned: p.assignments.len(),
                    available: p.keys.len() - p.assignments.len(),
                })
                .collect(),
        })
    }
    /// Only authorized initializer/management assembly may install records.
    /// This does not import, generate or delete any hardware object.
    pub async fn install(&self, keys: Vec<KeyRecord>, pools: Vec<KeyPool>) -> Result<()> {
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let mut next = old.directory.clone();
        next.keys.extend(keys);
        next.pools.extend(pools);
        next.validate(self.owner, self.caps)?;
        self.commit(&mut state, next).await
    }
    /// The initializer has already provisioned and verified these references.
    /// Used slots are retained forever; replenishment never reassigns a key.
    pub async fn replenish(&self, pool_id: Uuid, keys: Vec<KeyRecord>) -> Result<()> {
        ensure!(!keys.is_empty(), "empty pool replenishment");
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let mut next = old.directory.clone();
        let pool = next
            .pools
            .iter_mut()
            .find(|p| p.id == pool_id)
            .ok_or_else(|| anyhow::anyhow!("pool unavailable"))?;
        ensure!(
            pool.keys.len() + keys.len() <= 32,
            "pool directory capacity exhausted"
        );
        pool.keys.extend(keys.iter().map(|k| k.id));
        next.keys.extend(keys);
        next.validate(self.owner, self.caps)?;
        self.commit(&mut state, next).await
    }

    pub async fn allocate(&self, pool_id: Uuid, scope: &str) -> Result<SecretId> {
        ensure!(
            !scope.is_empty() && scope.len() <= 253 && !scope.contains('\0'),
            "invalid assignment scope"
        );
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let mut next = old.directory.clone();
        let pool = next
            .pools
            .iter_mut()
            .find(|p| p.id == pool_id)
            .ok_or_else(|| anyhow::anyhow!("pool unavailable"))?;
        if let Some(existing) = pool.assignments.iter().find(|a| a.scope == scope) {
            return Ok(existing.key);
        }
        let key = *pool
            .keys
            .iter()
            .find(|id| !pool.assignments.iter().any(|a| a.key == **id))
            .ok_or_else(|| anyhow::anyhow!("key pool exhausted; replenish explicitly"))?;
        pool.assignments.push(PoolAssignment {
            key,
            scope: scope.into(),
        });
        next.validate(self.owner, self.caps)?;
        self.commit(&mut state, next).await?;
        Ok(key)
    }
    async fn commit(&self, state: &mut Option<Snapshot>, next: RecoveryDirectory) -> Result<()> {
        self.commit_with_secret(state, next, None).await
    }
    async fn commit_with_secret(
        &self,
        state: &mut Option<Snapshot>,
        next: RecoveryDirectory,
        secret: Option<(crate::StoredSecret, Uuid)>,
    ) -> Result<()> {
        let revision = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?
            .revision;
        let next_revision = revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("directory revision exhausted"))?;
        let bytes = Zeroizing::new(serde_json::to_vec(&next)?);
        ensure!(bytes.len() <= 65508, "directory exceeds cipher capacity");
        let (nonce, ciphertext) = self
            .cipher
            .seal_bound(&bytes, &context(self.owner, next_revision))
            .await?;
        // Take state BEFORE the await. Cancellation/unknown commit status must
        // not leave stale free keys usable; reopening authenticates durable state.
        *state = None;
        let record = StoredDirectory {
            revision: next_revision,
            nonce: nonce.to_vec(),
            ciphertext,
        };
        let updated = if let Some((secret, import)) = secret {
            self.store
                .create_uuid_secret_with_directory(secret, import, revision, record)
                .await?
        } else {
            self.store
                .compare_exchange_key_directory(self.owner, revision, record)
                .await?
        };
        ensure!(updated, "directory changed concurrently; reload required");
        *state = Some(Snapshot {
            revision: next_revision,
            directory: next,
        });
        Ok(())
    }
}
