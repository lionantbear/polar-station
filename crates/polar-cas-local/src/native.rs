use crate::{map_missing, LocalCas};
use async_trait::async_trait;
use cap_std::{ambient_authority, fs::Dir};
use polar_cas::{
    ports::{
        ArtifactPublisher, ReplicaCommit, ReplicaEvidenceKind, ReplicaWriter, SourceReader,
        VerifiedStaging,
    },
    source::{OpaqueAccessRef, SourceLocator},
    verification::ContentVerifier,
    ContentRef, Error, ObjectId, ObjectKeyLayout, Reader, Result,
};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Read-only source binding; opening it does not register a replica or confer
/// ownership of these files. cap-std confines traversal to the injected root.
#[derive(Clone)]
pub struct LocalSource {
    pub(crate) dir: Arc<Dir>,
}
impl LocalSource {
    /// Explicit Host directory capability; does not register a source or imply
    /// immutability. Used to observe a kernel-paused merged workspace root.
    #[cfg(feature = "tree-ingest")]
    pub fn from_directory_capability(file: std::fs::File) -> Result<Self> {
        if !file.metadata()?.is_dir() {
            return Err(Error::Invalid("directory capability required".into()));
        }
        Ok(Self {
            dir: Arc::new(Dir::from_std_file(file)),
        })
    }
    /// No empty-path locator loophole: root observation is a distinct API.
    #[cfg(all(feature = "tree-ingest", target_os = "linux"))]
    pub fn observe_root(
        &self,
        limits: crate::observe::ObservationLimits,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<crate::observe::ObservedLocalTree> {
        crate::observe::ObservedLocalTree::open_confined(&self.dir, "", limits, cancel)
    }

    /// Prepare exactly one declared regular file without scanning its siblings.
    #[cfg(all(feature = "tree-ingest", target_os = "linux"))]
    pub fn observe_file(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
    ) -> Result<crate::observe::ObservedLocalFile> {
        validate(locator, access)?;
        crate::observe::ObservedLocalFile::open(&self.dir, &locator.key)
    }
    /// Observe a confined input scope once. Does not publish Source/Tree/Pack
    /// records; use on a blocking worker and revalidate before publication.
    #[cfg(all(feature = "tree-ingest", target_os = "linux"))]
    pub fn observe_tree(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
        limits: crate::observe::ObservationLimits,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<crate::observe::ObservedLocalTree> {
        validate(locator, access)?;
        crate::observe::ObservedLocalTree::open_confined(&self.dir, &locator.key, limits, cancelled)
    }
    /// Explicit sealing/admission preparation, never called by projection.
    /// An unsupported filesystem or missing host privilege fails closed.
    #[cfg(all(feature = "tree-ingest", target_os = "linux"))]
    pub fn lock_tree(
        &self,
        locator: &SourceLocator,
        limits: crate::observe::ObservationLimits,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        policy: crate::locked_tree::DirectorySealPolicy,
    ) -> std::result::Result<
        crate::locked_tree::LockedDirectory,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        validate(locator, None)?;
        let observed = crate::observe::ObservedLocalTree::open_confined(
            &self.dir,
            &locator.key,
            limits,
            cancel,
        )?;
        Ok(crate::locked_tree::LockedDirectory::from_observation(
            observed, policy,
        )?)
    }
    #[cfg(all(feature = "tree-ingest", target_os = "linux"))]
    pub fn matches_locked_tree(
        &self,
        locked: &crate::locked_tree::LockedDirectory,
        locator: &SourceLocator,
    ) -> Result<()> {
        validate(locator, None)?;
        if !locked.bound_to(self, &locator.key) {
            return Err(Error::Conflict);
        }
        locked.revalidate()
    }
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        if !root.as_ref().is_absolute() {
            return Err(Error::Invalid(
                "explicit absolute source root required".into(),
            ));
        }
        Ok(Self {
            dir: Arc::new(Dir::open_ambient_dir(root, ambient_authority())?),
        })
    }
}

