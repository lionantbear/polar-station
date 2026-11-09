//! Reproducible simplified-tree packages, distinct from the original tree root.
use crate::{
    canonical::canonical_json_bytes,
    manifest::bundle::BundleLimits,
    operation::{FailureCode, OperationState, ReplicaRef},
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};

pub const ARCHIVE_PACK_SCHEMA: &str = "polar.cas.archive-pack-plan.v1";

/// V1: GNU tar, sorted paths, root '.', uid/gid/mtime=0, empty owner names,
/// file=0444, executable/directory=0555, relative symlink=0777; no hardlinks.
/// Gzip additionally uses level 6, mtime 0, OS 255 and no name/comment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveProfile {
    TarV1,
    TarGzipV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePackRequest {
    pub request_key: RequestKey,
    pub root: ContentRef,
    pub destination: LocationId,
    pub algorithm: DigestAlgorithm,
    pub profile: ArchiveProfile,
    pub max_candidates: u32,
    /// Includes tar metadata, padding and GNU extension records.
    pub max_tar_bytes: u64,
    /// Actual stored bytes, including the compression envelope if selected.
    pub max_archive_bytes: u64,
    pub limits: BundleLimits,
}
impl ArchivePackRequest {
    pub fn operation_id(&self) -> OperationId {
        OperationId::new(format!(
            "pack-archive-{}",
            ObjectId::for_bytes(self.request_key.as_str().as_bytes()).hex()
        ))
        .expect("digest is a valid ID")
    }
    pub fn digest(&self) -> Result<ObjectId> {
        if !(1..=1024).contains(&self.max_candidates) {
            return Err(Error::Invalid(
                "archive candidate limit must be 1..=1024".into(),
            ));
        }
        if self.max_tar_bytes == 0
            || self.max_archive_bytes == 0
            || self.max_tar_bytes > i64::MAX as u64
            || self.max_archive_bytes > i64::MAX as u64
            || self.limits.max_bundle_bytes == 0
            || self.limits.max_bundle_bytes as u128 > i64::MAX as u128
            || self.root.size_bytes > self.limits.tree.max_manifest_bytes as u64
        {
            return Err(Error::TooLarge);
        }
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(&(ARCHIVE_PACK_SCHEMA, self))
                .map_err(|_| Error::Invalid("archive request encoding".into()))?,
        ))
    }
    pub fn check_output(&self, content: &ContentRef) -> Result<()> {
        self.digest()?;
        if content.id.algorithm() != self.algorithm {
            return Err(Error::MissingDigest);
        }
        if content.size_bytes > self.max_archive_bytes
            || (self.profile == ArchiveProfile::TarV1 && content.size_bytes > self.max_tar_bytes)
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedArchive {
    pub content: ContentRef,
    pub target: SourceLocator,
    pub stage_name: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePackPlan {
    pub schema: String,
    pub request: ArchivePackRequest,
    pub prepared: Option<PreparedArchive>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePackReceipt {
    pub operation_id: OperationId,
    pub input: ContentRef,
    pub profile: ArchiveProfile,
    pub output: ReplicaRef,
    pub reused: bool,
    pub completed_at_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePackOperation {
    pub operation_id: OperationId,
    pub state: OperationState,
    pub plan: ArchivePackPlan,
    pub receipt: Option<ArchivePackReceipt>,
    pub failure: Option<FailureCode>,
}
pub enum BeginArchivePack {
    Started(Box<ArchivePackOperation>),
    Replay(Box<ArchivePackReceipt>),
}
