//! Credential-free trusted-operator publication contract. Host owns authority.
use crate::{NodeRef, TreeFormat};
use polar_cas::{source::RequestKey, store_index::StoreRef, ContentRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosurePublishInput {
    pub root: NodeRef,
    pub format: TreeFormat,
    pub store: StoreRef,
    pub request: RequestKey,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedClosure {
    pub manifest: ContentRef,
    pub root: NodeRef,
    pub store: StoreRef,
    pub index_root: ContentRef,
    pub index_generation: u64,
    pub copied_objects: usize,
    pub copied_bytes: u64,
    pub reused_objects: usize,
    pub packed_files_not_copied: usize,
}

/// Safe public failures contain no provider diagnostics or credential material.
#[derive(Debug, thiserror::Error)]
pub enum ClosurePublishError {
    #[error("closure publication is unsupported")]
    Unsupported,
    #[error("closure publication is unavailable")]
    Unavailable,
    #[error("invalid closure publication parameters")]
    Invalid,
    #[error("closure publication conflict; review the request")]
    Conflict,
    #[error("closure publication failed")]
    Failed,
}
