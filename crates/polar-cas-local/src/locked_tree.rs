//! Strict directory ownership handoff. Never treats a path/hash/0444 mode as a seal.
//! The first provider requires immutable directory inodes and verity+immutable
//! regular files on one mount. Unsupported substrates fail closed. It does not
//! elevate privileges, remount, clone, or automatically unlock a partial seal.
use crate::{
    observe::{ObservedLocalTree, ObservedTreePlan},
    LocalSource,
};
use polar_cas::{
    locked_directory::{LockedDirectoryRecord, LockedEntry, LOCKED_DIRECTORY_SCHEMA},
    manifest::bundle::BundleLimits,
    projection::{ProjectionLayout, ProjectionNode},
    source::SourceLocator,
    Error, Result,
};
use polar_cas_fs::{IdentifiedSources, IdentityLimits};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectorySealPolicy {
    /// Check an already sealed tree; never change its inode flags.
    RequireLocked,
    /// Explicitly transfer ownership and seal. Files may become irreversible
    /// verity inodes; the caller retains any partially sealed tree on failure.
    SealOwned,
}

#[derive(Debug)]
pub struct DirectoryLockError {
    pub status: i32,
    pub os_error: i32,
    pub checked_nodes: u64,
    /// Number of successful sealing changes, not a rollback instruction.
    pub changed_nodes: u64,
}
impl fmt::Display for DirectoryLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "directory seal failed: status {}, errno {}, checked {}, changes {}; preserve partial seal",
            self.status, self.os_error, self.checked_nodes, self.changed_nodes)
    }
}
impl std::error::Error for DirectoryLockError {}

/// Not Clone/Deserialize. Pins one observed root; successful kernel checks,
/// not a caller boolean, create this capability. It still needs CAS admission.
/// Drops close descriptors but NEVER unseal the source directory.
pub struct LockedDirectory {
    tree: ObservedLocalTree,
}
impl LockedDirectory {
    pub(crate) fn from_observation(
        mut tree: ObservedLocalTree,
        policy: DirectorySealPolicy,
    ) -> std::result::Result<Self, DirectoryLockError> {
        tree.lock_kernel(policy == DirectorySealPolicy::SealOwned)?;
        Ok(Self { tree })
    }
    pub fn revalidate(&self) -> Result<()> {
        self.tree.revalidate()
    }
    pub fn entry_count(&self) -> u64 {
        self.tree.entry_count()
    }
    pub fn file_count(&self) -> u64 {
        self.tree.file_count()
    }
    pub(crate) fn bound_to(&self, source: &LocalSource, key: &str) -> bool {
        self.tree.bound_to(&source.dir, key)
    }
    /// The observer and kernel seal stay alive through the same existing
    /// identification callback. No secondary hashing/namespace implementation.
    pub fn with_identified_tree<T>(
        &self,
        limits: IdentityLimits,
        layout: &ProjectionLayout,
        tree_limits: BundleLimits,
        build: impl FnOnce(&ObservedTreePlan, &mut IdentifiedSources<'_>) -> Result<T>,
    ) -> Result<(ObservedTreePlan, T)> {
        self.tree
            .with_identified_tree(limits, layout, tree_limits, build)
    }
}


impl LockedDirectory {
    /// Reopen only an already-locked tree, compare every entry to an admitted
    /// physical record. Never enable verity, create a directory, or substitute
    /// a same-path replacement. `record` remains a claim unless its origin is
    /// anchored by the caller's Catalog; this function creates no CAS evidence.
    pub fn reopen(
        source: &LocalSource,
        locator: &SourceLocator,
        record: &LockedDirectoryRecord,
        limits: crate::observe::ObservationLimits,
        tree_limits: BundleLimits,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self> {
        record.validate(tree_limits)?;
        if record.entries.len() as u64 > limits.max_entries { return Err(Error::TooLarge); }
        let locked = source.lock_tree(locator, limits, cancelled, DirectorySealPolicy::RequireLocked)
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;
        locked.check_record(record)?;
        Ok(locked)
    }

    /// One full identity pass after locking. Reuses the existing native source
    /// identifier; cannot manufacture content identities from serialized input.
    pub fn identify_record(
        &self,
        limits: IdentityLimits,
        tree_limits: BundleLimits,
    ) -> Result<LockedDirectoryRecord> {
        let (plan, ()) = self.with_identified_tree(limits, &ProjectionLayout::Preserve, tree_limits, |_, _| Ok(()))?;
        let evidence = self.tree.seal_witnesses()?;
        if evidence.len() != plan.input().entries().len() { return Err(Error::Integrity); }
        let record = LockedDirectoryRecord {
            schema: LOCKED_DIRECTORY_SCHEMA.into(),
            tree: plan.input().output().root().clone(),
            entries: plan.input().entries().iter().zip(evidence).map(|(entry, inode)| LockedEntry {
                path: entry.path.clone(), node: entry.node.clone(), inode,
            }).collect(),
        };
        record.validate(tree_limits)?;
        self.check_record(&record)?;
        Ok(record)
    }

    /// O(entries * path depth) metadata/verity check, zero payload reads.
    pub fn check_record(&self, record: &LockedDirectoryRecord) -> Result<()> {
        let actual = self.tree.source_entries()?;
        let seals = self.tree.seal_witnesses()?;
        if actual.len() != record.entries.len() || seals.len() != actual.len() { return Err(Error::SourceContentMismatch); }
        for ((entry, seal), expected) in actual.iter().zip(seals).zip(&record.entries) {
            let structure_matches = match (&entry.node, &expected.node) {
                (ProjectionNode::Directory, ProjectionNode::Directory) => true,
                (ProjectionNode::Symlink { target: a }, ProjectionNode::Symlink { target: b }) => a == b,
                (ProjectionNode::File { executable: a, .. }, ProjectionNode::File { content, executable: b }) => a == b && seal.size_bytes == content.size_bytes,
                _ => false,
            };
            if entry.path != expected.path || seal != expected.inode || !structure_matches {
                return Err(Error::SourceContentMismatch);
            }
        }
        self.revalidate()
    }

    pub(crate) fn open_member_file(&self, index: usize) -> Result<std::fs::File> {
        self.tree.open_entry_file(index)
    }
}
