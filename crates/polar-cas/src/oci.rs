//! Persistent OCI metadata packing and full content-addressed publication.
//! Payload claims are not ready replicas; only completed copy provides that fact.
use crate::{
    canonical::canonical_json_bytes,
    operation::{CopyInput, CopyRequest, FailureCode, OperationState, ReplicaRef},
    source::{LocationId, OperationId, RequestKey},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
pub mod limits;
pub mod projection;

pub const OCI_PACK_SCHEMA: &str = "polar.cas.oci-pack-plan.v1";
pub const OCI_EXPORT_SCHEMA: &str = "polar.cas.oci-export-plan.v1";

/// Publishes one exact metadata document. Closure ordering and durable parent
/// completion belong to the runtime, not this caller-bound transport port.
#[async_trait::async_trait]
pub trait OciPublisher: Send + Sync {
    /// Preflight a caller-selected mutable name without network I/O.
    fn validate_tag(&self, _tag: &str) -> Result<()> {
        Err(Error::UnsupportedCapability)
    }
    /// Explicitly update a mutable name; this is NOT compare-and-swap.
    async fn publish_tag(&self, _tag: &str, _document: &OciPackDocument) -> Result<OciTagCommit> {
        Err(Error::UnsupportedCapability)
    }
    /// Full read of the exact tag. A different value is Conflict, not a new
    /// immutable source version. Recovery must not silently reissue a tag PUT.
    async fn inspect_tag(
        &self,
        _tag: &str,
        _reference: &OciArtifactRef,
    ) -> Result<Option<OciTagCommit>> {
        Err(Error::UnsupportedCapability)
    }
    async fn publish_document(
        &self,
        document: &OciPackDocument,
    ) -> Result<crate::ports::ReplicaCommit>;
    async fn inspect_document(
        &self,
        reference: &OciArtifactRef,
    ) -> Result<Option<crate::ports::ReplicaCommit>>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciExportRequest {
    pub request_key: RequestKey,
    pub root: OciArtifactRef,
    pub destination: LocationId,
    pub max_candidates: u32,
    pub max_blob_bytes: u64,
    pub max_total_blob_bytes: u64,
    pub limits: OciLimits,
    /// Explicit mutable update, only after the immutable closure is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}
impl OciExportRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "export-oci-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        self.root.validate()?;
        self.limits.validate()?;
        if self
            .tag
            .as_ref()
            .is_some_and(|tag| tag.is_empty() || tag.len() > 128)
        {
            return Err(Error::Invalid("OCI tag must have 1..=128 bytes".into()));
        }
        if !(1..=1024).contains(&self.max_candidates)
            || self.max_blob_bytes > self.max_total_blob_bytes
            || self.max_total_blob_bytes > i64::MAX as u64
            || self.root.content.size_bytes > self.limits.max_document_bytes
        {
            return Err(Error::Invalid("invalid OCI export bounds".into()));
        }
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(OCI_EXPORT_SCHEMA, self))
                .map_err(|_| Error::Invalid("OCI export request encoding".into()))?,
        ))
    }
    pub fn blob_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        if content.id.algorithm() != DigestAlgorithm::Sha256 {
            return Err(Error::UnsupportedCapability);
        }
        if content.size_bytes > self.max_blob_bytes {
            return Err(Error::TooLarge);
        }
        let key =
            canonical_json_bytes(&("polar.cas.oci-export-blob.v1", self.operation_id(), content))
                .map_err(|_| Error::Invalid("OCI export child encoding".into()))?;
        Ok(CopyRequest {
            writer: None,
            request_key: RequestKey::new(format!("oci-blob-{}", ObjectId::for_bytes(&key).hex()))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.max_candidates,
            },
            destination: self.destination.clone(),
            primary_algorithm: DigestAlgorithm::Sha256,
            max_object_bytes: self.max_blob_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciExportPlan {
    pub schema: String,
    pub request: OciExportRequest,
    /// Persisted before mutable I/O. It does not mean the tag was written.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tag_attempted: bool,
}
fn is_false(value: &bool) -> bool {
    !value
}

/// Transport observation, deliberately not deserializable or a replica.
pub struct OciTagCommit {
    pub tag: String,
    pub content: crate::verification::VerifiedContent,
    pub reused: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciTagReceipt {
    pub tag: String,
    pub content: ContentRef,
    /// No tag PUT was needed during this successful attempt.
    pub reused: bool,
}
/// Confirmed object results, not wire traffic or a guarantee of future availability.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciExportStats {
    pub blobs: u64,
    pub documents: u64,
    pub copied_objects: u64,
    pub reused_objects: u64,
    pub copied_bytes: u64,
    pub reused_bytes: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciExportReceipt {
    pub operation_id: OperationId,
    pub artifact: OciArtifactRef,
    pub root: ReplicaRef,
    pub stats: OciExportStats,
    pub completed_at_ms: u64,
    /// Historical observation only; root remains the immutable digest target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<OciTagReceipt>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciExportOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: OciExportPlan,
    pub receipt: Option<OciExportReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginOciExport {
    Started(Box<OciExportOperation>),
    Replay(Box<OciExportReceipt>),
}

/// Root depth is zero. Bounds apply to metadata, not referenced payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciLimits {
    pub max_document_bytes: u64,
    pub max_metadata_bytes: u64,
    pub max_documents: u32,
    pub max_descriptors: u32,
    pub max_depth: u32,
}
impl Default for OciLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 16 * 1024 * 1024,
            max_metadata_bytes: 64 * 1024 * 1024,
            max_documents: 10_000,
            max_descriptors: 100_000,
            max_depth: 64,
        }
    }
}
impl OciLimits {
    pub fn validate(self) -> Result<()> {
        if self.max_document_bytes == 0
            || self.max_document_bytes > self.max_metadata_bytes
            || self.max_metadata_bytes > i64::MAX as u64
            || self.max_documents == 0
            || self.max_descriptors == 0
            || self.max_depth > 1024
        {
            return Err(Error::Invalid("invalid OCI metadata limits".into()));
        }
        Ok(())
    }
}

