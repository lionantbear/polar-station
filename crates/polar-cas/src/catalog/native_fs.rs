//! Optional native filesystem Catalog capability. Byte I/O stays outside SQL.
use crate::{
    native_fs::{NativeProjection, VerifiedNativeProjection, VerifiedPackedMembers},
    operation::ReplicaRef,
    source::LocationId,
    ContentRef, Result,
};
use async_trait::async_trait;

#[async_trait]
pub trait NativeFsCatalog: Send + Sync {
    /// One transaction: binding + verified member replicas. Does not create
    /// Source rows, copy payloads, delete loose replicas or authorize host GC.
    async fn register_pack_members(
        &self,
        location: &LocationId,
        witness: &VerifiedPackedMembers,
    ) -> Result<Vec<ReplicaRef>>;
    /// Persist a compiled/verified mapping only after all image and Tree
    /// identities have been admitted through the ordinary content Catalog.
    async fn register_native_projection(
        &self,
        witness: &VerifiedNativeProjection,
    ) -> Result<String>;
    /// Register verified ordinary-path membership using existing locations and
    /// replica tables. All content and directory objects must already be known.
    /// The trusted native producer/receiver must validate the exact image/tree.
    async fn register_filesystem_pack(
        &self,
        _witness: &VerifiedNativeProjection,
        _tree: &crate::projection::ProjectionTree,
    ) -> Result<String> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Register the original immutable directory's file locations atomically.
    /// Metadata record and directory objects must already be admitted. No IO,
    /// no logical UUID/version state, and no payload copies happen in SQL.
    async fn register_locked_directory(
        &self,
        _witness: &crate::locked_directory::VerifiedLockedDirectory,
    ) -> Result<crate::locked_directory::LockedDirectoryBinding> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Physical directory views for a known exact T. No raw directory is exposed
    /// as the bytes of its canonical manifest; file replicas use member locators.
    async fn list_locked_directories(
        &self,
        _tree: &ContentRef,
        _after: Option<&LocationId>,
        _limit: u32,
    ) -> Result<Vec<crate::locked_directory::LockedDirectoryBinding>> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// A durable description, NOT a live mount or current availability proof.
    async fn native_projection(&self, compilation_key: &str) -> Result<NativeProjection>;
    /// Bounded dependency index for retention/audit. No automatic deletion.
    async fn list_native_pack_dependents(
        &self,
        pack: &ContentRef,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<String>>;
}
