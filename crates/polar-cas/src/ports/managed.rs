//! In-process leases for already admitted, exclusively managed local replicas.
use crate::{source::SourceLocator, ContentRef, Result};
use std::{fs::File, sync::Arc};

type Revalidate = dyn Fn(&File) -> Result<()> + Send + Sync;

/// A pinned read-only file and its exact admitted ContentRef/locator binding.
///
/// This capability is not serializable and is not a byte-verification witness.
/// It relies on the adapter's exclusive immutable-store contract, including
/// across process restarts. Descriptor ownership prevents pathname replacement
/// from changing the opened inode; it is not a cross-process GC lease, fs-verity
/// guarantee, or defense against out-of-contract same-owner payload mutation.
#[derive(Clone)]
pub struct ManagedReplica {
    file: Arc<File>,
    content: ContentRef,
    locator: SourceLocator,
    revalidate: Arc<Revalidate>,
}

impl ManagedReplica {
    /// Construct only inside a trusted managed-store adapter, after the caller
    /// established exact Catalog admission or bound an explicitly trusted
    /// SharedContent root, and the adapter opened a regular
    /// file read-only with nofollow/canonical locator, size and mode checks.
    /// Neither a caller's ContentRef claim nor a serialized receipt supplies
    /// this authority. Existing digest aliases must already be Catalog-bound.
    ///
    /// The callback must retain the storage owner and check the pinned file and
    /// current path metadata without payload I/O or mutation. It runs here and
    /// on explicit revalidation. It must not manufacture digest/admission facts.
    pub fn from_trusted_adapter(
        file: File,
        content: ContentRef,
        locator: SourceLocator,
        revalidate: impl Fn(&File) -> Result<()> + Send + Sync + 'static,
    ) -> Result<Self> {
        locator.validate()?;
        revalidate(&file)?;
        Ok(Self {
            file: Arc::new(file),
            content,
            locator,
            revalidate: Arc::new(revalidate),
        })
    }

    pub fn content(&self) -> &ContentRef {
        &self.content
    }

    pub fn locator(&self) -> &SourceLocator {
        &self.locator
    }

    /// The adapter opened this descriptor O_RDONLY. Cloning it does not copy
    /// payload; retain this lease for the cloned descriptor's useful lifetime.
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Metadata-only path/inode freshness check, never a content audit.
    pub fn revalidate(&self) -> Result<()> {
        (self.revalidate)(&self.file)
    }
}
