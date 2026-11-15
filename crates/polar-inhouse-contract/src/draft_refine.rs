//! Isolated draft editing, without chat ingress, tools or conversation memory.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRefineRequest {
    pub owner: String,
    pub thread: String,
    pub operation_id: String,
    pub base_revision: u32,
    pub original: String,
    pub instruction: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRefineResult {
    pub operation_id: String,
    pub base_revision: u32,
    pub text: String,
}
