//! Fixed CA authorization in the existing authenticated directory cache.
use super::*;
use crate::{policy::SecretOperation, SecretKind, StoredSecret};

impl KeyDirectory {
    /// Initializers must check configured execution capability before creating
    /// a hardware object. This does not attest that a selected device is online.
    pub fn validate_ca_protection(&self, protection: SecretProtection) -> Result<()> {
        protection
            .validate(self.caps)
            .map_err(|_| anyhow::anyhow!("CA protection unavailable"))
    }
    pub(crate) fn capabilities(&self) -> ProtectionCapabilities {
        self.caps
    }

    pub async fn ca_record(&self, id: SecretId) -> Result<KeyRecord> {
        let state = self.state.lock().await;
        let key = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?
            .directory
            .keys
            .iter()
            .find(|key| key.id == id)
            .ok_or_else(|| anyhow::anyhow!("CA unavailable"))?;
        ensure!(
            key.owner == self.owner
                && key.purpose == KeyPurpose::TransportCa
                && key.provider.is_none(),
            "CA unavailable"
        );
        key.policy.require(SecretOperation::Use)?;
        Ok(key.clone())
    }
    pub async fn authorize_ca(&self, expected: &KeyRecord) -> Result<()> {
        ensure!(
            self.ca_record(expected.id).await? == *expected,
            "CA configuration changed"
        );
        Ok(())
    }
    pub(crate) async fn create_ca(
        &self,
        key: KeyRecord,
        secret: StoredSecret,
        import: Uuid,
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
        ensure!(
            key.purpose == KeyPurpose::TransportCa
                && key.provider.is_none()
                && secret.metadata.id == key.id
                && secret.metadata.owner == self.owner
                && secret.metadata.kind == SecretKind::CertificateAuthority,
            "invalid CA material"
        );
        let mut next = old.directory.clone();
        next.keys.push(key);
        next.validate(self.owner, self.caps)?;
        self.commit_with_secret(&mut state, next, Some((secret, import)))
            .await
    }
    pub async fn revoke_ca(&self, id: SecretId, expected_revision: u64) -> Result<()> {
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(
            old.revision == expected_revision,
            "directory revision changed"
        );
        let mut next = old.directory.clone();
        let key = next
            .keys
            .iter_mut()
            .find(|k| k.id == id)
            .ok_or_else(|| anyhow::anyhow!("CA unavailable"))?;
        ensure!(
            key.purpose == KeyPurpose::TransportCa && key.owner == self.owner,
            "not a CA"
        );
        key.policy.require(SecretOperation::Revoke)?;
        key.policy.use_secret = crate::policy::OperationRule::Deny;
        self.commit(&mut state, next).await
    }
}
