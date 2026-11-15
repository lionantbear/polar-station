//! Ordinary-path members of an already admitted standalone EROFS image.
//! The Catalog, not this reader, establishes image/namespace membership.
use crate::{
    async_reader::{BlockingReader, ReadBudget},
    projection::{cas_error, descriptor},
    CasImageLease, FilesystemPackReader, Member,
};
use async_trait::async_trait;
use polar_cas::{
    native_fs::filesystem_member_locator,
    ports::SourceReader,
    source::{OpaqueAccessRef, SourceLocator},
    ContentRef, Error, Reader, Result,
};
use std::{collections::BTreeMap, sync::Arc};

pub struct FilesystemSourceReader {
    image: Arc<CasImageLease>,
    members: BTreeMap<String, ContentRef>,
    budget: ReadBudget,
    paths_by_locator: BTreeMap<String, String>,
}
impl FilesystemSourceReader {
    /// Only a runtime that loaded the Catalog's verified image/tree association
    /// may bind this reader. A plain image hash or caller-created path map is
    /// not new verification evidence. Outer CAS reads still verify at EOF.
    pub fn from_catalog(
        image: CasImageLease,
        members: BTreeMap<String, ContentRef>,
        budget: ReadBudget,
    ) -> Result<Self> {
        if members.len() > 100_000 {
            return Err(Error::TooLarge);
        }
        image.revalidate()?;
        let mut paths_by_locator = BTreeMap::new();
        for (path, content) in &members {
            let locator = filesystem_member_locator(path)?;
            if paths_by_locator.insert(locator.key, path.clone()).is_some() {
                return Err(Error::Integrity);
            }
            descriptor(content)?;
        }
        Ok(Self {
            image: Arc::new(image),
            members,
            budget,
            paths_by_locator,
        })
    }
    pub async fn open_locator_member(
        &self,
        locator: &SourceLocator,
        expected: &ContentRef,
    ) -> Result<Member> {
        locator.validate()?;
        if locator.revision.is_some() {
            return Err(Error::Invalid("filesystem member revision".into()));
        }
        let path = self
            .paths_by_locator
            .get(&locator.key)
            .ok_or(Error::NotFound)?;
        self.open_member(path, expected).await
    }
    pub async fn open_member(&self, path: &str, expected: &ContentRef) -> Result<Member> {
        if self.members.get(path) != Some(expected) {
            return Err(Error::Integrity);
        }
        self.image.revalidate()?;
        let image = self.image.clone();
        let path = path.to_owned();
        let expected = descriptor(expected)?;
        let permit = self.budget.acquire().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // NativeMeta is !Send; construct and destroy it on this worker.
            // Returned Member is Send and retains the exact immutable lease.
            let view = FilesystemPackReader::open_leased(image)?;
            view.member(&path, expected).map_err(cas_error)
        })
        .await
        .map_err(|_| Error::Backend("filesystem member worker failed".into()))?
    }
}
#[async_trait]
impl SourceReader for FilesystemSourceReader {
    async fn open(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
    ) -> Result<Reader> {
        locator.validate()?;
        if locator.revision.is_some() || access.is_some() {
            return Err(Error::Invalid(
                "filesystem member has no revision or access material".into(),
            ));
        }
        let path = self
            .paths_by_locator
            .get(&locator.key)
            .ok_or(Error::NotFound)?;
        let expected = self.members.get(path).ok_or(Error::Integrity)?;
        let member = self.open_locator_member(locator, expected).await?;
        Ok(Box::pin(BlockingReader::new(member, self.budget.clone())?))
    }
}