/// A compact CAS reference, not a replacement OCI Descriptor. Annotations,
/// platforms, URLs and embedded data remain in the original OCI documents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciArtifactRef {
    pub content: ContentRef,
    pub media_type: String,
}
impl OciArtifactRef {
    pub fn validate(&self) -> Result<()> {
        if self.content.id.algorithm() != DigestAlgorithm::Sha256 {
            return Err(Error::UnsupportedCapability);
        }
        if self.content.size_bytes > i64::MAX as u64 {
            return Err(Error::TooLarge);
        }
        if !matches!(
            self.media_type.as_str(),
            "application/vnd.oci.image.manifest.v1+json"
                | "application/vnd.oci.image.index.v1+json"
                | "application/vnd.docker.distribution.manifest.v2+json"
                | "application/vnd.docker.distribution.manifest.list.v2+json"
        ) {
            return Err(Error::UnsupportedCapability);
        }
        Ok(())
    }
}

/// In-process metadata input. Identity is verified, structure is checked by
/// the OCI format adapter. Bytes never enter a persistent operation plan.
#[derive(Clone, Debug)]
pub struct OciPackDocument {
    reference: OciArtifactRef,
    bytes: Vec<u8>,
}
impl OciPackDocument {
    pub fn new(reference: OciArtifactRef, bytes: Vec<u8>) -> Result<Self> {
        reference.validate()?;
        if bytes.len() as u64 != reference.content.size_bytes
            || ObjectId::for_bytes(&bytes) != reference.content.id
        {
            return Err(Error::Integrity);
        }
        Ok(Self { reference, bytes })
    }
    pub fn reference(&self) -> &OciArtifactRef {
        &self.reference
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciPackRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub request_key: RequestKey,
    pub root: OciArtifactRef,
    pub destination: LocationId,
    pub primary_algorithm: DigestAlgorithm,
    pub max_candidates: u32,
    pub limits: OciLimits,
}
impl OciPackRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "pack-oci-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        self.root.validate()?;
        self.limits.validate()?;
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        self.object_request(&self.root.content)?.digest()?;
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(OCI_PACK_SCHEMA, self))
                .map_err(|_| Error::Invalid("OCI pack request encoding".into()))?,
        ))
    }
    pub fn object_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        let key =
            canonical_json_bytes(&("polar.cas.oci-pack-child.v1", self.operation_id(), content))
                .map_err(|_| Error::Invalid("OCI pack child encoding".into()))?;
        Ok(CopyRequest {
            writer: self.writer.clone(),
            request_key: RequestKey::new(format!(
                "oci-metadata-{}",
                ObjectId::for_bytes(&key).hex()
            ))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.max_candidates,
            },
            destination: self.destination.clone(),
            primary_algorithm: self.primary_algorithm,
            max_object_bytes: self.limits.max_document_bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciPackPlan {
    pub schema: String,
    pub request: OciPackRequest,
}

/// Confirmed child results, not a wire-traffic meter or leaf availability.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciPackStats {
    pub metadata_objects: u64,
    pub metadata_bytes: u64,
    pub declared_blobs: u64,
    pub copied_objects: u64,
    pub reused_objects: u64,
    pub copied_bytes: u64,
    pub reused_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciPackReceipt {
    pub operation_id: OperationId,
    pub artifact: OciArtifactRef,
    pub root: ReplicaRef,
    pub stats: OciPackStats,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciPackOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: OciPackPlan,
    pub receipt: Option<OciPackReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginOciPack {
    Started(Box<OciPackOperation>),
    Replay(Box<OciPackReceipt>),
}
