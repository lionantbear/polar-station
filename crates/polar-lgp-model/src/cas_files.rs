//! Human/Agent file operations. Identity and storage bindings are host resources.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn page_size() -> u32 {
    30
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CasFindInput {
    /// Filename/digest; may be omitted only when writer is supplied.
    #[serde(default)]
    pub query: String,
    /// A Polar Node ID, or "me" for the immutable host-bound CAS writer.
    #[serde(default)]
    pub writer: Option<String>,
    #[serde(default)]
    pub thread: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default = "page_size")]
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CasFileInput {
    /// Filename, content digest, or the exact file selection from cas_find.
    pub file: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CasFileInfo {
    /// Visible successful-save facts, not authorship or proof of availability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<polar_cas::catalog::writers::WriterRecord>,
    pub file: String,
    pub name: Option<String>,
    pub digest: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
    pub source: String,
    pub thread: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CasFindResult {
    pub files: Vec<CasFileInfo>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CasOpenResult {
    pub info: CasFileInfo,
    pub preview: serde_json::Value,
}

/// Receipt for an explicitly requested attachment message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CasSendResult {
    pub info: CasFileInfo,
    pub attachment_ref: String,
    pub message: crate::LgpMessageKey,
    pub append_status: crate::MessageAppendStatus,
}


