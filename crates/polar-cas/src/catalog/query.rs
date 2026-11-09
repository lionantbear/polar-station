//! Read-only observations. Pages are not verification witnesses or work queues.
use crate::{
    manifest::TreeEntry,
    operation::{FailureCode, OperationKind, OperationState},
    projection::ProjectionProfile,
    source::{LocationId, OperationId, RequestKey, SourceLocator},
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};

/// A partial derived index, not a full manifest/digest proof. Child content is
/// still a claim and may have no local or remote replica.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntryPage {
    pub manifest: ContentRef,
    pub entry_count: u64,
    pub entries: Vec<TreeEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExportScope {
    Input(ObjectId),
    Destination(LocationId),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    Manifest,
    LocalTree,
    Oci,
    Iroh,
}

/// Stable historical completion order. A cursor is a key, not a row handle or
/// a cross-page snapshot. Reuse it with the same query scope.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportCursor {
    pub completed_at_ms: u64,
    pub export_id: OperationId,
}
impl ExportCursor {
    pub fn validate(&self) -> Result<()> {
        if self.completed_at_ms > i64::MAX as u64 {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

/// Historical publication facts, including disabled destinations and expired
/// links. No current availability/authorization or automatic renewal is implied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRecord {
    pub export_id: OperationId,
    pub input: ContentRef,
    pub kind: ExportKind,
    pub destination: LocationId,
    pub locator: SourceLocator,
    pub profile: Option<ProjectionProfile>,
    pub links_expire_at_ms: Option<u64>,
    pub completed_at_ms: u64,
}
impl ExportRecord {
    pub fn cursor(&self) -> ExportCursor {
        ExportCursor {
            completed_at_ms: self.completed_at_ms,
            export_id: self.export_id.clone(),
        }
    }
}

/// Candidate for explicit inspection/recovery, not a guarantee of available
/// input or a released execution guard. Does not load full plans or receipts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub operation_id: OperationId,
    pub request_key: RequestKey,
    pub kind: OperationKind,
    pub state: OperationState,
    pub failure: Option<FailureCode>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