fn validate(locator: &SourceLocator, access: Option<&OpaqueAccessRef>) -> Result<()> {
    locator.validate()?;
    if access.is_some() || locator.revision.is_some() {
        return Err(Error::UnsupportedCapability);
    }
    if !Path::new(&locator.key)
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(Error::Invalid(
            "local source requires a confined relative key".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl SourceReader for LocalSource {
    async fn open(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
    ) -> Result<Reader> {
        validate(locator, access)?;
        let dir = self.dir.clone();
        let key = locator.key.clone();
        let file = tokio::task::spawn_blocking(move || {
            let file = dir.open(key).map_err(map_missing)?;
            if !file.metadata()?.is_file() {
                return Err(Error::Invalid("source is not a regular file".into()));
            }
            Ok::<_, Error>(file.into_std())
        })
        .await
        .map_err(|_| Error::Backend("local source worker failed".into()))??;
        Ok(Box::pin(tokio::fs::File::from_std(file)))
    }
}

#[async_trait]
impl ReplicaWriter for LocalCas {
    fn read_observer(&self) -> Option<Arc<dyn polar_cas::ports::ReplicaObserver>> {
        Some(Arc::new(self.clone()))
    }

    fn trusted_shared_root(&self) -> Option<polar_cas::shared::TrustedSharedRoot> {
        matches!(self.key_layout, ObjectKeyLayout::SharedContent).then(|| {
            polar_cas::shared::TrustedSharedRoot::from_trusted_adapter(self.root.clone())
                .expect("canonical absolute root")
        })
    }
    fn locator(&self, content: &ContentRef) -> Result<SourceLocator> {
        if !matches!(self.key_layout, ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        Ok(SourceLocator {
            key: content.id.content_key(),
            revision: None,
        })
    }

    async fn reuse(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        validate(locator, None)?;
        let located_id = ObjectId::from_content_key(&locator.key)?;
        if self.locator(&ContentRef {
            id: located_id,
            size_bytes: content.size_bytes,
        })? != *locator
        {
            return Err(Error::Conflict);
        }
        #[cfg(unix)]
        {
            let Some(_lease) = self.open_managed_at(content, locator, None).await? else {
                return Ok(None);
            };
            return Ok(Some(ReplicaCommit {
                content: polar_cas::verification::VerifiedContent::from_trusted_admission(
                    vec![content.id.clone()],
                    content.size_bytes,
                )?,
                locator: locator.clone(),
                evidence: Default::default(),
                evidence_kind: ReplicaEvidenceKind::TrustedLocal,
                reused: true,
            }));
        }
        #[cfg(not(unix))]
        self.inspect(content, locator).await
    }

    async fn commit(
        &self,
        content: &ContentRef,
        staging: VerifiedStaging,
    ) -> Result<ReplicaCommit> {
        self.commit_file(content, &self.locator(content)?, staging)
            .await
    }

    async fn inspect(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        validate(locator, None)?;
        let located_id = ObjectId::from_content_key(&locator.key)?;
        if self.locator(&ContentRef {
            id: located_id.clone(),
            size_bytes: content.size_bytes,
        })? != *locator
        {
            return Err(Error::Conflict);
        }
        let found = self.inspect_file(content, locator).await?;
        if found
            .as_ref()
            .is_some_and(|commit| !commit.content.ids().contains(&located_id))
        {
            return Err(Error::Integrity);
        }
        Ok(found)
    }

    #[cfg(unix)]
    async fn open_managed(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<polar_cas::ports::ManagedReplica>> {
        validate_artifact(locator)?;
        let located_id = ObjectId::from_content_key(&locator.key)?;
        if self.locator(&ContentRef {
            id: located_id,
            size_bytes: content.size_bytes,
        })? != *locator
        {
            return Err(Error::Conflict);
        }
        // A different digest alias is allowed only through the caller's exact
        // Ready Catalog association. No additional digest is inferred here.
        self.open_managed_at(content, locator, None).await
    }
}

impl LocalCas {
    async fn commit_file(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
        staging: VerifiedStaging,
    ) -> Result<ReplicaCommit> {
        if staging.content().reference(&content.id)? != *content {
            return Err(Error::Integrity);
        }
        let store = self.clone();
        let content = content.clone();
        let locator = locator.clone();
        tokio::task::spawn_blocking(move || {
            let reused = store.publish_file(&content, &locator.key, staging.path())?;
            Ok(ReplicaCommit {
                content: staging.content().clone(),
                evidence_kind: if reused
                    && matches!(store.key_layout, ObjectKeyLayout::SharedContent)
                {
                    ReplicaEvidenceKind::TrustedLocal
                } else if reused {
                    ReplicaEvidenceKind::VerifiedRead
                } else {
                    ReplicaEvidenceKind::VerifiedWrite
                },
                locator,
                evidence: [("completion".into(), "file-and-parent-fsync".into())].into(),
                reused,
            })
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }

    async fn inspect_file(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        self.inspect_file_with_durability(content, locator, true)
            .await
    }

    async fn inspect_file_with_durability(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
        durable: bool,
    ) -> Result<Option<ReplicaCommit>> {
        let store = self.clone();
        let content = content.clone();
        let locator = locator.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = match store.dir.open(&locator.key) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !file.metadata()?.is_file() || file.metadata()?.len() != content.size_bytes {
                return Err(Error::Integrity);
            }
            let mut verifier = ContentVerifier::default();
            let mut read = 0_u64;
            let mut bytes = [0_u8; 64 * 1024];
            loop {
                let count = file.read(&mut bytes)?;
                if count == 0 {
                    break;
                }
                read = read.checked_add(count as u64).ok_or(Error::TooLarge)?;
                if read > content.size_bytes {
                    return Err(Error::Integrity);
                }
                verifier.update(&bytes[..count])?;
            }
            let verified = verifier.finish();
            if !verified.ids().contains(&content.id) || verified.size_bytes() != content.size_bytes
            {
                return Err(Error::Integrity);
            }
            // Recovery may race a previously interrupted commit after the
            // final link was created but before directory fsync finished.
            if durable {
                file.sync_all()?;
                store.sync_parents(&locator.key)?;
            }
            Ok(Some(ReplicaCommit {
                content: verified,
                evidence_kind: ReplicaEvidenceKind::VerifiedRead,
                locator,
                evidence: [("verification".into(), "full-hash".into())].into(),
                reused: true,
            }))
        })
        .await
        .map_err(|_| Error::Backend("local verification worker failed".into()))?
    }
}

impl LocalCas {
    /// Explicit multi-algorithm shared-content binding. Existing namespace
    /// stores retain their old layout and are never auto-probed or migrated.
    pub fn open_shared(root: PathBuf) -> Result<Self> {
        Self::open_with_layout(root, ObjectKeyLayout::SharedContent)
    }
}

#[async_trait]
impl ArtifactPublisher for LocalCas {
    async fn publish_artifact(
        &self,
        content: &ContentRef,
        target: &SourceLocator,
        staging: VerifiedStaging,
    ) -> Result<ReplicaCommit> {
        validate_artifact(target)?;
        if staging.content().reference(&content.id)? != *content {
            return Err(Error::Integrity);
        }
        self.commit_file(content, target, staging)
            .await
            .map_err(|error| {
                // A named immutable publication cannot supersede different bytes.
                // Input evidence was checked separately above.
                if matches!(error, Error::Integrity) {
                    Error::Conflict
                } else {
                    error
                }
            })
    }
    async fn inspect_artifact(
        &self,
        content: &ContentRef,
        target: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        validate_artifact(target)?;
        self.inspect_file(content, target).await
    }
}

pub(crate) fn validate_artifact(locator: &SourceLocator) -> Result<()> {
    validate(locator, None)?;
    if locator.key.contains('\\')
        || locator
            .key
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Error::Invalid(
            "artifact requires an unambiguous relative key".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl polar_cas::ports::ReplicaObserver for LocalCas {
    fn canonical_locator(&self, content: &ContentRef) -> Result<SourceLocator> {
        ReplicaWriter::locator(self, content)
    }
    async fn present(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
        admitted: bool,
    ) -> Result<bool> {
        validate_artifact(locator)?;
        if admitted {
            return match ReplicaWriter::open_managed(self, content, locator).await? {
                Some(file) => {
                    file.revalidate()?;
                    Ok(true)
                }
                None => Ok(false),
            };
        }
        // Unregistered/stale candidates never inherit Ready admission trust.
        Ok(self
            .inspect_file_with_durability(content, locator, false)
            .await?
            .is_some())
    }
}
