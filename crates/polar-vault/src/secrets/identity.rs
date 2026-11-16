//! Explicit software-identity initialization. This is not a prerequisite for
//! TPM, ESP or SE051 backends. Root plaintext has no ordinary consumer lease.
use super::*;
use crate::{
    keys::*,
    policy::{OperationRule, SecretPolicy},
    protection::*,
};
use polar_identity::IdentityKeypair;

/// Trusted IN-PROCESS backend adapter, not a wire or SDK port. Borrowed root
/// bytes exist only inside the provider. Return an owner whose Drop closes it.
/// This is an API ownership boundary, not isolation from malicious host code.
pub trait IdentityMaterialOpener {
    type Opened;
    fn open(&self, seed: &[u8; 32], expected: NodeId) -> Result<Self::Opened>;
}
impl PolarVault {
    /// Caller supplies an existing identity from the private initializer. This
    /// never generates or replaces a NodeId, nor claims recovery words backed up.
    /// The Vault is protected by the provider cipher; authority.owner is A.
    pub async fn import_software_identity(
        &self,
        authority: &VaultAuthority,
        import: Uuid,
        name: &str,
        input: SecretInput,
        expected_directory_revision: u64,
    ) -> Result<SecretId> {
        ensure!(!import.is_nil(), "invalid import UUID");
        validate_name(name)?;
        let seed: &[u8; 32] = input
            .0
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("identity requires 32 bytes"))?;
        ensure!(
            IdentityKeypair::from_seed_bytes(seed).node_id() == authority.owner,
            "identity seed mismatch"
        );
        let directory = self.software_identity_directory(authority).await?;
        if let Some(id) = self.store.find_uuid_import(authority.owner, import).await? {
            let (key, stored, bytes) = self.software_identity_material(authority, id, 1).await?;
            ensure!(
                stored.metadata.name == name
                    && key.scope == name
                    && bytes.as_slice() == input.0.as_slice(),
                "identity import conflict"
            );
            return Ok(id);
        }
        let stored = self
            .prepare_import_record(authority, name, SecretKind::PolarIdentity, &input, None)
            .await?;
        let id = stored.metadata.id;
        let key = KeyRecord {
            id,
            owner: authority.owner,
            purpose: KeyPurpose::PolarRoot,
            algorithm: KeyAlgorithm::Ed25519,
            management: KeyManagement::VaultStored,
            protection: SecretProtection {
                storage: SecretStorage::HostMemory,
                compute: SecretCompute::Host,
                retention: SecretRetention::OwnerLifetime,
                vault: Some(VaultLocation::Host),
            },
            policy: SecretPolicy {
                metadata: OperationRule::Allow,
                use_secret: OperationRule::Allow,
                ciphertext_export: OperationRule::Deny,
                plaintext_export: OperationRule::Deny,
                rotate: OperationRule::Deny,
                revoke: OperationRule::Allow,
                delete: OperationRule::Deny,
                policy_change: OperationRule::Deny,
            },
            other_copies: vec![],
            recovery: KeyRecovery::RootWords,
            public_key: authority.owner.as_bytes().to_vec(),
            scope: name.into(),
            hardware_object: None,
            provider: None,
        };
        directory
            .create_polar_identity(key, stored, import, expected_directory_revision)
            .await?;
        Ok(id)
    }
    async fn software_identity_material(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        version: u64,
    ) -> Result<(KeyRecord, StoredSecret, Zeroizing<Vec<u8>>)> {
        ensure!(version == 1, "fixed identity version mismatch");
        let directory = self.software_identity_directory(authority).await?;
        let key = directory.identity_record(id).await?;
        ensure!(
            key.provider.is_none()
                && key.protection.storage == SecretStorage::HostMemory
                && key.protection.compute == SecretCompute::Host
                && key.protection.retention == SecretRetention::OwnerLifetime,
            "not software identity material"
        );
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !record.metadata.revoked
                && !record.metadata.device_only
                && record.metadata.kind == SecretKind::PolarIdentity
                && record.metadata.version == version,
            "identity material unavailable"
        );
        let bytes = Zeroizing::new(
            self.cipher
                .open_bound(
                    &record.nonce,
                    &record.ciphertext,
                    &record.authenticated_context()?,
                )
                .await?,
        );
        ensure!(bytes.len() == 32, "invalid identity material");
        directory.authorize_identity(&key).await?;
        Ok((key, record, bytes))
    }
    pub async fn open_software_identity<O: IdentityMaterialOpener>(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        version: u64,
        opener: &O,
    ) -> Result<O::Opened> {
        let (key, _, bytes) = self
            .software_identity_material(authority, id, version)
            .await?;
        let seed: &[u8; 32] = bytes.as_slice().try_into()?;
        ensure!(
            IdentityKeypair::from_seed_bytes(seed).node_id() == authority.owner,
            "identity material mismatch"
        );
        let opened = opener.open(seed, authority.owner)?;
        // If revoked during opening, the unreturned owner's Drop closes it.
        let directory = self.software_identity_directory(authority).await?;
        directory.authorize_identity(&key).await?;
        Ok(opened)
    }
}
