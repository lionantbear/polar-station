//! Fixed identity references in the existing authenticated key directory.
use super::{KeyAlgorithm, KeyDirectory, KeyPurpose, KeyRecord};
use crate::{policy::SecretOperation, SecretId};
use anyhow::{ensure, Result};

impl KeyDirectory {
    pub async fn identity_reference(&self, id: SecretId) -> Result<KeyRecord> {
        let state = self.state.lock().await;
        let snapshot = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let key = snapshot
            .directory
            .keys
            .iter()
            .find(|k| k.id == id)
            .ok_or_else(|| anyhow::anyhow!("identity reference unavailable"))?;
        ensure!(
            key.owner == self.owner
                && key.purpose == KeyPurpose::PolarRoot
                && key.algorithm == KeyAlgorithm::Ed25519
                && key.provider.is_some(),
            "identity reference unavailable"
        );
        key.policy.require(SecretOperation::Use)?;
        Ok(key.clone())
    }
    pub async fn authorize_identity_reference(&self, expected: &KeyRecord) -> Result<()> {
        let state = self.state.lock().await;
        let snapshot = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        let current = snapshot
            .directory
            .keys
            .iter()
            .find(|key| key.id == expected.id)
            .ok_or_else(|| anyhow::anyhow!("identity reference unavailable"))?;
        ensure!(
            current.owner == self.owner
                && current.provider.is_some()
                && current.purpose == KeyPurpose::PolarRoot
                && current == expected,
            "identity reference changed"
        );
        current.policy.require(SecretOperation::Use)?;
        Ok(())
    }
    /// Explicit management operation; identities/references cannot be replaced.
    pub async fn revoke_identity_reference(
        &self,
        id: SecretId,
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
        let mut next = old.directory.clone();
        let key = next
            .keys
            .iter_mut()
            .find(|k| k.id == id)
            .ok_or_else(|| anyhow::anyhow!("identity reference unavailable"))?;
        ensure!(
            key.provider.is_some() && key.purpose == KeyPurpose::PolarRoot,
            "identity reference unavailable"
        );
        key.policy.require(SecretOperation::Revoke)?;
        key.policy.use_secret = crate::policy::OperationRule::Deny;
        self.commit(&mut state, next).await
    }
}

impl KeyDirectory {
    /// Dedicated root material or a reference, using the same authenticated
    /// directory cache. No database read occurs on operation admission.
    pub async fn authorize_identity(&self, expected: &KeyRecord) -> Result<()> {
        let current = self.identity_record(expected.id).await?;
        ensure!(current == *expected, "identity configuration changed");
        Ok(())
    }
    pub async fn identity_record(&self, id: SecretId) -> Result<KeyRecord> {
        let state = self.state.lock().await;
        let current = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?
            .directory
            .keys
            .iter()
            .find(|k| k.id == id)
            .ok_or_else(|| anyhow::anyhow!("identity unavailable"))?;
        ensure!(
            current.owner == self.owner
                && current.purpose == KeyPurpose::PolarRoot
                && current.algorithm == KeyAlgorithm::Ed25519,
            "identity unavailable"
        );
        current.policy.require(SecretOperation::Use)?;
        Ok(current.clone())
    }
    pub(crate) async fn create_polar_identity(
        &self,
        key: KeyRecord,
        secret: crate::StoredSecret,
        import: uuid::Uuid,
        expected: u64,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(old.revision == expected, "directory revision changed");
        ensure!(
            key.provider.is_none()
                && key.purpose == KeyPurpose::PolarRoot
                && secret.metadata.id == key.id
                && secret.metadata.owner == self.owner
                && secret.metadata.kind == crate::SecretKind::PolarIdentity,
            "invalid identity material"
        );
        let mut next = old.directory.clone();
        next.keys.push(key);
        next.validate(self.owner, self.caps)?;
        self.commit_with_secret(&mut state, next, Some((secret, import)))
            .await
    }
    /// Revoke use of a persisted fixed identity without rotating its NodeId.
    pub async fn revoke_polar_identity(&self, id: SecretId, expected_revision: u64) -> Result<()> {
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
            .ok_or_else(|| anyhow::anyhow!("identity unavailable"))?;
        ensure!(
            key.purpose == KeyPurpose::PolarRoot && key.provider.is_none(),
            "not a stored identity"
        );
        key.policy.require(SecretOperation::Revoke)?;
        key.policy.use_secret = crate::policy::OperationRule::Deny;
        self.commit(&mut state, next).await
    }
}
