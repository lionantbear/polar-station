//! Portable backup identity and immutable manifest/receipt values.
use crate::{CasNamespace, ObjectId, ObjectRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupIdentity {
    pub instance_id: String,
    pub storage_space_id: String,
    pub database_snapshot: ObjectId,
    pub namespace: CasNamespace,
    /// Explicit selected domain coverage, never an implied whole-node claim.
    pub reference_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupObject {
    pub original: ObjectRef,
    pub saved: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub format: String,
    pub identity: BackupIdentity,
    pub objects: Vec<BackupObject>,
}

/// Issued only after all objects and the final manifest are durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupReceipt {
    pub identity: BackupIdentity,
    pub manifest: ObjectRef,
    pub object_count: usize,
}
