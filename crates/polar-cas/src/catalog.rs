//! Transaction boundary for content facts. No storage I/O runs in this port.
use crate::{
    manifest::{TreeLimits, TreeManifest},
    operation::ReplicaRef,
    source::{
        LocationId, LocationSpec, ReplicaId, SourceId, SourceRecord, SourceSpec, SourceState,
    },
    verification::VerifiedContent,
    ContentRef, ObjectId, Result,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const MAX_REGISTER_LOCATIONS: usize = 256;
pub const MAX_REGISTER_SOURCES: usize = 100_000;
pub mod backup;
pub mod files;
pub mod native_fs;
pub mod query;
pub mod writers;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaState {
    Ready,
    Missing,
    Corrupt,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplicaRecord {
    pub replica: ReplicaRef,
    pub state: ReplicaState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationMode {
    /// Normal copy cannot rehabilitate a previously invalid source.
    Copy,
    /// Explicit re-verification may restore only the original content binding.
    Reverify,
}

#[async_trait]
pub trait Catalog: Send + Sync {
    /// Search admitted file content using existing recorded directory names.
    /// No payload reads, scans, or admission of unverified manifest children.
    async fn find_files(&self, _query: &files::FileQuery) -> Result<files::FilePage> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Opt-in metadata fence, checked atomically with this page's SQL rows.
    /// A continuation without its token or a changed token returns Conflict.
    /// Unsupported adapters must never fall back to an unfenced query.
    async fn find_files_fenced(
        &self,
        _query: &files::FileQuery,
        _expected: Option<&files::FileMetadataToken>,
    ) -> Result<files::FencedFilePage> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Capture the current metadata view before assembling runtime candidates.
    /// This marker does not bind a query predicate or grant access.
    async fn file_metadata_token(&self) -> Result<files::FileMetadataToken> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Fresh validation for callers assembling multiple candidate batches.
    async fn validate_file_metadata(&self, _expected: &files::FileMetadataToken) -> Result<()> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Scoped source claims/observations, separate from replica admission. Old
    /// adapters may opt out explicitly; callers may still read without learning.
    async fn remember_hint(
        &self,
        _scope: &ObjectId,
        _content: &ContentRef,
        _hint: &crate::source_hint::SourceHint,
        _outcome: crate::source_hint::HintOutcome,
    ) -> Result<()> {
        Err(crate::Error::UnsupportedCapability)
    }
    async fn remembered_hints(
        &self,
        _scope: &ObjectId,
        _content: &ContentRef,
    ) -> Result<Vec<crate::source_hint::RememberedHint>> {
        Ok(Vec::new())
    }

    /// Enabled location only; optional exact state, stable source-ID cursor.
    /// Like every list below, limit is 1..=1024 and no storage I/O is performed.
    async fn list_location_sources(
        &self,
        location: &LocationId,
        state: Option<SourceState>,
        after: Option<&SourceId>,
        limit: u32,
    ) -> Result<Vec<SourceRecord>>;
    /// A bounded derived index view, not full-tree verification. Child content
    /// remains a claim. The cursor is a validated single entry name.
    async fn list_manifest_entries(
        &self,
        content: &ContentRef,
        after_name: Option<&str>,
        limit: u32,
    ) -> Result<query::ManifestEntryPage>;
    /// Historical exports, including disabled destinations/expired links. Input
    /// scope merges only proven aliases. A page never proves current availability.
    async fn list_exports(
        &self,
        scope: &query::ExportScope,
        after: Option<&query::ExportCursor>,
        limit: u32,
    ) -> Result<Vec<query::ExportRecord>>;
    /// Explicit enabled binding only; never discovers storage or credentials.
    async fn location(&self, id: &LocationId) -> Result<LocationSpec>;
    /// Exact metadata lookup for an already authorized storage identity. A
    /// disabled binding is an error, never an absent identity or permission.
    async fn location_for_storage(&self, storage_identity: &str) -> Result<Option<LocationSpec>>;
    async fn register_location(&self, location: &LocationSpec) -> Result<()>;
    /// Inspect only exact Host-owned immutable bindings, including disabled
    /// bindings. This neither discovers locations nor grants access.
    async fn location_access(&self, _expected: &[LocationSpec]) -> Result<Vec<bool>> {
        Err(crate::Error::UnsupportedCapability)
    }
    /// Atomically revoke a bounded set after checking every exact binding and
    /// expected enabled state. Never enables a location or deletes any facts.
    async fn revoke_locations(
        &self,
        _expected: &[LocationSpec],
        _expected_enabled: bool,
    ) -> Result<()> {
        Err(crate::Error::UnsupportedCapability)
    }
    async fn register_source(&self, source: &SourceSpec) -> Result<SourceRecord>;
    /// Atomic, bounded registration of public claims only; no content I/O.
    async fn register_sources(
        &self,
        locations: &[LocationSpec],
        sources: &[SourceSpec],
    ) -> Result<Vec<SourceRecord>>;
    async fn source(&self, source_id: &SourceId) -> Result<SourceRecord>;
    async fn resolve(&self, id: &ObjectId) -> Result<Option<ContentRef>>;
    /// Stable-ID pagination over enabled locations; states remain observable.
    /// A claim can match directly but never creates an unverified digest alias.
    async fn list_sources(
        &self,
        content: &ObjectId,
        after: Option<&SourceId>,
        limit: u32,
    ) -> Result<Vec<SourceRecord>>;
    async fn list_replicas(
        &self,
        content: &ContentRef,
        location: Option<&LocationId>,
        after: Option<&ReplicaId>,
        limit: u32,
    ) -> Result<Vec<ReplicaRecord>>;
    /// A negative observation changes only this exact physical replica.
    async fn observe_replica(&self, replica: &ReplicaId, state: ReplicaState) -> Result<()>;
    /// Index only a committed directory object; child claims are not replicas.
    async fn index_manifest(
        &self,
        content: &ContentRef,
        bytes: &[u8],
        limits: TreeLimits,
    ) -> Result<()>;
    /// Reconstruct and verify a derived index, never infer an empty directory.
    async fn manifest(&self, content: &ContentRef, limits: TreeLimits) -> Result<TreeManifest>;
    /// Atomically recheck claims/binding and merge proven aliases. This does
    /// NOT create a ready replica or report a copy as complete.
    async fn record_verified(
        &self,
        source: Option<&SourceId>,
        content: &VerifiedContent,
        mode: VerificationMode,
    ) -> Result<ContentRef>;
    /// Observations cannot replace an immutable binding. Invalid is sticky
    /// until explicit successful Reverify, never cleared by availability.
    async fn observe_source(&self, source: &SourceId, state: SourceState) -> Result<()>;
}

pub mod hybrid_fs;
