//! Host-local active backing identity, NOT a CAS source/admission witness.
//! Timestamps and byte lengths deliberately do not identify mutable directories.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInode {
    pub device: u64,
    pub inode: u64,
    pub owner: u32,
    pub mode: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStorageRecord {
    pub schema: String,
    pub root: WorkspaceInode,
    pub upper: WorkspaceInode,
    pub work: WorkspaceInode,
    pub lease: WorkspaceInode,
}
