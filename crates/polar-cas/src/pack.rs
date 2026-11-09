//! Directory packing records. Leaf references do not assert payload availability.
use crate::{
    canonical::canonical_json_bytes,
    manifest::TreeLimits,
    operation::{FailureCode, OperationState, ReplicaRef},
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackOptions {
    pub request_key: RequestKey,
    pub destination: LocationId,
    pub algorithm: DigestAlgorithm,
    pub max_candidates: u32,
    pub limits: TreeLimits,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPackRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub options: PackOptions,
    pub manifest: ContentRef,
}
impl ManifestPackRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "pack-{}",
            ObjectId::for_bytes(self.options.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        if !(1..=1024).contains(&self.options.max_candidates) {
            return Err(Error::Invalid(
                "pack candidate limit must be 1..=1024".into(),
            ));
        }
        if self.manifest.id.algorithm() != self.options.algorithm {
            return Err(Error::Invalid(
                "pack algorithm does not match directory identity".into(),
            ));
        }
        if self.manifest.size_bytes > self.options.limits.max_manifest_bytes as u64
            || self.manifest.size_bytes > i64::MAX as u64
        {
            return Err(Error::TooLarge);
        }
        Ok(ObjectId::for_bytes(&canonical_json_bytes(self).map_err(
            |_| Error::Invalid("pack request encoding".into()),
        )?))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPackPlan {
    pub schema: String,
    pub request: ManifestPackRequest,
    pub target: Option<SourceLocator>,
    pub stage_name: Option<String>,
    /// Physical receipt awaits Shared Catalog publication in the same operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_writer_receipt: Option<PackReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<crate::catalog::writers::SaveAttribution>,
    pub operation_id: OperationId,
    pub manifest: ContentRef,
    pub replica: ReplicaRef,
    pub entry_count: u64,
    pub reused: bool,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: ManifestPackPlan,
    pub receipt: Option<PackReceipt>,
    pub failure: Option<FailureCode>,
}

pub enum BeginPack {
    Started(Box<PackOperation>),
    Replay(Box<PackReceipt>),
}
