//! Results for the metadata-only Tree construction operation.
use crate::{pack::PackReceipt, ContentRef};
use serde::{Deserialize, Serialize};

/// Directory nodes are persisted CAS objects; their leaf file references do not
/// assert availability and do not create payload replicas or a Source record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeCreateReceipt {
    pub tree: ContentRef,
    pub directories: Vec<PackReceipt>,
    pub logical_entry_count: usize,
    pub logical_file_bytes: u64,
}
