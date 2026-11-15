use super::{filesystem::Target, LocalTreePublisher, LocalTreeRequest};
use crate::{
    admission::{AdmissionReceipt, LocalFile, Preparation},
    LocalCas,
};
use async_trait::async_trait;
use polar_cas::{
    ports::{LocalMaterializer, ProjectionCommit, ProjectionIo},
    projection::{export::ProjectionPreparation, FileMode},
    Error, ObjectId, Result,
};
use std::collections::BTreeMap;

/// Explicit pair of caller-owned roots; no Catalog discovery or network I/O.
#[derive(Clone)]
pub struct NativeMaterializer {
    store: LocalCas,
    publisher: LocalTreePublisher,
    trusted: Option<BTreeMap<(String, bool), LocalFile>>,
}
impl NativeMaterializer {
    pub fn new(store: LocalCas, publisher: LocalTreePublisher) -> Self {
        Self {
            store,
            publisher,
            trusted: None,
        }
    }
    /// Strict proof-only execution. Missing proof/digest never falls back to
    /// payload reads. These handles must not be reconstructed from JSON.
    pub fn with_trusted_files(mut self, files: Vec<LocalFile>) -> Result<Self> {
        if files
            .iter()
            .any(|f| !f.path().starts_with(self.store.root()))
        {
            return Err(Error::Conflict);
        }
        let mut indexed: BTreeMap<(String, bool), LocalFile> = BTreeMap::new();
        for file in files {
            for id in file.content().ids() {
                let key = (id.to_string(), file.mode() == FileMode::Executable);
                if indexed
                    .get(&key)
                    .is_some_and(|old| old.path() != file.path())
                {
                    return Err(Error::Conflict);
                }
                indexed.insert(key, file.clone());
            }
        }
        self.trusted = Some(indexed);
        Ok(self)
    }
    fn run(&self, input: &ProjectionIo, inspect: bool) -> Result<Option<ProjectionCommit>> {
        input.validate()?;
        super::filesystem::require_owner(input.profile.owner)?;
        crate::native::validate_artifact(&input.target)?;
        for source in &input.sources {
            crate::native::validate_artifact(&source.locator)?;
        }
        if inspect {
            match self.publisher.dir.symlink_metadata(&input.target.key) {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
        let mut files = Vec::new();
        let mut preparation = ProjectionPreparation::default();
        for source in &input.sources {
            let mode = if source.executable {
                FileMode::Executable
            } else {
                FileMode::ReadOnly
            };
            let prepared = if let Some(trusted) = &self.trusted {
                let file = trusted
                    .get(&(source.content.id.to_string(), source.executable))
                    .or_else(|| {
                        let physical = ObjectId::from_content_key(&source.locator.key).ok()?;
                        trusted.get(&(physical.to_string(), source.executable))
                    })
                    .ok_or(Error::MissingLocal)?;
                file.validate_for(&source.content, mode, input.profile.owner)?;
                AdmissionReceipt {
                    file: file.clone(),
                    preparation: Preparation::Reused,
                    read_bytes: 0,
                    copied_bytes: 0,
                }
            } else if inspect {
                self.store
                    .inspect_variant(&source.content, &source.locator, mode, input.profile.owner)?
                    .ok_or(Error::MissingLocal)?
            } else {
                self.store
                    .prepare(
                        &source.content,
                        &source.locator,
                        mode,
                        input.profile.owner,
                        input.profile.variants,
                    )
                    .map_err(|error| match error {
                        Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            Error::MissingLocal
                        }
                        other => other,
                    })?
            };
            preparation.read_bytes = preparation
                .read_bytes
                .checked_add(prepared.read_bytes)
                .ok_or(Error::TooLarge)?;
            preparation.copied_bytes = preparation
                .copied_bytes
                .checked_add(prepared.copied_bytes)
                .ok_or(Error::TooLarge)?;
            match prepared.preparation {
                Preparation::Reused => preparation.reused_files += 1,
                Preparation::Hardlink => preparation.hardlinked_files += 1,
                Preparation::Reflink => preparation.reflinked_files += 1,
                Preparation::Copy => preparation.copied_files += 1,
            }
            files.push(prepared.file);
        }
        let request = LocalTreeRequest::new(
            input.tree.clone(),
            input.target.clone(),
            input.profile.clone(),
            files,
        )?;
        let published = if inspect {
            let target = match Target::open(&self.publisher, &request, false) {
                Ok(target) => target,
                Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            };
            self.publisher.inspect(&request, &target)?
        } else {
            Some(self.publisher.publish(&request)?)
        };
        Ok(published.map(|published| ProjectionCommit {
            target: input.target.clone(),
            root: published.root,
            record: published.record,
            reused: published.reused,
            preparation,
        }))
    }
}
#[async_trait]
impl LocalMaterializer for NativeMaterializer {
    fn read_observer(&self) -> Option<std::sync::Arc<dyn polar_cas::ports::ProjectionObserver>> {
        Some(std::sync::Arc::new(self.publisher.read_observer()
            .with_sources(self.store.clone(), self.trusted.clone())))
    }
    async fn adopt_existing(&self, request: polar_cas::ports::ProjectionTargetIo) -> Result<Option<ProjectionCommit>> {
        let observer = self.publisher.read_observer().with_sources(self.store.clone(), self.trusted.clone());
        tokio::task::spawn_blocking(move || observer.run(request, true)).await
            .map_err(|_| Error::OutcomeUncertain)?
    }
    async fn materialize(&self, request: ProjectionIo) -> Result<ProjectionCommit> {
        let driver = self.clone();
        tokio::task::spawn_blocking(move || {
            driver.run(&request, false)?.ok_or(Error::OutcomeUncertain)
        })
        .await
        .map_err(|_| Error::OutcomeUncertain)?
    }
    async fn inspect_projection(&self, request: ProjectionIo) -> Result<Option<ProjectionCommit>> {
        let driver = self.clone();
        tokio::task::spawn_blocking(move || driver.run(&request, true))
            .await
            .map_err(|_| Error::OutcomeUncertain)?
    }
}
