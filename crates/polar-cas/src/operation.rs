//! Explicit copy intent and non-secret historical completion.
use crate::{
    canonical::canonical_json_bytes,
    catalog::Catalog,
    manifest::bundle::{DirectoryRecord, ManifestBundle},
    manifest::TreeManifest,
    manifest_copy::{
        BeginManifestCopy, ManifestCopyOperation, ManifestCopyReceipt, ManifestCopyRequest,
    },
    pack::{BeginPack, ManifestPackRequest, PackOperation, PackReceipt},
    ports::ProjectionCommit,
    ports::ReplicaCommit,
    projection::{
        export::{BeginProjection, ProjectionOperation, ProjectionReceipt, ProjectionRequest},
        ProjectionTree,
    },
    publish::{
        BeginManifestExport, ManifestExportOperation, ManifestExportReceipt, ManifestExportRequest,
        PublishReceipt,
    },
    source::{LocationId, OperationId, ReplicaId, RequestKey, SourceId, SourceLocator},
    verification::VerifiedContent,
    ContentClaim, ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The string variant preserves the original source-only request encoding.
/// Reader carries a claim, never the stream or a fabricated source locator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum CopyInput {
    Source(SourceId),
    Content {
        content: ContentRef,
        max_candidates: u32,
    },
    Reader {
        reader: ContentClaim,
    },
}
impl From<SourceId> for CopyInput {
    fn from(source: SourceId) -> Self {
        Self::Source(source)
    }
}
impl CopyInput {
    pub fn check(&self, verified: &VerifiedContent) -> Result<()> {
        let matches = match self {
            Self::Source(_) => true, // The catalog checks the immutable source.
            Self::Content { content, .. } => {
                verified.ids().contains(&content.id) && verified.size_bytes() == content.size_bytes
            }
            Self::Reader { reader } => verified.satisfies(reader),
        };
        if matches {
            Ok(())
        } else {
            Err(Error::ClaimMismatch)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRequest {
    /// Runtime-frozen attribution. Wire-facing adapters never select this field.
    /// Omission preserves the canonical digest of all legacy requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub request_key: RequestKey,
    pub source: CopyInput,
    pub destination: LocationId,
    pub primary_algorithm: DigestAlgorithm,
    pub max_object_bytes: u64,
}
impl CopyRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "copy-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        if self.max_object_bytes == 0 || self.max_object_bytes > i64::MAX as u64 {
            return Err(Error::Invalid(
                "copy requires a positive bounded size limit".into(),
            ));
        }
        match &self.source {
            CopyInput::Content {
                content,
                max_candidates,
            } => {
                if *max_candidates == 0 || *max_candidates > 1024 {
                    return Err(Error::Invalid(
                        "copy candidate limit must be 1..=1024".into(),
                    ));
                }
                if content.size_bytes > self.max_object_bytes {
                    return Err(Error::TooLarge);
                }
            }
            CopyInput::Reader { reader }
                if reader
                    .size_bytes
                    .is_some_and(|size| size > self.max_object_bytes) =>
            {
                return Err(Error::TooLarge);
            }
            _ => {}
        }
        Ok(ObjectId::for_bytes(&canonical_json_bytes(self).map_err(
            |_| Error::Invalid("copy request encoding".into()),
        )?))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Prepared,
    Completed,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Ingest,
    Copy,
    Pack,
    #[serde(rename = "export")]
    Publish,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "receipt",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OperationOutcome {
    Ingest(Box<crate::ingest::TreeIngestReceipt>),
    PackOci(crate::oci::OciPackReceipt),
    PackArchive(crate::archive::ArchivePackReceipt),
    Copy(CopyReceipt),
    CopyManifest(ManifestCopyReceipt),
    Pack(PackReceipt),
    Publish(PublishReceipt),
}

/// Stable, non-secret failure codes; provider messages and access URLs are not
/// persisted in the operation log.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    Invalid,
    NotFound,
    Integrity,
    TooLarge,
    Io,
    Backend,
    Conflict,
    ClaimMismatch,
    SourceContentMismatch,
    SourceInvalid,
    InProgress,
    OutcomeUncertain,
    MissingDigest,
    MissingLocal,
    UnsupportedCapability,
    AccessMaterialUnavailable,
}
impl FailureCode {
    pub fn from_error(error: &Error) -> Self {
        match error {
            Error::Invalid(_) => Self::Invalid,
            Error::NotFound => Self::NotFound,
            Error::Integrity => Self::Integrity,
            Error::TooLarge => Self::TooLarge,
            Error::Io(_) => Self::Io,
            Error::Backend(_) => Self::Backend,
            Error::Conflict => Self::Conflict,
            Error::ClaimMismatch => Self::ClaimMismatch,
            Error::SourceContentMismatch => Self::SourceContentMismatch,
            Error::SourceInvalid => Self::SourceInvalid,
            Error::InProgress => Self::InProgress,
            Error::OutcomeUncertain => Self::OutcomeUncertain,
            Error::MissingDigest => Self::MissingDigest,
            Error::MissingLocal => Self::MissingLocal,
            Error::UnsupportedCapability => Self::UnsupportedCapability,
            Error::AccessMaterialUnavailable => Self::AccessMaterialUnavailable,
            Error::OperationFailed(code) => *code,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Integrity => "integrity",
            Self::TooLarge => "too_large",
            Self::Io => "io",
            Self::Backend => "backend",
            Self::Conflict => "conflict",
            Self::ClaimMismatch => "claim_mismatch",
            Self::SourceContentMismatch => "source_content_mismatch",
            Self::SourceInvalid => "source_invalid",
            Self::InProgress => "in_progress",
            Self::OutcomeUncertain => "outcome_uncertain",
            Self::MissingDigest => "missing_digest",
            Self::MissingLocal => "missing_local",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::AccessMaterialUnavailable => "access_material_unavailable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyPlan {
    /// Durable physical result awaiting Shared Catalog publication. The existing
    /// SQL CHECK permits formal receipt_json only in Completed, so this frozen
    /// provisional outcome lives in the original plan, not a second table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_writer_receipt: Option<CopyReceipt>,
    pub schema: String,
    pub request: CopyRequest,
    pub stage_name: Option<String>,
    pub content: Option<ContentRef>,
    pub target: Option<SourceLocator>,
    /// Actual source read for a content-based request, if any. Old plans omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_source: Option<SourceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicaRef {
    pub id: ReplicaId,
    pub content: ContentRef,
    pub location: LocationId,
    pub locator: SourceLocator,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyReceipt {
    /// Copied from the original operation plan, never the recovery operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub operation_id: OperationId,
    pub replica: ReplicaRef,
    pub reused: bool,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: CopyPlan,
    pub receipt: Option<CopyReceipt>,
    pub failure: Option<FailureCode>,
}

pub enum BeginCopy {
    Started(Box<CopyOperation>),
    Replay(Box<CopyReceipt>),
}

/// An in-process in-flight guard, not a persistent lease, permission, or job.
/// Releasing it only allows explicit recovery; it does not retry any I/O.
#[derive(Clone)]
pub struct OperationGuard {
    _owner: std::sync::Arc<dyn Send + Sync>,
}
impl OperationGuard {
    pub fn from_owner(owner: impl Send + Sync + 'static) -> Self {
        Self {
            _owner: std::sync::Arc::new(owner),
        }
    }
}

#[async_trait]
pub trait OperationCatalog: Catalog {
    /// Ingest reuses this operation log and its execution guard. Older Catalog
    /// adapters may explicitly omit the additive capability.
    async fn begin_tree_ingest(
        &self,
        _request: &crate::ingest::TreeIngestRequest,
        _storage: crate::ingest::TreeIngestStorage,
    ) -> Result<crate::ingest::BeginTreeIngest> {
        Err(Error::UnsupportedCapability)
    }
    async fn tree_ingest_operation(
        &self,
        _id: &OperationId,
    ) -> Result<crate::ingest::TreeIngestOperation> {
        Err(Error::UnsupportedCapability)
    }
    async fn prepare_tree_ingest(
        &self,
        _id: &OperationId,
        _frozen: &crate::ingest::TreeIngestFrozen,
    ) -> Result<()> {
        Err(Error::UnsupportedCapability)
    }
    /// Members, complete input availability and checkpoint become visible in
    /// one transaction. P itself was admitted by ordinary child Copy operations.
    async fn commit_tree_ingest_storage(
        &self,
        _id: &OperationId,
        _receipt: &crate::ingest::TreeIngestStorageReceipt,
        _packed: &[(LocationId, crate::native_fs::VerifiedPackedMembers)],
    ) -> Result<()> {
        Err(Error::UnsupportedCapability)
    }
    async fn prepare_tree_ingest_meta(
        &self,
        _id: &OperationId,
        _plan: &crate::hybrid_fs::HybridCompilePlan,
    ) -> Result<()> {
        Err(Error::UnsupportedCapability)
    }
    /// The existing Hybrid compiler has registered the exact variant. Final
    /// Source->Tin binding and complete receipt commit atomically here.
    async fn complete_tree_ingest(
        &self,
        _id: &OperationId,
        _projection: &crate::hybrid_fs::HybridProjection,
        _compilation_key: &str,
        _meta_copy: &CopyReceipt,
    ) -> Result<crate::ingest::TreeIngestReceipt> {
        Err(Error::UnsupportedCapability)
    }
    async fn begin_oci_projection(
        &self,
        request: &crate::oci::projection::OciProjectionRequest,
    ) -> Result<crate::oci::projection::BeginOciProjection>;
    async fn oci_projection_operation(
        &self,
        id: &OperationId,
    ) -> Result<crate::oci::projection::OciProjectionOperation>;
    async fn prepare_oci_projection(
        &self,
        id: &OperationId,
        rendered: &ProjectionTree,
    ) -> Result<()>;
    async fn complete_oci_projection(
        &self,
        id: &OperationId,
        rendered: &ProjectionTree,
        commit: ProjectionCommit,
    ) -> Result<crate::oci::projection::OciProjectionReceipt>;
    async fn begin_oci_export(
        &self,
        request: &crate::oci::OciExportRequest,
    ) -> Result<crate::oci::BeginOciExport>;
    async fn oci_export_operation(
        &self,
        id: &OperationId,
    ) -> Result<crate::oci::OciExportOperation>;
    async fn prepare_oci_tag(&self, id: &OperationId) -> Result<()>;
    async fn complete_oci_export(
        &self,
        id: &OperationId,
        documents: &[crate::oci::OciPackDocument],
        commits: Vec<crate::ports::ReplicaCommit>,
        tag: Option<crate::oci::OciTagCommit>,
    ) -> Result<crate::oci::OciExportReceipt>;
    /// Stable-ID, 1..=1024 observational page of prepared/uncertain operations.
    /// No execution claim, plan decoding, automatic retry, or work-queue promise.
    async fn list_recovery(
        &self,
        after: Option<&OperationId>,
        limit: u32,
    ) -> Result<Vec<crate::catalog::query::RecoveryRecord>>;
    async fn begin_oci_pack(
        &self,
        request: &crate::oci::OciPackRequest,
    ) -> Result<crate::oci::BeginOciPack>;
    async fn oci_pack_operation(&self, id: &OperationId) -> Result<crate::oci::OciPackOperation>;
    async fn complete_oci_pack(
        &self,
        id: &OperationId,
        documents: &[crate::oci::OciPackDocument],
    ) -> Result<crate::oci::OciPackReceipt>;
    async fn begin_archive_pack(
        &self,
        request: &crate::archive::ArchivePackRequest,
    ) -> Result<crate::archive::BeginArchivePack>;
    async fn archive_pack_operation(
        &self,
        id: &OperationId,
    ) -> Result<crate::archive::ArchivePackOperation>;
    async fn prepare_archive_pack(
        &self,
        id: &OperationId,
        content: &VerifiedContent,
        target: &SourceLocator,
        stage_name: &str,
    ) -> Result<ContentRef>;
    async fn complete_archive_pack(
        &self,
        id: &OperationId,
        commit: ReplicaCommit,
    ) -> Result<crate::archive::ArchivePackReceipt>;
    async fn begin_iroh_export(
        &self,
        request: &crate::iroh::IrohExportRequest,
    ) -> Result<crate::iroh::BeginIrohExport>;
    async fn iroh_export_operation(
        &self,
        id: &OperationId,
    ) -> Result<crate::iroh::IrohExportOperation>;
    async fn prepare_iroh_export(
        &self,
        id: &OperationId,
        input: &crate::iroh::IrohExportInput,
        prepared: &crate::iroh::IrohPrepared,
    ) -> Result<()>;
    async fn complete_iroh_export(
        &self,
        id: &OperationId,
        input: &crate::iroh::IrohExportInput,
        commit: crate::iroh::IrohCommit,
    ) -> Result<crate::iroh::IrohExportReceipt>;
    async fn begin_projection(&self, request: &ProjectionRequest) -> Result<BeginProjection>;
    async fn projection_operation(&self, id: &OperationId) -> Result<ProjectionOperation>;
    async fn prepare_projection(&self, id: &OperationId, tree: &ProjectionTree) -> Result<()>;
    async fn complete_projection(
        &self,
        id: &OperationId,
        tree: &ProjectionTree,
        commit: ProjectionCommit,
    ) -> Result<ProjectionReceipt>;
    async fn begin_manifest_export(
        &self,
        request: &ManifestExportRequest,
    ) -> Result<BeginManifestExport>;
    async fn manifest_export_operation(&self, id: &OperationId) -> Result<ManifestExportOperation>;
    async fn prepare_manifest_export(
        &self,
        id: &OperationId,
        bundle: &ManifestBundle,
        stage_name: &str,
    ) -> Result<ContentRef>;
    async fn complete_manifest_export(
        &self,
        id: &OperationId,
        commit: ReplicaCommit,
    ) -> Result<ManifestExportReceipt>;
    fn claim_execution(&self, operation: &OperationId) -> Result<OperationGuard>;
    async fn operation_kind(&self, id: &OperationId) -> Result<OperationKind>;
    async fn operation_schema(&self, id: &OperationId) -> Result<String>;
    async fn begin_manifest_copy(&self, request: &ManifestCopyRequest)
        -> Result<BeginManifestCopy>;
    async fn manifest_copy_operation(&self, id: &OperationId) -> Result<ManifestCopyOperation>;
    async fn complete_manifest_copy(
        &self,
        id: &OperationId,
        directories: &[DirectoryRecord],
    ) -> Result<ManifestCopyReceipt>;
    async fn fail_operation(
        &self,
        id: &OperationId,
        uncertain: bool,
        code: FailureCode,
    ) -> Result<()>;
    async fn acknowledge_pack_writer(&self, receipt: &PackReceipt) -> Result<()> {
        if receipt.writer.is_some() {
            Err(Error::UnsupportedCapability)
        } else {
            Ok(())
        }
    }
    async fn begin_pack(&self, request: &ManifestPackRequest) -> Result<BeginPack>;
    async fn pack_operation(&self, id: &OperationId) -> Result<PackOperation>;
    async fn prepare_pack(
        &self,
        id: &OperationId,
        target: &SourceLocator,
        stage_name: Option<&str>,
    ) -> Result<()>;
    async fn complete_pack(
        &self,
        id: &OperationId,
        commit: ReplicaCommit,
        tree: &TreeManifest,
    ) -> Result<PackReceipt>;
    async fn begin_copy(&self, request: &CopyRequest) -> Result<BeginCopy>;
    async fn copy_operation(&self, id: &OperationId) -> Result<CopyOperation>;
    async fn update_copy_stage(&self, id: &OperationId, stage_name: &str) -> Result<()>;
    /// Bind the source and freeze the final target in the SAME transaction,
    /// before any external commit. No ready replica is created here.
    async fn prepare_copy(
        &self,
        id: &OperationId,
        content: &VerifiedContent,
        target: &SourceLocator,
        read_source: Option<&SourceId>,
    ) -> Result<ContentRef>;
    /// Commit physical evidence and its immutable receipt. With writer attribution,
    /// the operation remains Uncertain until acknowledge_copy_writer confirms the
    /// Shared Catalog publication. Legacy unattributed saves complete here.
    async fn complete_copy(&self, id: &OperationId, commit: ReplicaCommit) -> Result<CopyReceipt>;
    /// Existing operation journal only; this does not publish/invent writer facts.
    /// Writer-aware adapters must keep pending publication discoverable by recovery.
    async fn acknowledge_copy_writer(&self, _receipt: &CopyReceipt) -> Result<()> {
        Err(Error::UnsupportedCapability)
    }
    async fn fail_copy(&self, id: &OperationId, uncertain: bool, code: FailureCode) -> Result<()>;
}

/// Compatibility name for the original copy-only catalog port.
pub use OperationCatalog as CopyCatalog;
