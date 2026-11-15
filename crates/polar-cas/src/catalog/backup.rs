//! Ports for an independent Catalog backup. No SQLite or Node dependency.
use crate::{
    backup_v2::{
        BackupCursor, BackupInventoryPage, BackupInventorySummary, BackupLocation, BackupPlacement,
        CatalogSnapshot, CatalogSnapshotRequest,
    },
    source::LocationId,
    ContentRef, ObjectId, Result,
};
use async_trait::async_trait;

#[async_trait]
pub trait BackupCatalog: Send + Sync {
    /// Uses the existing live owner. Must not open another writer/executor.
    async fn create_backup_snapshot(
        &self,
        request: &CatalogSnapshotRequest,
    ) -> Result<CatalogSnapshot>;
}

/// An already authenticated, frozen read handle. Implementations must not
/// observe live catalog changes, migrate the input, or silently truncate.
/// Blocking methods run on a blocking worker, not a Tokio executor thread.
pub trait BackupInventory: Send + Sync {
    /// Optional fixed-head supplement admitted by the managed-root owner.
    fn shared_catalog(&self) -> Option<&crate::backup_v2::SharedCatalogBackup> {
        None
    }
    fn snapshot(&self) -> &CatalogSnapshot;
    fn summary(&self) -> Result<BackupInventorySummary>;
    fn locations(&self, limit: u32) -> Result<Vec<BackupLocation>>;
    fn objects_page(&self, after: Option<&BackupCursor>, limit: u32)
        -> Result<BackupInventoryPage>;
    fn placements(&self, content: &ContentRef, limit: u32) -> Result<Vec<BackupPlacement>>;
    /// Metadata/registration coverage only, not a fresh content hash. Exact
    /// location, unversioned canonical key, proven digest and size must match.
    fn registered_file(
        &self,
        location: &LocationId,
        key: &str,
        content: &ObjectId,
        size_bytes: u64,
    ) -> Result<bool>;
}

/// Imports logical Catalog data into an explicitly admitted empty target. It
/// must never promote historical replicas or replay source operations.
#[async_trait]
pub trait RestoreCatalog: Send + Sync {
    /// Read-only check of the imported logical graph and exact restore marker.
    /// New target operational rows are permitted; original logical facts cannot disappear.
    async fn verify_metadata(
        &self,
        path: &std::path::Path,
        expected: &CatalogSnapshot,
        request: &crate::backup_v2::CatalogMetadataRestore,
        workspace: &std::path::Path,
    ) -> Result<crate::backup_v2::CatalogMetadataRestored>;

    async fn restore_metadata(
        &self,
        path: &std::path::Path,
        expected: &CatalogSnapshot,
        request: &crate::backup_v2::CatalogMetadataRestore,
        workspace: &std::path::Path,
    ) -> Result<crate::backup_v2::CatalogMetadataRestored>;
}

/// Capability supplied by the owning host. A checker must audit every managed
/// location covered by the policy under the same source freeze/delete guard.
/// It must fail closed on unknown scope or unregistered durable files.
pub trait BackupCoverage: Send + Sync {
    fn check(
        &self,
        inventory: &dyn BackupInventory,
        policy: &crate::backup_v2::CasBackupPolicy,
    ) -> Result<()>;
}

/// Source-independent schema and inventory adapter for a sealed database image.
/// It must validate the trusted identity/digest and never migrate its input.
pub trait BackupSnapshotReader: Send + Sync {
    fn open_inventory(
        &self,
        path: &std::path::Path,
        expected: &CatalogSnapshot,
        workspace: &std::path::Path,
    ) -> Result<Box<dyn BackupInventory>>;
}

/// In-process proof/capability for an explicitly enumerated managed-root backup.
/// No network DTO can implement this by merely claiming an index root.
pub trait SharedCatalogBackupSource: Send + Sync {
    fn descriptor(&self) -> &crate::backup_v2::SharedCatalogBackup;
    fn trusted_root(&self) -> &std::path::Path;
    fn open(&self, content: &ContentRef) -> Result<crate::Reader>;
    fn cover_inventory(
        &self,
        inventory: std::sync::Arc<dyn BackupInventory>,
    ) -> std::sync::Arc<dyn BackupInventory>;
}
