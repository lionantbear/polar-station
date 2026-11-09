//! A bounded tree copy composed from the existing immutable object operations.
use crate::{
    canonical::canonical_json_bytes,
    manifest::{
        bundle::{
            AccessMode, BundleDraft, BundleLimits, Completeness, DirectoryRecord, ManifestBundle,
        },
        TreeLimits,
    },
    operation::{CopyInput, CopyRequest, FailureCode, OperationState, ReplicaRef},
    source::{LocationId, OperationId, RequestKey},
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};

pub const MANIFEST_COPY_SCHEMA: &str = "polar.cas.manifest-copy-plan.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyOptions {
    pub request_key: RequestKey,
    pub destination: LocationId,
    pub max_candidates: u32,
    pub max_object_bytes: u64,
    pub max_metadata_bytes: usize,
    pub limits: TreeLimits,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub root: ContentRef,
    pub options: ManifestCopyOptions,
}
impl ManifestCopyRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "copy-tree-{}",
            ObjectId::for_bytes(self.options.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        self.object_request(&self.root)?.digest()?;
        if self.options.max_metadata_bytes == 0
            || self.root.size_bytes > self.options.limits.max_manifest_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        Ok(ObjectId::for_bytes(&canonical_json_bytes(self).map_err(
            |_| Error::Invalid("tree copy request encoding".into()),
        )?))
    }
    pub fn object_request(&self, content: &ContentRef) -> Result<CopyRequest> {
        let key = canonical_json_bytes(&(
            "polar.cas.manifest-copy-child.v1",
            self.operation_id(),
            content,
        ))
        .map_err(|_| Error::Invalid("tree copy child encoding".into()))?;
        Ok(CopyRequest {
            writer: self.writer.clone(),
            request_key: RequestKey::new(format!(
                "tree-object-{}",
                ObjectId::for_bytes(&key).hex()
            ))?,
            source: CopyInput::Content {
                content: content.clone(),
                max_candidates: self.options.max_candidates,
            },
            destination: self.options.destination.clone(),
            primary_algorithm: content.id.algorithm(),
            max_object_bytes: self.options.max_object_bytes,
        })
    }
    /// The sole tree validator is shared with the exchange format. No routes
    /// are copied into this metadata-only view or any operation plan.
    pub fn validate_tree(&self, directories: &[DirectoryRecord]) -> Result<ManifestBundle> {
        self.digest()?;
        if directories.len() > self.options.limits.max_entries.saturating_add(1) {
            return Err(Error::TooLarge);
        }
        let mut bytes = 0usize;
        for directory in directories {
            bytes = bytes
                .checked_add(directory.canonical_json.len())
                .ok_or(Error::TooLarge)?;
            if bytes > self.options.max_metadata_bytes {
                return Err(Error::TooLarge);
            }
        }
        let tree = ManifestBundle::new(
            BundleDraft {
                root: self.root.clone(),
                directories: directories.to_vec(),
                storages: vec![],
                sources: vec![],
                access: AccessMode::Durable,
                completeness: Completeness::ReferencesOnly,
            },
            BundleLimits {
                tree: self.options.limits,
                max_bundle_bytes: self.options.max_metadata_bytes,
                max_directories: self.options.limits.max_entries.saturating_add(1),
                max_storages: 0,
                max_sources: 0,
                max_routes_per_object: 0,
            },
        )?;
        for content in tree
            .directories()
            .iter()
            .map(|d| &d.content)
            .chain(tree.closure().files())
        {
            self.object_request(content)?.digest()?;
        }
        Ok(tree)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyPlan {
    pub schema: String,
    pub request: ManifestCopyRequest,
}

/// Aggregates confirmed child receipts, not provider wire traffic. Failed or
/// lost-response attempts and metadata/verification reads are not byte meters.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyStats {
    pub object_references: u64,
    pub directory_references: u64,
    pub file_references: u64,
    pub copied_objects: u64,
    pub reused_objects: u64,
    pub copied_bytes: u64,
    pub reused_bytes: u64,
    pub logical_entries: u64,
    pub logical_file_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyReceipt {
    pub operation_id: OperationId,
    pub root: ReplicaRef,
    pub stats: ManifestCopyStats,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCopyOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: ManifestCopyPlan,
    pub receipt: Option<ManifestCopyReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginManifestCopy {
    Started(Box<ManifestCopyOperation>),
    Replay(Box<ManifestCopyReceipt>),
}
