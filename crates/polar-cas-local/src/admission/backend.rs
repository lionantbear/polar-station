//! Fixed-mode native admission behind the existing Catalog copy transaction.
use super::{AdmissionReceipt, FileMode, FileOwner, Preparation};
use crate::LocalCas;
use async_trait::async_trait;
use polar_cas::{
    ports::{ReplicaCommit, ReplicaEvidenceKind, ReplicaWriter, VerifiedStaging},
    source::SourceLocator,
    ContentRef, Error, ObjectId, ObjectKeyLayout, Result,
};

/// Strict same-filesystem admission into existing immutable native variants.
///
/// The caller owns the root and supplies closed, synchronized, immutable
/// verified staging. Include root, mode and owner in the Catalog location's
/// storage identity. This adapter never changes permissions, copies bytes, or
/// hashes during commit. Explicit inspect/recovery does hash the full file.
#[derive(Clone)]
pub struct LocalAdmissionBackend {
    store: LocalCas,
    mode: FileMode,
    owner: FileOwner,
}

impl LocalAdmissionBackend {
    pub fn new(store: LocalCas, mode: FileMode, owner: FileOwner) -> Result<Self> {
        if !matches!(store.key_layout, ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        Ok(Self { store, mode, owner })
    }

    fn located_id(&self, locator: &SourceLocator) -> Result<ObjectId> {
        let (id, mode) = self.store.native_variant(locator)?;
        if mode != self.mode {
            return Err(Error::Conflict);
        }
        Ok(id)
    }
}

fn completion(
    admitted: AdmissionReceipt,
    locator: SourceLocator,
    evidence_kind: ReplicaEvidenceKind,
) -> ReplicaCommit {
    ReplicaCommit {
        content: admitted.file.content().clone(),
        locator,
        evidence_kind,
        evidence: [
            ("completion".into(), "file-and-parent-fsync".into()),
            ("read_bytes".into(), admitted.read_bytes.to_string()),
            ("copied_bytes".into(), admitted.copied_bytes.to_string()),
        ]
        .into(),
        reused: admitted.preparation == Preparation::Reused,
    }
}

#[async_trait]
impl ReplicaWriter for LocalAdmissionBackend {
    fn locator(&self, content: &ContentRef) -> Result<SourceLocator> {
        Ok(SourceLocator {
            key: self
                .store
                .variant_key(&content.id, self.mode)?
                .to_str()
                .ok_or(Error::Integrity)?
                .into(),
            revision: None,
        })
    }

    async fn commit(
        &self,
        content: &ContentRef,
        staging: VerifiedStaging,
    ) -> Result<ReplicaCommit> {
        if staging.content().reference(&content.id)? != *content {
            return Err(Error::Integrity);
        }
        let locator = self.locator(content)?;
        let content = content.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            // The worker owns staging, including its execution guard, until
            // physical completion even if the async caller is cancelled.
            let admitted = backend.store.admit_path(
                staging.path(),
                &content.id,
                staging.content().clone(),
                backend.mode,
                backend.owner,
            )?;
            Ok(completion(
                admitted,
                locator,
                ReplicaEvidenceKind::TrustedLocal,
            ))
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }

    async fn inspect(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        let variant_id = self.located_id(locator)?;
        let backend = self.clone();
        let content = content.clone();
        let locator = locator.clone();
        tokio::task::spawn_blocking(move || {
            Ok(backend
                .store
                .inspect_variant_at(&content, &variant_id, backend.mode, backend.owner)?
                .map(|admitted| completion(admitted, locator, ReplicaEvidenceKind::VerifiedRead)))
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }

    async fn open_managed(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<polar_cas::ports::ManagedReplica>> {
        self.located_id(locator)?;
        self.store
            .open_managed_at(content, locator, Some((self.mode, self.owner)))
            .await
    }
}
