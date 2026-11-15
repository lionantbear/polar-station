//! Byte I/O ports. Credentials and endpoint lifetimes belong to the caller.
use crate::{
    manifest::bundle::TemporaryRequest,
    source::{OpaqueAccessRef, PublicBinding, SourceLocator},
    verification::VerifiedContent,
    ContentRef, Reader, Result,
};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
mod managed;
mod materialize;
pub use managed::ManagedReplica;
pub use materialize::{LocalMaterializer, ProjectionCommit, ProjectionIo, ProjectionSource, ProjectionObserver, ProjectionTargetIo};

#[async_trait]
pub trait SourceReader: Send + Sync {
    async fn open(
        &self,
        locator: &SourceLocator,
        access: Option<&OpaqueAccessRef>,
    ) -> Result<Reader>;
}

/// Read-only capability for already admitted immutable managed storage. It is
/// separate from byte sources and does not grant a commit/publish operation.
#[async_trait]
pub trait ManagedReplicaReader: Send + Sync {
    async fn open_managed(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ManagedReplica>>;
}

/// Existing managed writers can be explicitly bound through this narrower
/// read-only interface, without exposing their commit capability to a runtime.
#[async_trait]
impl<T: ReplicaWriter + ?Sized> ManagedReplicaReader for T {
    async fn open_managed(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ManagedReplica>> {
        ReplicaWriter::open_managed(self, content, locator).await
    }
}

/// Closed, synchronized, exclusively owned staging bytes. Not a network DTO.
/// The owner is retained by every clone, including a backend's blocking worker,
/// so cancellation cannot unlink its input during a physical commit.
#[derive(Clone)]
pub struct VerifiedStaging {
    path: PathBuf,
    content: VerifiedContent,
    _owner: Arc<dyn Send + Sync>,
}
impl VerifiedStaging {
    /// Only a verifier which owns this immutable staging file may construct it.
    /// A path supplied by an untrusted API is not an owned staging file.
    pub fn from_owned_staging<T: Send + Sync + 'static>(
        path: PathBuf,
        content: VerifiedContent,
        owner: T,
    ) -> Self {
        Self {
            path,
            content,
            _owner: Arc::new(owner),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn content(&self) -> &VerifiedContent {
        &self.content
    }

    pub fn retain(self, owner: impl Send + Sync + 'static) -> Self {
        Self {
            path: self.path.clone(),
            content: self.content.clone(),
            _owner: Arc::new((self, owner)),
        }
    }
}

/// Actual adapter completion or full verification, not an existence hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicaEvidenceKind {
    VerifiedWrite,
    VerifiedRead,
    TrustedLocal,
}

pub struct ReplicaCommit {
    pub content: VerifiedContent,
    pub locator: SourceLocator,
    pub evidence: PublicBinding,
    pub evidence_kind: ReplicaEvidenceKind,
    pub reused: bool,
}

/// Read-only inspection of the canonical address and recorded aliases. An
/// implementation may verify bytes but MUST NOT publish, register, repair,
/// create files, chmod, or fsync. `admitted` is asserted only for an exact Ready
/// Catalog association; otherwise a content check is required.
#[async_trait]
pub trait ReplicaObserver: Send + Sync {
    fn canonical_locator(&self, content: &ContentRef) -> Result<SourceLocator>;
    async fn present(
        &self, content: &ContentRef, locator: &SourceLocator, admitted: bool,
    ) -> Result<bool>;
}

#[async_trait]
pub trait ReplicaWriter: Send + Sync {
    /// Optional narrower capability. Returning None never authorizes callers
    /// to use a write/recovery method for read-only observation.
    fn read_observer(&self) -> Option<Arc<dyn ReplicaObserver>> { None }

    /// Explicit local shared-store capability. Remote location metadata, URLs
    /// and user-provided roots must never manufacture this authority.
    fn trusted_shared_root(&self) -> Option<crate::shared::TrustedSharedRoot> {
        None
    }
    fn locator(&self, content: &ContentRef) -> Result<SourceLocator>;
    async fn commit(&self, content: &ContentRef, staging: VerifiedStaging)
        -> Result<ReplicaCommit>;
    /// Must fully verify the exact target (or equally strong trusted evidence).
    /// HEAD, length and ETag alone must never return a successful commit fact.
    /// UnsupportedCapability denotes a representation outside this writer's
    /// scope, not a negative health observation. A SourceReader may still read
    /// it as input for a different representation at the same location.
    async fn inspect(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>>;

    /// Operational reuse/recovery. Trusted immutable stores may return an
    /// admitted local fact without rehashing; explicit inspect remains an audit.
    async fn reuse(
        &self,
        content: &ContentRef,
        locator: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>> {
        self.inspect(content, locator).await
    }

    /// Pin an already Catalog-admitted replica in an exclusively managed store.
    /// The caller must establish the Ready ContentRef/locator association first;
    /// this operation creates no admission, digest alias or replica-health fact.
    /// Implementations check confined canonical paths, regular-file size/mode
    /// and current inode metadata, and return a read-only descriptor without
    /// reading, hashing, copying, repairing or changing payload permissions.
    ///
    /// The store promises never to modify admitted bytes in place. Metadata
    /// checks do not detect every same-owner mutation or silent disk corruption;
    /// use explicit `inspect`/audit when a fresh byte-level proof is required.
    /// Missing objects return None; unsupported stores do not inherit trust.
    async fn open_managed(
        &self,
        _content: &ContentRef,
        _locator: &SourceLocator,
    ) -> Result<Option<ManagedReplica>> {
        Err(crate::Error::UnsupportedCapability)
    }
}

/// Immutable, explicitly named output. Unlike ReplicaWriter the target need
/// not be a content-address key. Output visibility is owned by the caller.
#[async_trait]
pub trait ArtifactPublisher: Send + Sync {
    /// No replacement: identical bytes may be reused after verification;
    /// different bytes at this name conflict. Never returns a planned receipt.
    async fn publish_artifact(
        &self,
        content: &ContentRef,
        target: &SourceLocator,
        staging: VerifiedStaging,
    ) -> Result<ReplicaCommit>;
    /// Fully verify the exact named output, not just its length or existence.
    async fn inspect_artifact(
        &self,
        content: &ContentRef,
        target: &SourceLocator,
    ) -> Result<Option<ReplicaCommit>>;
}

#[async_trait]
pub trait ReadLinkProvider: Send + Sync {
    /// Access material is an explicit sensitive output. The declared expiry
    /// does not guarantee provider availability or credential lifetime.
    async fn temporary_read(
        &self,
        locator: &SourceLocator,
        expires_in_seconds: u64,
    ) -> Result<TemporaryRequest>;
}

/// Caller-owned transient access material, not Catalog state or authorization.
/// Resolution must bind this handle to the exact source locator requested.
/// This is an in-process callback, not a CAS network resolver/service.
#[async_trait]
pub trait TemporaryRequestResolver: Send + Sync {
    async fn resolve_request(
        &self,
        locator: &SourceLocator,
        access: &OpaqueAccessRef,
    ) -> Result<TemporaryRequest>;
}
