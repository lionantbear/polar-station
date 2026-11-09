//! Document use-case contracts. Persistence is separate from domain acceptance.
use crate::{history::RevisionReceipt, NodeRef};
use polar_cas::ContentRef;
use schemars::{gen::SchemaGenerator, schema::Schema, JsonSchema};
use serde::{Deserialize, Serialize};

pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Complete only an omitted root identity before using the existing strict
/// parser. Duplicate keys, explicit null UUIDs and invalid structure still fail.
pub fn prepare_document(
    bytes: &[u8],
    fallback: uuid::Uuid,
    limits: &crate::Limits,
) -> crate::Result<crate::IdeaTree> {
    let mut value = crate::itree::strict_value(bytes, limits)?;
    let root = value
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| crate::invalid("root", "root name required"))?
        .to_owned();
    let node = value
        .get_mut("nodes")
        .and_then(|nodes| nodes.get_mut(&root))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| crate::invalid("root", "missing root"))?;
    node.entry("uuid")
        .or_insert_with(|| serde_json::json!(fallback));
    crate::parse_idea(&serde_json::to_vec(&value)?, limits)
}

/// Schema for the existing CAS ContentRef wire representation, not another ID.
pub fn content_ref_schema(_: &mut SchemaGenerator) -> Schema {
    serde_json::from_value(
        serde_json::json!({"type":"object","additionalProperties":false,
        "required":["id","size_bytes"],"properties":{
            "id":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"},
            "size_bytes":{"type":"integer","minimum":0}
        }}),
    )
    .expect("static ContentRef schema")
}
pub fn optional_content_ref_schema(gen: &mut SchemaGenerator) -> Schema {
    serde_json::from_value(serde_json::json!({"anyOf":[content_ref_schema(gen),{"type":"null"}]}))
        .expect("static optional ContentRef schema")
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSaveInput {
    /// Existing CAS filename, digest or file selector returned by cas_find.
    pub file: String,
    #[serde(default = "main_branch")]
    pub branch: String,
    /// Expected current head of this instance's target branch; absent for a new branch.
    #[serde(default)]
    #[schemars(schema_with = "optional_content_ref_schema")]
    pub base: Option<ContentRef>,
    /// Explicit parent for a new branch. Mutually exclusive with `base`; this
    /// does not adopt another instance's branch head or grant CAS access.
    #[serde(default)]
    #[schemars(schema_with = "optional_content_ref_schema")]
    pub fork_from: Option<ContentRef>,
}
fn main_branch() -> String {
    "main".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentReadInput {
    #[schemars(schema_with = "content_ref_schema")]
    pub revision: ContentRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentVersion {
    pub document: NodeRef,
    pub history: RevisionReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentReadReport {
    pub document: NodeRef,
    pub revision: ContentRef,
    pub tree: serde_json::Value,
}
