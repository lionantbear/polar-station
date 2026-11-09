//! Shared IDEA-CAVE query contracts. Indexes are derived, bounded views.
use crate::NodeRef;
use polar_cas::ContentRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    #[default]
    Current,
    History,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PropertyFilter {
    pub path: String,
    pub value: Value,
    #[serde(default)]
    pub member: bool,
}
fn page_size() -> usize {
    20
}
/// Ordering on an indexed scalar property; ties use the stable NodeRef key.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PropertyOrder {
    pub path: String,
    #[serde(default)]
    pub descending: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindInput {
    /// A bounded OR of UUIDs, intersected with all other query restrictions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uuids: Vec<String>,
    /// JSON Pointers to return from the selected index generation, without
    /// opening source documents. Missing fields are omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub select: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<PropertyOrder>,
    /// Restrict current membership to this exact I-tree revision and its fixed
    /// composition closure. Does not select a branch or grant content access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::documents::optional_content_ref_schema")]
    pub within_revision: Option<ContentRef>,
    /// Select only typed document roots at this branch's current heads, without
    /// expanding their composition. Requires node_type and Current scope;
    /// mutually exclusive with within_revision. Bound to the paging snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_branch: Option<String>,
    /// Canonical Polar Node ID, or "me" resolved by the bound runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub document: Option<String>,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    #[serde(default)]
    pub scope: SearchScope,
    #[serde(default = "page_size")]
    pub limit: usize,
    #[serde(default)]
    pub cursor: Option<String>,
}
impl Default for FindInput {
    fn default() -> Self {
        Self {
            uuids: vec![],
            select: vec![],
            order: None,
            within_revision: None,
            root_branch: None,
            writer: None,
            text: String::new(),
            uuid: None,
            node_type: None,
            document: None,
            properties: vec![],
            scope: SearchScope::Current,
            limit: page_size(),
            cursor: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservedHead {
    pub subject: String,
    pub branch: String,
    pub revision: ContentRef,
    pub document: ContentRef,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverage {
    pub documents: usize,
    pub nodes: usize,
    pub heads: Vec<ObservedHead>,
    pub unresolved: Vec<NodeRef>,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMatch {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub projection: std::collections::BTreeMap<String, Value>,
    pub reference: NodeRef,
    pub title: String,
    pub snippet: String,
    pub matched_fields: Vec<String>,
    pub current: bool,
    pub score: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindReport {
    pub snapshot: ContentRef,
    pub coverage: Coverage,
    pub matches: Vec<NodeMatch>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebuildInput {
    #[serde(default)]
    #[schemars(schema_with = "content_refs_schema")]
    pub documents: Vec<ContentRef>,
    #[serde(default)]
    #[schemars(schema_with = "crate::documents::optional_content_ref_schema")]
    pub manifest: Option<ContentRef>,
    #[serde(default = "yes")]
    pub include_history: bool,
}
impl Default for RebuildInput {
    fn default() -> Self {
        Self {
            documents: vec![],
            manifest: None,
            include_history: true,
        }
    }
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexManifest {
    pub schema: String,
    pub documents: Vec<ContentRef>,
    /// Durable version facts, selected together with the query generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<ContentRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RebuildReport {
    pub snapshot: ContentRef,
    pub documents: usize,
    pub nodes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferencesInput {
    #[serde(default = "one")]
    pub depth: usize,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub target: Option<NodeRef>,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub scope: SearchScope,
    #[serde(default = "page_size")]
    pub limit: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceMatch {
    /// Intermediate sources on the path toward the requested target; empty for a direct edge.
    pub via: Vec<NodeRef>,
    pub source: NodeRef,
    pub slot: String,
    pub relation: String,
    pub target: crate::Reference,
    pub unresolved: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferencesReport {
    pub snapshot: ContentRef,
    pub coverage: Coverage,
    pub matches: Vec<ReferenceMatch>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    pub subject: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default = "page_size")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub revision: ContentRef,
    pub subject: String,
    pub state: crate::history::RevisionState,
    pub parents: Vec<ContentRef>,
    pub height: u64,
    pub head_branches: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryReport {
    pub entries: Vec<HistoryEntry>,
    pub heads: Vec<ObservedHead>,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookupInput {
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LookupReport {
    pub content: Option<ContentRef>,
    pub references: Vec<ReferenceMatch>,
    pub truncated: bool,
    pub digest: String,
    pub documents: Vec<ContentRef>,
    pub nodes: Vec<NodeRef>,
    pub revisions: Vec<HistoryEntry>,
}

fn content_refs_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    serde_json::from_value(serde_json::json!({"type":"array","items":crate::documents::content_ref_schema(gen),"maxItems":10000})).expect("static ContentRef list schema")
}

fn one() -> usize {
    1
}

/// Exact identity relations in the library bound to the service.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentitySelector {
    Contents {
        uuid: String,
    },
    Objects {
        #[schemars(with = "String")]
        hash: polar_cas::ObjectId,
    },
    Definitions {
        uuid: String,
    },
    References {
        uuid: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityQueryInput {
    pub query: IdentitySelector,
    /// Previous report snapshot hash; reject a changed generation during paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_snapshot: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "page_size")]
    pub limit: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityQueryReport {
    Contents {
        snapshot: ContentRef,
        exists: bool,
        total: usize,
        hashes: Vec<polar_cas::ObjectId>,
    },
    Objects {
        snapshot: ContentRef,
        total: usize,
        uuids: Vec<String>,
    },
    Definitions {
        snapshot: ContentRef,
        total: usize,
        nodes: Vec<NodeRef>,
    },
    References {
        snapshot: ContentRef,
        total: usize,
        nodes: Vec<NodeRef>,
    },
}
