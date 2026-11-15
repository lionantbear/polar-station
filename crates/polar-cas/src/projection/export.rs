//! Compact durable intent for local tree publication; no filesystem I/O.
use super::{ProjectionLayout, ProjectionProfile, ProjectionStatistics, ProjectionTree};
use crate::{
    canonical::canonical_json_bytes,
    manifest::{bundle::BundleLimits, validate_name},
    operation::{FailureCode, OperationState},
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub const PROJECTION_SCHEMA: &str = "polar.cas.projection-plan.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRequest {
    pub request_key: RequestKey,
    pub root: ContentRef,
    pub local: LocationId,
    pub destination: LocationId,
    pub target: SourceLocator,
    pub layout: ProjectionLayout,
    pub profile: ProjectionProfile,
    pub algorithm: DigestAlgorithm,
    pub max_candidates: u32,
    pub limits: BundleLimits,
}
impl ProjectionRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "project-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest ID")
    }
    pub fn normalized(&self) -> Result<Self> {
        self.target.validate()?;
        if self.target.revision.is_some() {
            return Err(Error::Invalid(
                "projection target cannot select a revision".into(),
            ));
        }
        for part in self.target.key.split('/') {
            validate_name(part)?;
        }
        if !(1..=1024).contains(&self.max_candidates) {
            return Err(Error::Invalid(
                "projection candidate limit must be 1..=1024".into(),
            ));
        }
        if self.limits.max_bundle_bytes == 0
            || self.root.size_bytes > self.limits.tree.max_manifest_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        let mut request = self.clone();
        request.layout = self.layout.normalized(self.limits)?;
        // Leave room in the existing 1 MiB Catalog JSON bound for plan fields.
        if canonical_json_bytes(&request)
            .map_err(|_| Error::Integrity)?
            .len()
            > 900 * 1024
        {
            return Err(Error::TooLarge);
        }
        Ok(request)
    }
    pub fn digest(&self) -> Result<ObjectId> {
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(PROJECTION_SCHEMA, self.normalized()?))
                .map_err(|_| Error::Integrity)?,
        ))
    }
    pub fn check_tree(&self, tree: &ProjectionTree) -> Result<()> {
        if tree.input() != &self.root
            || tree.layout() != &self.layout.normalized(self.limits)?
            || tree.limits() != self.limits
            || tree.algorithm() != self.algorithm
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPlan {
    pub schema: String,
    pub request: ProjectionRequest,
    pub output: Option<ContentRef>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPreparation {
    pub read_bytes: u64,
    pub copied_bytes: u64,
    pub reused_files: u64,
    pub hardlinked_files: u64,
    pub reflinked_files: u64,
    pub copied_files: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReceipt {
    pub operation_id: OperationId,
    pub input: ContentRef,
    pub manifest: ContentRef,
    pub destination: LocationId,
    pub target: SourceLocator,
    pub root: PathBuf,
    pub profile: ProjectionProfile,
    pub layout: ProjectionLayout,
    pub statistics: ProjectionStatistics,
    pub preparation: ProjectionPreparation,
    pub reused: bool,
    pub completed_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: ProjectionPlan,
    pub receipt: Option<ProjectionReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginProjection {
    Started(Box<ProjectionOperation>),
    Replay(Box<ProjectionReceipt>),
}
