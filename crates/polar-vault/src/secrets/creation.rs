//! Private credential creation. The transport owns framing; this domain owns
//! validation, seed handling and atomic publication with authenticated policy.
use super::*;
use crate::{keys::*, policy::SecretPolicy, protection::*};
use polar_identity::{IdentityKeypair, IdentitySeed};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshSecretCreation {
    pub scope: String,
    pub protection: SecretProtection,
    pub policy: SecretPolicy,
    pub expected_revision: u64,
}
impl SshSecretCreation {
    pub fn validate(&self, caps: ProtectionCapabilities) -> Result<()> {
        ensure!(
            !self.scope.is_empty() && self.scope.len() <= 128 && !self.scope.contains('\0'),
            "invalid SSH scope"
        );
        self.policy.validate_connected_ssh()?;
        self.protection
            .validate(caps)
            .map_err(|_| anyhow::anyhow!("unsupported SSH creation protection"))?;
        ensure!(
            self.protection.vault == Some(VaultLocation::Host)
                && matches!(
                    (self.protection.storage, self.protection.compute),
                    (
                        SecretStorage::Vault,
                        SecretCompute::Host | SecretCompute::Device
                    ) | (SecretStorage::HostMemory, SecretCompute::Host)
                ),
            "SSH creation protection is not connected"
        );
        Ok(())
    }
    fn record(&self, owner: NodeId, id: SecretId, public: NodeId) -> KeyRecord {
        KeyRecord {
            id,
            owner,
            purpose: KeyPurpose::Ssh,
            algorithm: KeyAlgorithm::Ed25519,
            management: KeyManagement::VaultStored,
            protection: self.protection,
            policy: self.policy.clone(),
            other_copies: vec![],
            recovery: KeyRecovery::VaultBackup { id },
            public_key: public.as_bytes().to_vec(),
            scope: self.scope.clone(),
            hardware_object: None,
        }
    }
}
impl SecretImportMetadata {
    pub fn validate(&self, caps: ProtectionCapabilities) -> Result<()> {
        ensure!(!self.import_id.is_nil(), "invalid import UUID");
        validate_name(&self.name)?;
        if self.device_only {
            ensure!(
                self.kind == SecretKind::GitSshPrivateKey && caps.device,
                "device input unavailable"
            );
        }
        if let Some(ssh) = &self.ssh {
            ssh.validate(caps)?;
            ensure!(
                self.kind == SecretKind::GitSshPrivateKey
                    && self.device_only == (ssh.protection.compute == SecretCompute::Device),
                "SSH creation executor mismatch"
            );
        }
        Ok(())
    }
}

/// Trusted, fixed-owner port for the existing private input channel. Neither
/// the serialized header nor a standard SSH client supplies this authority.
#[async_trait]
pub trait SecretImportPort: Send + Sync {
    async fn admit(&self, metadata: &SecretImportMetadata) -> Result<()>;
    async fn import(
        &self,
        metadata: SecretImportMetadata,
        input: SecretInput,
    ) -> Result<SecretMetadata>;
}

/// Vault-only assembly, also usable by hosts without an SSH listener. The
/// product crypto service wraps it to publish live SSH grants after creation.
pub struct VaultSecretImporter {
    vault: Arc<PolarVault>,
    authority: VaultAuthority,
    caps: ProtectionCapabilities,
    device: Option<Arc<dyn Ed25519SecretExecutor>>,
}
impl VaultSecretImporter {
    pub fn new(
        vault: Arc<PolarVault>,
        authority: VaultAuthority,
        caps: ProtectionCapabilities,
        device: Option<Arc<dyn Ed25519SecretExecutor>>,
    ) -> Self {
        Self {
            vault,
            authority,
            caps,
            device,
        }
    }
}
#[async_trait]
impl SecretImportPort for VaultSecretImporter {
    async fn admit(&self, metadata: &SecretImportMetadata) -> Result<()> {
        metadata.validate(self.caps)?;
        if metadata.device_only {
            ensure!(
                self.device
                    .as_ref()
                    .is_some_and(|d| d.compute_location() == SecretCompute::Device),
                "device input unavailable"
            );
        }
        Ok(())
    }
    async fn import(
        &self,
        metadata: SecretImportMetadata,
        input: SecretInput,
    ) -> Result<SecretMetadata> {
        self.admit(&metadata).await?;
        self.vault
            .import_private_credential(
                &self.authority,
                metadata,
                input,
                self.caps,
                self.device.as_deref(),
            )
            .await
    }
}

impl PolarVault {
    async fn import_private_credential(
        &self,
        authority: &VaultAuthority,
        metadata: SecretImportMetadata,
        input: SecretInput,
        caps: ProtectionCapabilities,
        device: Option<&dyn Ed25519SecretExecutor>,
    ) -> Result<SecretMetadata> {
        let device = if metadata.device_only {
            Some(device.ok_or_else(|| anyhow::anyhow!("device input unavailable"))?)
        } else {
            None
        };
        let id = if let Some(ssh) = &metadata.ssh {
            let bytes = Zeroizing::new(
                <[u8; 32]>::try_from(input.0.as_slice())
                    .map_err(|_| anyhow::anyhow!("SSH creation requires a 32-byte seed"))?,
            );
            let public = IdentityKeypair::from_seed(IdentitySeed::from_bytes(*bytes)).node_id();
            drop(bytes);
            ensure!(
                public != authority.owner,
                "Polar root cannot be an SSH leaf"
            );
            let directory = self.managed_keys(authority, caps).await?;
            if let Some(id) = self
                .store
                .find_uuid_import(authority.owner, metadata.import_id)
                .await?
            {
                self.check_import(authority, id, &metadata.name, metadata.kind, &input, device)
                    .await?;
                let expected = ssh.record(authority.owner, id, public);
                directory
                    .recover_created_ssh(&expected, ssh.expected_revision)
                    .await?;
                let view = directory.view().await?;
                ensure!(
                    view.keys.iter().any(|k| k.record == expected),
                    "SSH creation retry conflicts with current configuration"
                );
                id
            } else {
                let record = self
                    .prepare_import_record(authority, &metadata.name, metadata.kind, &input, device)
                    .await?;
                let id = record.metadata.id;
                directory
                    .create_ssh_leaf(
                        ssh.record(authority.owner, id, public),
                        record,
                        metadata.import_id,
                        ssh.expected_revision,
                    )
                    .await?;
                id
            }
        } else {
            self.import_secret_as(
                authority,
                metadata.import_id,
                &metadata.name,
                metadata.kind,
                input,
                device,
            )
            .await?
        };
        self.inspect_secret(authority, id).await
    }
}
