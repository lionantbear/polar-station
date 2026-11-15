//! Read-only destination inspection. Shares the publisher's complete namespace
//! validator but never runs prepare(), mkdir, fsync, chmod or publication.
use super::{filesystem::Target, LocalTreePublisher, LocalTreeRequest};
use crate::{admission::{FileMode, LocalFile}, LocalCas};
use async_trait::async_trait;
use cap_std::{ambient_authority, fs::Dir};
use polar_cas::{
    ports::{ProjectionCommit, ProjectionObserver, ProjectionTargetIo},
    projection::{export::ProjectionPreparation, FileMaterialization, FilePlacementMethod},
    Error, Result,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct LocalTreeObserver {
    publisher: LocalTreePublisher,
    /// Only strict hardlink identity needs this read-only source witness.
    store: Option<LocalCas>,
    trusted: Option<BTreeMap<(String, bool), LocalFile>>,
}
impl LocalTreeObserver {
    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() || root.canonicalize()? != root {
            return Err(Error::Invalid("existing canonical projection root required".into()));
        }
        let dir = Arc::new(Dir::open_ambient_dir(&root, ambient_authority())?);
        Ok(Self::from_publisher(LocalTreePublisher { root, dir }))
    }
    pub(super) fn from_publisher(publisher: LocalTreePublisher) -> Self {
        Self { publisher, store: None, trusted: None }
    }
    pub(super) fn with_sources(mut self, store: LocalCas,
        trusted: Option<BTreeMap<(String, bool), LocalFile>>) -> Self {
        self.store = Some(store);
        self.trusted = trusted;
        self
    }
    pub(super) fn run(&self, input: ProjectionTargetIo, durable: bool) -> Result<Option<ProjectionCommit>> {
        input.validate()?;
        crate::native::validate_artifact(&input.target)?;
        // Only the shared inspection walk consumes this private request. It
        // cannot be used for publication, which validates admitted file proofs.
        let request = LocalTreeRequest { tree: input.tree, target: input.target,
            profile: input.profile, files: BTreeMap::new(), _owner: Arc::new(()) };
        let target = match Target::open(&self.publisher, &request, false) {
            Ok(v) => v,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut read_bytes = 0u64;
        let mut sources = BTreeMap::<(String, bool), LocalFile>::new();
        let published = self.publisher.inspect_with(&request, &target, durable,
            |path, content, executable, method| {
                let mode = if executable { FileMode::Executable } else { FileMode::ReadOnly };
                if request.profile.files == FileMaterialization::HardlinkRequired {
                    if method != FilePlacementMethod::Hardlink { return Err(Error::Integrity); }
                    let key = (content.id.to_string(), executable);
                    if !sources.contains_key(&key) {
                        let proof = self.trusted.as_ref().and_then(|t| t.get(&key)).cloned();
                        let proof = match proof {
                            Some(proof) => proof,
                            None => {
                                let store = self.store.as_ref().ok_or(Error::AccessMaterialUnavailable)?;
                                let proof = store.observe_existing_variant(content, mode, request.profile.owner)?
                                    .ok_or(Error::MissingLocal)?;
                                read_bytes = read_bytes.checked_add(content.size_bytes).ok_or(Error::TooLarge)?;
                                proof
                            }
                        };
                        sources.insert(key.clone(), proof);
                    }
                    let proof = sources.get(&key).ok_or(Error::Integrity)?;
                    proof.validate_for(content, mode, request.profile.owner)?;
                    proof.verify_link(path)?;
                    if durable { proof.sync()?; }
                } else {
                    // CopyAllowed does not require the historical realization
                    // method to remain an inode relation. Verify current bytes.
                    LocalFile::inspect_target_bytes(path, content, mode, request.profile.owner, durable)?;
                    read_bytes = read_bytes.checked_add(content.size_bytes).ok_or(Error::TooLarge)?;
                }
                Ok(())
            })?;
        Ok(published.map(|p| ProjectionCommit { target: request.target, root: p.root,
            record: p.record, preparation: ProjectionPreparation { read_bytes, ..Default::default() },
            reused: true }))
    }
}
#[async_trait]
impl ProjectionObserver for LocalTreeObserver {
    async fn observe_projection(&self, request: ProjectionTargetIo) -> Result<Option<ProjectionCommit>> {
        let observer = self.clone();
        tokio::task::spawn_blocking(move || observer.run(request, false)).await
            .map_err(|_| Error::Backend("target observation worker failed".into()))?
    }
}
