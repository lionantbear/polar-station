//! CA material never becomes an ordinary credential lease. Only a trusted
//! in-process backend adapter may borrow plaintext to open a fixed-key handle.
use super::*;
use crate::keys::{directory::KeyDirectory, KeyRecord};
use std::sync::Arc;

pub trait CaMaterialOpener {
    type Opened;
    fn open(&self, material: &[u8], expected: &KeyRecord) -> Result<Self::Opened>;
}
impl PolarVault {
    pub async fn require_ca_import(
        &self,
        authority: &VaultAuthority,
        id: SecretId,
        import: Uuid,
    ) -> Result<()> {
        ensure!(
            !import.is_nil()
                && self.store.find_uuid_import(authority.owner, import).await? == Some(id),
            "CA initialization request changed"
        );
        Ok(())
    }
    async fn check_ca_directory(
        &self,
        authority: &VaultAuthority,
        directory: &Arc<KeyDirectory>,
    ) -> Result<()> {
        let own = self
            .managed_keys(authority, directory.capabilities())
            .await?;
        ensure!(Arc::ptr_eq(&own, directory), "foreign CA directory");
        Ok(())
    }
    /// Explicit initializer; directory and encrypted UUID object commit in the
    /// existing adapter transaction. Backend validates material before calling.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_ca_material(
        &self,
        authority: &VaultAuthority,
        directory: &Arc<KeyDirectory>,
        key: KeyRecord,
        import: Uuid,
        input: SecretInput,
        expected_revision: u64,
    ) -> Result<SecretId> {
        self.check_ca_directory(authority, directory).await?;
        ensure!(!import.is_nil(), "invalid CA import UUID");
        key.validate(authority.owner, directory.capabilities())?;
        ensure!(
            key.purpose == crate::keys::KeyPurpose::TransportCa,
            "not a CA"
        );
        if let Some(id) = self.store.find_uuid_import(authority.owner, import).await? {
            ensure!(id == key.id, "CA import conflict");
            let (old, bytes) = self.ca_material(authority, directory, id).await?;
            ensure!(
                old == key && bytes.as_slice() == input.0.as_slice(),
                "CA import conflict"
            );
            return Ok(id);
        }
        let stored = self
            .prepare_import_record_with_id(
                authority,
                key.id,
                &key.scope,
                SecretKind::CertificateAuthority,
                &input,
                None,
            )
            .await?;
        let id = key.id;
        directory
            .create_ca(key, stored, import, expected_revision)
            .await?;
        Ok(id)
    }
    async fn ca_material(
        &self,
        authority: &VaultAuthority,
        directory: &Arc<KeyDirectory>,
        id: SecretId,
    ) -> Result<(KeyRecord, Zeroizing<Vec<u8>>)> {
        self.check_ca_directory(authority, directory).await?;
        let key = directory.ca_record(id).await?;
        let record = self.authorized_secret(authority.owner, id).await?;
        ensure!(
            !record.metadata.revoked
                && !record.metadata.device_only
                && record.metadata.version == 1
                && record.metadata.kind == SecretKind::CertificateAuthority,
            "CA material unavailable"
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
        ensure!(
            !bytes.is_empty() && bytes.len() <= 65_536,
            "invalid CA material size"
        );
        directory.authorize_ca(&key).await?;
        Ok((key, bytes))
    }
    pub async fn open_ca_material<O: CaMaterialOpener>(
        &self,
        authority: &VaultAuthority,
        directory: &Arc<KeyDirectory>,
        id: SecretId,
        opener: &O,
    ) -> Result<O::Opened> {
        let (key, bytes) = self.ca_material(authority, directory, id).await?;
        let opened = opener.open(&bytes, &key)?;
        directory.authorize_ca(&key).await?;
        Ok(opened)
    }
}
