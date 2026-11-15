//! Tree ingest progress in the existing operation log. Descriptors are durable
//! recovery inputs, not storage-admission or live-filesystem capabilities.
use crate::{
    canonical::canonical_json_bytes,
    hybrid_fs::{HybridCompilePlan, HybridProjection, HybridTreeProfile},
    manifest::bundle::{
        AccessMode, BundleDraft, BundleLimits, Completeness, DirectoryRecord, ManifestBundle,
    },
    operation::{CopyReceipt, FailureCode, OperationState},
    projection::{ProjectionLayout, ProjectionTree},
    source::{LocationId, OperationId, RequestKey, SourceId, SourceSpec},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
mod definition;
pub use definition::{TreeDefinition, TreeFileInput, TreeSubtree, TreeSubtreeInput};

pub const TREE_INGEST_SCHEMA: &str = "polar.cas.tree-ingest-plan.v1";
pub const DEFINED_TREE_INGEST_SCHEMA: &str = "polar.cas.tree-ingest-plan.v2";
pub const ATTRIBUTED_TREE_INGEST_SCHEMA: &str = "polar.cas.tree-ingest-plan.v3";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeIngestStorage {
    Loose,
    Erofs,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestRequest {
    /// Runtime-frozen parent attribution, inherited by recovery-created children.
    /// Omission preserves unknown legacy origin and its canonical request digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub request_key: RequestKey,
    /// An immutable observation identity. Its binding is always Tin, never T/P/M.
    pub source: SourceSpec,
    /// Omitted for legacy directory observation. Source still binds aggregate Tin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<TreeDefinition>,
    pub destination: LocationId,
    pub algorithm: DigestAlgorithm,
    pub layout: ProjectionLayout,
    pub limits: BundleLimits,
    pub profile: HybridTreeProfile,
    pub include_existing_loose: bool,
    pub max_candidates: u32,
    pub max_source_bytes: u64,
    pub max_output_bytes: u64,
}
impl TreeIngestRequest {
    pub fn schema(&self) -> &'static str {
        if self.writer.is_some() {
            ATTRIBUTED_TREE_INGEST_SCHEMA
        } else if self.definition.is_some() {
            DEFINED_TREE_INGEST_SCHEMA
        } else {
            TREE_INGEST_SCHEMA
        }
    }
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "ingest-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self, storage: TreeIngestStorage) -> Result<ObjectId> {
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        self.source.locator.validate()?;
        if !(1..=1024).contains(&self.max_candidates)
            || self.max_source_bytes == 0
            || self.max_source_bytes > i64::MAX as u64
            || self.max_output_bytes == 0
            || self.max_output_bytes > i64::MAX as u64
            || self.limits.max_bundle_bytes == 0
            || self.limits.max_directories == 0
            || self.limits.tree.max_manifest_bytes == 0
        {
            return Err(Error::Invalid(
                "tree ingest requires bounded nonzero budgets".into(),
            ));
        }
        if self
            .source
            .expected
            .size_bytes
            .is_some_and(|n| n > i64::MAX as u64)
        {
            return Err(Error::TooLarge);
        }
        let mut normalized = self.clone();
        normalized.layout = self.layout.normalized(self.limits)?;
        if let Some(definition) = &self.definition {
            normalized.definition = Some(definition.normalized(self.limits)?);
        }
        let bytes = canonical_json_bytes(&(self.schema(), storage, normalized))
            .map_err(|_| Error::Invalid("tree ingest request encoding".into()))?;
        Ok(ObjectId::for_bytes(&bytes))
    }
}

/// Complete input scope plus its transformed target. Storing the canonical
/// directory closure makes recovery independent of the original namespace once
/// storage is ready, including input files excluded by Select.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestFrozen {
    pub input_tree: ContentRef,
    pub target_tree: ContentRef,
    pub input_directories: Vec<DirectoryRecord>,
    pub target_directories: Vec<DirectoryRecord>,
}
impl TreeIngestFrozen {
    pub fn input_bundle(&self, limits: BundleLimits) -> Result<ManifestBundle> {
        metadata_bundle(&self.input_tree, &self.input_directories, limits)
    }
    pub fn target_bundle(&self, limits: BundleLimits) -> Result<ManifestBundle> {
        metadata_bundle(&self.target_tree, &self.target_directories, limits)
    }
    pub fn validate(&self, request: &TreeIngestRequest) -> Result<()> {
        let input = self.input_bundle(request.limits)?;
        let target = self.target_bundle(request.limits)?;
        if input.closure().logical_file_bytes() > request.max_source_bytes {
            return Err(Error::TooLarge);
        }
        let planned = ProjectionTree::plan(
            &self.input_tree,
            &self.input_directories,
            &request.layout,
            request.algorithm,
            request.limits,
        )?;
        if planned.output().root() != target.root()
            || planned.output().directories() != target.directories()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
fn metadata_bundle(
    root: &ContentRef,
    directories: &[DirectoryRecord],
    limits: BundleLimits,
) -> Result<ManifestBundle> {
    ManifestBundle::new(
        BundleDraft {
            root: root.clone(),
            directories: directories.to_vec(),
            storages: vec![],
            sources: vec![],
            access: AccessMode::Durable,
            completeness: Completeness::ReferencesOnly,
        },
        limits,
    )
}

/// Exact child-copy completions, not transfer byte counters. New packs have
/// ordinary CAS replicas; their member replicas use the existing EROFS binding.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestStorageReceipt {
    pub loose: Vec<CopyReceipt>,
    pub new_packs: Vec<CopyReceipt>,
    pub reused_packs: Vec<ContentRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeIngestCheckpoint {
    Prepared,
    StorageReady,
    MetaPlanned,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestPlan {
    pub schema: String,
    pub request: TreeIngestRequest,
    pub storage: TreeIngestStorage,
    pub frozen: Option<TreeIngestFrozen>,
    pub storage_receipt: Option<TreeIngestStorageReceipt>,
    pub compile_plan: Option<HybridCompilePlan>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestStatistics {
    pub input_file_references: u64,
    pub input_logical_file_bytes: u64,
    pub target_file_references: u64,
    pub target_logical_file_bytes: u64,
    pub new_loose_payload_files: u64,
    pub new_pack_images: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestReceipt {
    pub operation_id: OperationId,
    pub source: SourceId,
    pub input_tree: ContentRef,
    pub target_tree: ContentRef,
    pub storage: TreeIngestStorageReceipt,
    pub projection: HybridProjection,
    pub compilation_key: String,
    pub meta_copy: CopyReceipt,
    pub statistics: TreeIngestStatistics,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeIngestOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: TreeIngestPlan,
    pub receipt: Option<TreeIngestReceipt>,
    pub failure: Option<FailureCode>,
}
impl TreeIngestOperation {
    pub fn checkpoint(&self) -> TreeIngestCheckpoint {
        if self.state == OperationState::Completed {
            TreeIngestCheckpoint::Completed
        } else if self.plan.compile_plan.is_some() {
            TreeIngestCheckpoint::MetaPlanned
        } else if self.plan.storage_receipt.is_some() {
            TreeIngestCheckpoint::StorageReady
        } else {
            TreeIngestCheckpoint::Prepared
        }
    }
}
pub enum BeginTreeIngest {
    /// Existing prepared/uncertain operations resume under claim_execution.
    Started(Box<TreeIngestOperation>),
    Replay(Box<TreeIngestReceipt>),
}
