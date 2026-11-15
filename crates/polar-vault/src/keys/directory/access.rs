//! Nonsecret but integrity-sensitive access grants. Only management writes;
//! normal signing checks the authenticated RAM snapshot and performs no IO.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAccessBinding {
    pub instance: Uuid,
    pub identity: SecretId,
    pub certificate_sha256: [u8; 32],
    pub revoked: bool,
}
pub(super) fn validate(directory: &RecoveryDirectory) -> Result<()> {
    ensure!(
        directory.provider_access.len() <= 128,
        "access directory capacity exceeded"
    );
    for (index, grant) in directory.provider_access.iter().enumerate() {
        ensure!(
            !grant.instance.is_nil() && grant.certificate_sha256 != [0; 32],
            "invalid access binding"
        );
        ensure!(
            !directory.provider_access[..index]
                .iter()
                .any(|other| other.certificate_sha256 == grant.certificate_sha256),
            "certificate already bound"
        );
        ensure!(
            directory
                .keys
                .iter()
                .any(|key| key.id == grant.identity && key.purpose == KeyPurpose::PolarRoot),
            "access binding identity unavailable"
        );
        ensure!(
            !directory
                .provider_access
                .iter()
                .any(|other| other.instance == grant.instance && other.identity != grant.identity),
            "instance identity is fixed"
        );
    }
    Ok(())
}
impl KeyDirectory {
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub async fn authorize_provider_access(&self, expected: &ProviderAccessBinding) -> Result<()> {
        let state = self.state.lock().await;
        let directory = &state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?
            .directory;
        ensure!(
            !expected.revoked
                && directory
                    .provider_access
                    .iter()
                    .any(|grant| grant == expected && !grant.revoked),
            "instance access unavailable"
        );
        let key = directory
            .keys
            .iter()
            .find(|key| key.id == expected.identity && key.purpose == KeyPurpose::PolarRoot)
            .ok_or_else(|| anyhow::anyhow!("identity unavailable"))?;
        key.policy.require(crate::policy::SecretOperation::Use)
    }
    /// Trusted management only. None enrolls a new credential; Some(old hash)
    /// atomically retires the old credential belonging to this same instance.
    pub async fn enroll_provider_access(
        &self,
        grant: ProviderAccessBinding,
        replace: Option<[u8; 32]>,
        expected_revision: u64,
    ) -> Result<()> {
        ensure!(!grant.revoked, "cannot enroll revoked access");
        let mut state = self.state.lock().await;
        let old = state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("directory requires reload"))?;
        ensure!(
            old.revision == expected_revision,
            "directory revision changed"
        );
        let mut next = old.directory.clone();
        if let Some(existing) = next
            .provider_access
            .iter()
            .find(|item| item.certificate_sha256 == grant.certificate_sha256)
        {
            ensure!(
                existing == &grant,
                "certificate cannot be rebound or revived"
            );
            if let Some(hash) = replace {
                ensure!(
                    next.provider_access
                        .iter()
                        .any(|old| old.certificate_sha256 == hash
                            && old.revoked
                            && old.instance == grant.instance
                            && old.identity == grant.identity),
                    "rotation retry does not match committed access"
                );
            }
            return Ok(());
        }
        ensure!(
            !next
                .provider_access
                .iter()
                .any(|item| item.instance == grant.instance && !item.revoked)
                || replace.is_some(),
            "credential rotation must retire current access"
        );
        if let Some(hash) = replace {
            let previous = next
                .provider_access
                .iter_mut()
                .find(|item| item.certificate_sha256 == hash)
                .ok_or_else(|| anyhow::anyhow!("old credential unavailable"))?;
            ensure!(
                !previous.revoked
                    && previous.instance == grant.instance
                    && previous.identity == grant.identity,
                "rotation identity mismatch"
            );
            previous.revoked = true;
        }
        next.provider_access.push(grant);
        next.validate(self.owner, self.caps)?;
        self.commit(&mut state, next).await
    }
    pub async fn revoke_provider_access(
        &self,
        hash: [u8; 32],
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
        let grant = next
            .provider_access
            .iter_mut()
            .find(|grant| grant.certificate_sha256 == hash)
            .ok_or_else(|| anyhow::anyhow!("access unavailable"))?;
        if grant.revoked {
            return Ok(());
        }
        grant.revoked = true;
        self.commit(&mut state, next).await
    }
}
