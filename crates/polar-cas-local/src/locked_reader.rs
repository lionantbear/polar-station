//! Host-bound original-directory access. One bounded namespace check per reopen;
//! each member open revalidates its path and inode through the existing observer.
use crate::{locked_tree::LockedDirectory, observe::ObservationLimits, LocalSource};
use async_trait::async_trait;
use polar_cas::{
    locked_directory::LockedDirectoryRecord,
    manifest::bundle::BundleLimits,
    native_fs::filesystem_member_locator,
    ports::SourceReader,
    projection::ProjectionNode,
    source::{OpaqueAccessRef, SourceLocator},
    ContentRef, Error, Reader, Result,
};
use std::{collections::BTreeMap, fs::File, sync::{Arc, Mutex, atomic::AtomicBool}};

/// No raw-path constructor. Caller supplies an admitted Catalog record and its
/// authorized LocalSource. Never binds a new source automatically on cache miss.
#[derive(Clone)]
pub struct LockedDirectoryReader {
    locked: Arc<Mutex<LockedDirectory>>,
    record: Arc<LockedDirectoryRecord>,
    members: Arc<BTreeMap<String, (usize, ContentRef)>>,
}
impl LockedDirectoryReader {
    pub fn reopen_from_catalog(
        source: &LocalSource,
        locator: &SourceLocator,
        record: LockedDirectoryRecord,
        observation: ObservationLimits,
        limits: BundleLimits,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let locked = LockedDirectory::reopen(source, locator, &record, observation, limits, cancel)?;
        Self::assemble(locked, record)
    }
    pub fn from_locked(locked: LockedDirectory, record: LockedDirectoryRecord) -> Result<Self> {
        record.validate(BundleLimits::default())?;
        locked.check_record(&record)?;
        Self::assemble(locked, record)
    }
    // Callers have already checked the complete live record; avoid a second
    // cold namespace/verity pass just to build the in-memory member index.
    fn assemble(locked: LockedDirectory, record: LockedDirectoryRecord) -> Result<Self> {
        let mut members = BTreeMap::new();
        for (i, e) in record.entries.iter().enumerate() {
            if let ProjectionNode::File { content, .. } = &e.node {
                let key = filesystem_member_locator(&e.path)?.key;
                if members.insert(key, (i, content.clone())).is_some() { return Err(Error::Integrity); }
            }
        }
        Ok(Self { locked: Arc::new(Mutex::new(locked)), record: Arc::new(record), members: Arc::new(members) })
    }
    pub fn record(&self) -> &LockedDirectoryRecord { &self.record }
    pub fn revalidate(&self) -> Result<()> {
        self.locked.lock().map_err(|_| Error::Backend("directory reader poisoned".into()))?.check_record(&self.record)
    }
    /// Returns a new readonly kernel-verified fd without full-file hashing or
    /// staging a copy. SourceReader consumers still verify ordinary CAS hashes.
    pub fn open_member(&self, locator: &SourceLocator, expected: &ContentRef) -> Result<File> {
        locator.validate()?;
        if locator.revision.is_some() { return Err(Error::UnsupportedCapability); }
        let (index, content) = self.members.get(&locator.key).ok_or(Error::NotFound)?;
        if content != expected { return Err(Error::Integrity); }
        let file = self.locked.lock().map_err(|_| Error::Backend("directory reader poisoned".into()))?.open_member_file(*index)?;
        if file.metadata()?.len() != content.size_bytes { return Err(Error::SourceContentMismatch); }
        Ok(file)
    }
}
#[async_trait]
impl SourceReader for LockedDirectoryReader {
    async fn open(&self, locator: &SourceLocator, access: Option<&OpaqueAccessRef>) -> Result<Reader> {
        if access.is_some() { return Err(Error::UnsupportedCapability); }
        let expected = self.members.get(&locator.key).ok_or(Error::NotFound)?.1.clone();
        let reader = self.clone();
        let locator = locator.clone();
        let file = tokio::task::spawn_blocking(move || reader.open_member(&locator, &expected)).await
            .map_err(|_| Error::Backend("locked-directory read worker failed".into()))??;
        Ok(Box::pin(tokio::fs::File::from_std(file)))
    }
}
