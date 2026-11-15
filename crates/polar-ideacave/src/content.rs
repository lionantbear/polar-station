//! Content-first tools share these contracts across SDK, Tool and Slash.
use crate::{documents::DocumentVersion, search::PropertyFilter, NodeRef};
use polar_cas::ContentRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_SELECTION_FILES: usize = 100;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const SELECTION_SCHEMA: &str = "polar.ideacave.content-selection.v1";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentSource {
    /// Exact content, without choosing a mutable latest version.
    Content {
        #[schemars(schema_with = "crate::documents::content_ref_schema")]
        content: ContentRef,
    },
    /// Existing filename, digest or file selector. Ambiguous names are errors.
    File { file: String },
    /// Original files on one result page, never their displayed snippets.
    Selection {
        #[schemars(schema_with = "crate::documents::content_ref_schema")]
        selection: ContentRef,
    },
    /// Exact immutable directory tree and a relative file path within it.
    Tree {
        #[schemars(schema_with = "crate::documents::content_ref_schema")]
        tree: ContentRef,
        path: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedFile {
    pub content: ContentRef,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentSelection {
    pub schema: String,
    pub files: Vec<SelectedFile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    #[default]
    Content,
    Files,
}
fn page_size() -> usize {
    20
}
fn output_bytes() -> usize {
    16 * 1024
}
fn max_matches() -> usize {
    50
}
fn first_line() -> usize {
    1
}
fn branch() -> String {
    "main".into()
}
fn document_name() -> String {
    "document.json".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentSearchInput {
    #[serde(default)]
    pub query: String,
    /// content searches indexed nodes; files searches filenames/digests/writers.
    #[serde(default)]
    pub kind: SearchKind,
    #[serde(default)]
    pub writer: Option<String>,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub properties: Vec<PropertyFilter>,
    #[serde(default)]
    pub history: bool,
    #[serde(default = "page_size")]
    pub limit: usize,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentMatch {
    pub file: SelectedFile,
    pub node: Option<NodeRef>,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentSearchReport {
    pub matches: Vec<ContentMatch>,
    pub selection: ContentSource,
    pub next_cursor: Option<String>,
    pub coverage: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentPosition {
    pub file_index: usize,
    pub byte_offset: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatInput {
    pub source: ContentSource,
    #[serde(default)]
    pub position: ContentPosition,
    #[serde(default = "first_line")]
    pub start_line: usize,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default = "output_bytes")]
    pub max_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextPart {
    pub file: SelectedFile,
    pub byte_offset: usize,
    pub line_number: usize,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatReport {
    pub parts: Vec<TextPart>,
    pub next: Option<ContentPosition>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JqInput {
    pub source: ContentSource,
    pub filter: String,
    #[serde(default = "max_matches")]
    pub max_items: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonPart {
    pub file: SelectedFile,
    pub values: Vec<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JqReport {
    pub parts: Vec<JsonPart>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RgInput {
    pub source: ContentSource,
    pub pattern: String,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub fixed_strings: bool,
    #[serde(default)]
    pub context: usize,
    #[serde(default = "max_matches")]
    pub max_matches: usize,
    #[serde(default)]
    pub position: ContentPosition,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RgHit {
    pub part: TextPart,
    pub is_context: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RgReport {
    pub hits: Vec<RgHit>,
    pub selection: ContentSource,
    pub next: Option<ContentPosition>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchTarget {
    /// Immutable bytes. No base means create; a base means update.
    Content {
        #[serde(default)]
        #[schemars(schema_with = "crate::documents::optional_content_ref_schema")]
        base: Option<ContentRef>,
        name: String,
    },
    /// Edit the pretty JSON view of the exact document revision, as jq '.' emits.
    Document {
        #[serde(default)]
        #[schemars(schema_with = "crate::documents::optional_content_ref_schema")]
        base: Option<ContentRef>,
        #[serde(default = "branch")]
        branch: String,
        #[serde(default = "document_name")]
        name: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentPatchInput {
    /// An explicit content/document target. Omit only for a configured workspace.
    #[serde(default)]
    pub target: Option<PatchTarget>,
    /// Full verified apply_patch body, starting with '*** Begin Patch' and ending
    /// with '*** End Patch'. Use bare '@@' update markers. Do not send unified diff
    /// headers such as '---', '+++', or '@@ -1,3 +1,3 @@'. Do not copy '1. ', '2. ',
    /// or other read_file line-number prefixes into patch lines.
    pub patch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentPatchReport {
    pub content: ContentRef,
    pub document: Option<DocumentVersion>,
}
