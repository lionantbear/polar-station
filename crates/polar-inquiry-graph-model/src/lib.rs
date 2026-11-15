//! Polar InquiryGraph shared model.
//!
//! InquiryGraph is the first-class Polar core domain for structured inquiry:
//! a remote or local Polar node offers a verifiable inquiry package, a surface
//! renders the package through a constrained renderer, and the user produces a
//! typed result artifact that can be submitted back through LGP.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

pub const INQUIRY_GRAPH_PACKAGE_SCHEMA_VERSION: &str = "polar.inquiry_graph.package.v1";
pub const INQUIRY_GRAPH_RESULT_SCHEMA_VERSION: &str = "polar.inquiry_graph.result.v1";
pub const INQUIRY_GRAPH_NODE_RESOLUTION_REQUEST_SCHEMA_VERSION: &str =
    "polar.inquiry_graph.node_resolution.request.v1";
pub const INQUIRY_GRAPH_NODE_RESOLUTION_SCHEMA_VERSION: &str =
    "polar.inquiry_graph.node_resolution.v1";
pub const INQUIRY_COMPLETED_RESULT_READ_REQUEST_SCHEMA_V1: &str =
    "polar.inquiry.completed_result.read.request.v1";
pub const INQUIRY_COMPLETED_RESULT_READ_RESPONSE_SCHEMA_V1: &str =
    "polar.inquiry.completed_result.read.response.v1";
pub const INQUIRY_ERROR_SCHEMA_V1: &str = "polar.inquiry.error.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InquiryCompletedResultReadRequestV1 {
    pub schema: String,
    pub request_id: String,
    pub completed_inquiry_session_id: String,
}

impl InquiryCompletedResultReadRequestV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != INQUIRY_COMPLETED_RESULT_READ_REQUEST_SCHEMA_V1 {
            return Err(format!(
                "completed-result request schema must be {INQUIRY_COMPLETED_RESULT_READ_REQUEST_SCHEMA_V1}"
            ));
        }
        ensure_inquiry_standard_non_empty("request_id", &self.request_id)?;
        ensure_inquiry_standard_non_empty(
            "completed_inquiry_session_id",
            &self.completed_inquiry_session_id,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InquiryCompletedResultProvenanceV1 {
    pub package_hash: String,
    pub provider_kind: String,
    pub provider_id: String,
    pub provider_version: String,
    pub result_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InquiryCompletedResultReadResponseV1 {
    pub schema: String,
    pub request_id: String,
    pub result: InquiryGraphResultArtifact,
    pub provenance: InquiryCompletedResultProvenanceV1,
}

impl InquiryCompletedResultReadResponseV1 {
    pub fn canonical_result_sha256(&self) -> Result<String, serde_json::Error> {
        let value = serde_json::to_value(&self.result)?;
        let canonical = canonicalize_inquiry_standard_json(&value);
        let bytes = serde_json::to_vec(&canonical)?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    pub fn validate_for_request(
        &self,
        request: &InquiryCompletedResultReadRequestV1,
    ) -> Result<(), String> {
        if self.schema != INQUIRY_COMPLETED_RESULT_READ_RESPONSE_SCHEMA_V1 {
            return Err(format!(
                "completed-result response schema must be {INQUIRY_COMPLETED_RESULT_READ_RESPONSE_SCHEMA_V1}"
            ));
        }
        if self.request_id != request.request_id {
            return Err("completed-result response request_id does not match request".into());
        }
        if self.result.session_id != request.completed_inquiry_session_id {
            return Err("completed-result session does not match request".into());
        }
        if self.result.schema_version != INQUIRY_GRAPH_RESULT_SCHEMA_VERSION {
            return Err("completed-result uses an unsupported result schema".into());
        }
        let hash = self
            .canonical_result_sha256()
            .map_err(|error| format!("failed to hash completed result: {error}"))?;
        if hash != self.provenance.result_sha256 {
            return Err("completed-result provenance hash does not match result".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryStandardErrorCodeV1 {
    InvalidRequest,
    NotCompleted,
    ResultNotFound,
    AccessDenied,
    SchemaUnsupported,
    ResultInvalid,
    ResolverFailed,
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InquiryStandardErrorV1 {
    pub schema: String,
    pub request_id: String,
    pub code: InquiryStandardErrorCodeV1,
    pub message: String,
    pub retryable: bool,
}

fn ensure_inquiry_standard_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

fn canonicalize_inquiry_standard_json(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(values) => {
            let mut entries = values
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_inquiry_standard_json(value)))
                .collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            JsonValue::Object(entries.into_iter().collect())
        }
        JsonValue::Array(values) => JsonValue::Array(
            values
                .iter()
                .map(canonicalize_inquiry_standard_json)
                .collect(),
        ),
        other => other.clone(),
    }
}

pub mod message_kind {
    pub const PACKAGE_OFFER: &str = "inquiry_graph.package.offer";
    pub const PACKAGE_ACCEPTED: &str = "inquiry_graph.package.accepted";
    pub const PACKAGE_DECLINED: &str = "inquiry_graph.package.declined";
    pub const SESSION_STARTED: &str = "inquiry_graph.session.started";
    pub const SESSION_ANSWER_CHECKPOINT: &str = "inquiry_graph.session.answer_checkpoint";
    pub const SESSION_RESULT: &str = "inquiry_graph.session.result";
    pub const SESSION_CANCELLED: &str = "inquiry_graph.session.cancelled";
    pub const SESSION_ERROR: &str = "inquiry_graph.session.error";
    pub const RECEIPT: &str = "inquiry_graph.receipt";

    pub const ALL: &[&str] = &[
        PACKAGE_OFFER,
        PACKAGE_ACCEPTED,
        PACKAGE_DECLINED,
        SESSION_STARTED,
        SESSION_ANSWER_CHECKPOINT,
        SESSION_RESULT,
        SESSION_CANCELLED,
        SESSION_ERROR,
        RECEIPT,
    ];
}

pub fn is_inquiry_graph_message_kind(value: &str) -> bool {
    let value = value.trim();
    value
        .strip_prefix("inquiry_graph.")
        .map(|suffix| !suffix.trim().is_empty())
        .unwrap_or(false)
}

pub mod package_kind {
    pub const QUESTIONNAIRE: &str = "questionnaire";
    pub const FAMILIAR_CALIBRATION: &str = "familiar_calibration";
    pub const MEMORY_REVIEW: &str = "memory_review";
    pub const KNOWLEDGE_INTAKE: &str = "knowledge_intake";
    pub const PREFERENCE_UPDATE: &str = "preference_update";
    pub const AGENT_SETUP: &str = "agent_setup";
    pub const DIAGNOSTIC: &str = "diagnostic";
    pub const CREATIVE_BRIEF: &str = "creative_brief";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphPackage {
    pub manifest: InquiryGraphPackageManifest,
    #[serde(default)]
    pub nodes: Vec<InquiryGraphNode>,
    #[serde(default)]
    pub edges: Vec<InquiryGraphEdge>,
    #[serde(default)]
    pub result_schema: Option<JsonValue>,
    #[serde(default)]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphPackageRef {
    pub package_id: String,
    pub package_version: String,
}

impl InquiryGraphPackageRef {
    pub fn from_package(package: &InquiryGraphPackage) -> Self {
        Self {
            package_id: package.manifest.package_id.clone(),
            package_version: package.manifest.package_version.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphPackageManifest {
    pub package_id: String,
    pub package_version: String,
    pub package_kind: String,
    pub schema_version: String,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub entry_node_id: String,
    #[serde(default)]
    pub required_capabilities: Vec<InquiryGraphCapability>,
    #[serde(default)]
    pub privacy: InquiryGraphPrivacyPolicy,
    #[serde(default)]
    pub result_target: Option<InquiryGraphResultTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphPrivacyPolicy {
    pub sensitivity: String,
    pub raw_answer_retention: String,
    pub memory_write_policy: String,
    pub allow_local_draft: bool,
}

impl Default for InquiryGraphPrivacyPolicy {
    fn default() -> Self {
        Self {
            sensitivity: "personal".to_string(),
            raw_answer_retention: "remote_optional".to_string(),
            memory_write_policy: "requires_user_confirmation".to_string(),
            allow_local_draft: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphResultTarget {
    pub owner: String,
    pub thread: String,
    #[serde(default)]
    pub access_plane: Option<String>,
    #[serde(default)]
    pub message_kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphCapability {
    Section,
    BinaryChoice,
    PairwisePreference,
    MaxDiff,
    ScenarioChoice,
    ResponseAb,
    ReviewConfirm,
    CardSort,
    ShortText,
    LongText,
    Consent,
    ResultReview,
    IdentityCapture,
}

impl InquiryGraphCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::BinaryChoice => "binary_choice",
            Self::PairwisePreference => "pairwise_preference",
            Self::MaxDiff => "maxdiff",
            Self::ScenarioChoice => "scenario_choice",
            Self::ResponseAb => "response_ab",
            Self::ReviewConfirm => "review_confirm",
            Self::CardSort => "card_sort",
            Self::ShortText => "short_text",
            Self::LongText => "long_text",
            Self::Consent => "consent",
            Self::ResultReview => "result_review",
            Self::IdentityCapture => "identity_capture",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphNode {
    pub node_id: String,
    pub kind: InquiryGraphNodeKind,
    pub title: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub choices: Vec<InquiryGraphChoice>,
    #[serde(default)]
    pub fields: Vec<InquiryGraphField>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub allow_unknown: bool,
    #[serde(default)]
    pub skip_allowed: bool,
    #[serde(default)]
    pub emits: Vec<InquiryGraphPatchTarget>,
    /// Optional, package-declared dynamic data hydration. The topology remains
    /// static: a resolver may only fill the declared choice slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<InquiryGraphNodeResolutionSpec>,
    #[serde(default)]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphNodeResolutionExecution {
    LocalDeterministic,
    #[default]
    RemoteRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionSpec {
    pub kind: String,
    pub trigger: String,
    pub resolver_id: String,
    /// Declares whether a conforming client can resolve the node locally or must
    /// invoke the authority-bound provider path. Historical plugin packages ran
    /// through the provider path, so an omitted legacy value defaults to
    /// `remote_required`.
    #[serde(default)]
    pub execution: InquiryGraphNodeResolutionExecution,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Dependencies that are included when answered but do not block entry.
    /// This permits static graph packages to model bounded adaptive branches.
    #[serde(default)]
    pub optional_depends_on: Vec<String>,
    pub output: InquiryGraphNodeResolutionOutputSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionOutputSpec {
    pub kind: String,
    #[serde(default)]
    pub choice_slots: Vec<InquiryGraphDynamicChoiceSlot>,
    pub min_choices: usize,
    pub max_choices: usize,
    pub result_namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphDynamicChoiceSlot {
    pub choice_id: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphNodeKind {
    Section,
    BinaryChoice,
    PairwisePreference,
    MaxDiff,
    ScenarioChoice,
    ResponseAb,
    ReviewConfirm,
    CardSort,
    ShortText,
    LongText,
    Consent,
    ResultReview,
    IdentityCapture,
}

impl InquiryGraphNodeKind {
    pub fn required_capability(self) -> InquiryGraphCapability {
        match self {
            Self::Section => InquiryGraphCapability::Section,
            Self::BinaryChoice => InquiryGraphCapability::BinaryChoice,
            Self::PairwisePreference => InquiryGraphCapability::PairwisePreference,
            Self::MaxDiff => InquiryGraphCapability::MaxDiff,
            Self::ScenarioChoice => InquiryGraphCapability::ScenarioChoice,
            Self::ResponseAb => InquiryGraphCapability::ResponseAb,
            Self::ReviewConfirm => InquiryGraphCapability::ReviewConfirm,
            Self::CardSort => InquiryGraphCapability::CardSort,
            Self::ShortText => InquiryGraphCapability::ShortText,
            Self::LongText => InquiryGraphCapability::LongText,
            Self::Consent => InquiryGraphCapability::Consent,
            Self::ResultReview => InquiryGraphCapability::ResultReview,
            Self::IdentityCapture => InquiryGraphCapability::IdentityCapture,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphChoice {
    pub choice_id: String,
    pub label: String,
    #[serde(default)]
    pub value: JsonValue,
    #[serde(default)]
    pub is_unknown: bool,
    #[serde(default)]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphField {
    pub field_id: String,
    pub label: String,
    #[serde(default)]
    pub input_kind: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub max_length: Option<u32>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub placeholder_template: Option<String>,
    #[serde(default)]
    pub value_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphPatchTarget {
    PromptProfile,
    MemoryCandidate,
    KnowledgeSeed,
    ToolPreference,
    StylePreference,
    AgentBehavior,
    ConsentPolicy,
    ActivationIdentity,
    ProvisioningRequiredFields,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphEdge {
    pub edge_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    #[serde(default)]
    pub condition: InquiryGraphEdgeCondition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InquiryGraphEdgeCondition {
    #[default]
    Always,
    AnswerEquals {
        node_id: String,
        choice_id: String,
    },
    AnswerIn {
        node_id: String,
        choice_ids: Vec<String>,
    },
    Unknown {
        node_id: String,
    },
    JsonLogic {
        expression: JsonValue,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphSessionState {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub package_id: String,
    pub package_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<InquiryGraphPackageProviderRef>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub interaction_mode: InquiryGraphSessionInteractionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<InquiryGraphSessionSource>,
    pub entry_node_id: String,
    #[serde(default)]
    pub current_node_id: Option<String>,
    pub status: InquiryGraphSessionStatus,
    #[serde(default)]
    pub answers: Vec<InquiryGraphAnswer>,
    /// Active immutable resolution references keyed by node id. Full request
    /// and response artifacts live in the InquiryGraph store.
    #[serde(default)]
    pub node_resolution_refs: BTreeMap<String, InquiryGraphNodeResolutionRef>,
    /// Idempotency receipts for client answer operations.
    #[serde(default)]
    pub processed_answer_operations: BTreeMap<String, u64>,
    /// Canonical payload hashes keyed by client operation id. Reusing an
    /// operation id with a different answer payload is a conflict.
    #[serde(default)]
    pub processed_answer_operation_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub visited_node_ids: Vec<String>,
    pub started_at_secs: i64,
    pub updated_at_secs: i64,
    #[serde(default)]
    pub result_target: Option<InquiryGraphResultTarget>,
    #[serde(default)]
    pub consent: InquiryGraphConsentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_client_submission_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// Immutable provider provenance for the package snapshot used by a session.
pub enum InquiryGraphPackageProviderRef {
    Plugin {
        plugin_id: String,
        plugin_version: String,
        provider_export: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphSessionInteractionMode {
    #[default]
    InteractiveStepwise,
    ExternalDeferredSubmit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InquiryGraphSessionSource {
    Product {
        product_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_instance_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_subject: Option<String>,
    },
    LgpOffer {
        owner: String,
        thread: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_key: Option<String>,
    },
    Connector {
        thread_handle: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        familiar_service_id: String,
        surface_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_instance_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_subject: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphSessionStatus {
    Active,
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphConsentState {
    pub allow_remote_processing: bool,
    pub allow_memory_write: String,
    pub allow_raw_answer_retention: bool,
}

impl Default for InquiryGraphConsentState {
    fn default() -> Self {
        Self {
            allow_remote_processing: true,
            allow_memory_write: "requires_review".to_string(),
            allow_raw_answer_retention: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphAnswer {
    pub node_id: String,
    pub answer_kind: InquiryGraphAnswerKind,
    #[serde(default)]
    pub choice_ids: Vec<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub number: Option<f64>,
    #[serde(default)]
    pub unknown: bool,
    pub answered_at_secs: i64,
    #[serde(default)]
    pub metadata: JsonValue,
}

/// Shared by SDK adapters and the Inquiry service; the actor is supplied
/// separately by the authenticated adapter, never in these parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphSessionAnswerByIdInput {
    pub session_id: String,
    pub expected_revision: u64,
    pub client_operation_id: String,
    pub answer: InquiryGraphAnswer,
    pub now_secs: i64,
}

/// One batch against a persisted session. Authentication is supplied by the
/// adapter, never by these parameters; the service owns the revision check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphSessionAnswerBatchByIdInput {
    pub session_id: String,
    pub expected_revision: u64,
    pub client_operation_id: String,
    #[serde(default)]
    pub answers: Vec<InquiryGraphAnswer>,
    pub now_secs: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InquiryGraphCommandError {
    InvalidAnswer,
    RevisionConflict,
    IdempotencyConflict,
}

impl std::fmt::Display for InquiryGraphCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidAnswer => "invalid InquiryGraph answer batch",
            Self::RevisionConflict => {
                "InquiryGraph session revision conflict while applying answer batch"
            }
            Self::IdempotencyConflict => {
                "client_operation_id was already used with a different answer payload"
            }
        })
    }
}
impl std::error::Error for InquiryGraphCommandError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphAnswerKind {
    Choice,
    MultiChoice,
    Text,
    Unknown,
    Skip,
    Consent,
    ReviewDecision,
    IdentityCapture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionProviderRef {
    pub plugin_id: String,
    pub plugin_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionRequest {
    pub schema_version: String,
    pub invocation_id: String,
    pub session_id: String,
    pub session_revision: u64,
    pub package_id: String,
    pub package_version: String,
    pub package_hash: String,
    pub provider: InquiryGraphNodeResolutionProviderRef,
    pub node_id: String,
    pub resolver_id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub optional_depends_on: Vec<String>,
    #[serde(default)]
    pub answers: Vec<InquiryGraphAnswer>,
    #[serde(default)]
    pub prior_resolutions: Vec<InquiryGraphNodeResolutionRef>,
    pub input_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionArtifact {
    pub schema_version: String,
    pub session_id: String,
    pub session_revision: u64,
    pub package_hash: String,
    pub node_id: String,
    pub resolver_id: String,
    pub input_hash: String,
    /// Presentation-only hydration. Resolvers cannot change topology or node kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub metadata: JsonValue,
    #[serde(default)]
    pub choices: Vec<InquiryGraphChoice>,
    #[serde(default)]
    pub result_fragment: JsonValue,
    #[serde(default)]
    pub diagnostics: JsonValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphNodeResolutionStatus {
    Pending,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphNodeResolutionRef {
    pub node_id: String,
    pub resolver_id: String,
    pub input_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_hash: Option<String>,
    pub provider: InquiryGraphNodeResolutionProviderRef,
    pub status: InquiryGraphNodeResolutionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphResolutionState {
    #[default]
    NotRequired,
    Pending,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InquiryGraphResolutionProjection {
    pub state: InquiryGraphResolutionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphSessionProjection {
    pub session_id: String,
    #[serde(default)]
    pub revision: u64,
    pub package_title: String,
    pub package_kind: String,
    #[serde(default)]
    pub current_node: Option<InquiryGraphNode>,
    #[serde(default)]
    pub resolution: InquiryGraphResolutionProjection,
    pub current_step: usize,
    pub estimated_total_steps: usize,
    pub answered_node_count: usize,
    pub progress_percent: u8,
    pub can_go_back: bool,
    pub can_skip: bool,
    pub can_submit: bool,
    pub status: InquiryGraphSessionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphResultArtifact {
    pub schema_version: String,
    pub session_id: String,
    pub package_id: String,
    pub package_version: String,
    pub completed_at_secs: i64,
    #[serde(default)]
    pub answers: Vec<InquiryGraphAnswer>,
    #[serde(default)]
    pub node_resolutions: Vec<InquiryGraphNodeResolutionArtifact>,
    #[serde(default)]
    pub derived_profile: JsonValue,
    #[serde(default)]
    pub calibration_patch: InquiryGraphCalibrationPatch,
    #[serde(default)]
    pub learning_summary: InquiryGraphLearningSummary,
    pub consent: InquiryGraphConsentState,
    #[serde(default)]
    pub result_target: Option<InquiryGraphResultTarget>,
}

pub mod review_message_kind {
    pub const REVIEW_REQUESTED: &str = "inquiry_graph.review.requested";
    pub const REVIEW_DECIDED: &str = "inquiry_graph.review.decided";
    pub const REVIEW_FOLLOW_UP: &str = "inquiry_graph.review.follow_up";

    pub const ALL: &[&str] = &[REVIEW_REQUESTED, REVIEW_DECIDED, REVIEW_FOLLOW_UP];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphReviewStatus {
    Pending,
    Accepted,
    Rejected,
    Dismissed,
    Expired,
    Cancelled,
}

impl InquiryGraphReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Dismissed => "dismissed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphReviewRespondentIdentityMode {
    Anonymous,
    DisplayName,
    RegisteredUser,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InquiryGraphReviewDecisionKind {
    Accept,
    Reject,
    Dismiss,
    RequestFollowUp,
}

impl InquiryGraphReviewDecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Dismiss => "dismiss",
            Self::RequestFollowUp => "request_follow_up",
        }
    }

    pub fn review_status(self) -> InquiryGraphReviewStatus {
        match self {
            Self::Accept => InquiryGraphReviewStatus::Accepted,
            Self::Reject => InquiryGraphReviewStatus::Rejected,
            Self::Dismiss => InquiryGraphReviewStatus::Dismissed,
            Self::RequestFollowUp => InquiryGraphReviewStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphReviewPolicy {
    pub allow_result_summary_visibility: bool,
    pub allow_raw_answer_visibility: bool,
    pub allow_respondent_identity_visibility: bool,
    #[serde(default)]
    pub allowed_decisions: Vec<InquiryGraphReviewDecisionKind>,
    #[serde(default)]
    pub expires_at_secs: Option<i64>,
    #[serde(default)]
    pub product_context: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphReviewDecision {
    pub decision: InquiryGraphReviewDecisionKind,
    pub reviewer_ref: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub accepted_item_ids: Vec<String>,
    pub decided_at_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphReviewRecord {
    pub review_id: String,
    pub package_id: String,
    pub package_version: String,
    pub session_id: String,
    pub result_ref: String,
    pub respondent_identity_mode: InquiryGraphReviewRespondentIdentityMode,
    #[serde(default)]
    pub respondent_ref: Option<String>,
    #[serde(default)]
    pub respondent_source_thread_ref: Option<String>,
    pub reviewer_ref: String,
    pub reviewer_target_thread_ref: String,
    #[serde(default)]
    pub delivery_route_ref: Option<String>,
    #[serde(default)]
    pub delivery_message_ref: Option<String>,
    pub policy: InquiryGraphReviewPolicy,
    pub status: InquiryGraphReviewStatus,
    #[serde(default)]
    pub latest_decision: Option<InquiryGraphReviewDecision>,
    pub created_at_secs: i64,
    #[serde(default)]
    pub delivered_at_secs: Option<i64>,
    #[serde(default)]
    pub decided_at_secs: Option<i64>,
    pub updated_at_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InquiryGraphLearningSummary {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub learned_items: Vec<InquiryGraphLearnedItem>,
    #[serde(default)]
    pub uncertain_items: Vec<InquiryGraphUncertainItem>,
    #[serde(default)]
    pub behavior_changes: Vec<InquiryGraphBehaviorChange>,
    #[serde(default)]
    pub memory_candidates: Vec<InquiryGraphMemoryCandidate>,
    #[serde(default)]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphLearnedItem {
    pub item_id: String,
    pub statement: String,
    #[serde(default)]
    pub evidence_node_ids: Vec<String>,
    #[serde(default)]
    pub proposed_behavior: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub persistence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphUncertainItem {
    pub item_id: String,
    pub statement: String,
    #[serde(default)]
    pub evidence_node_ids: Vec<String>,
    #[serde(default)]
    pub suggested_followup: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphBehaviorChange {
    pub change_id: String,
    pub description: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub requires_user_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InquiryGraphCalibrationPatch {
    #[serde(default)]
    pub prompt_fragments: Vec<InquiryGraphPromptFragment>,
    #[serde(default)]
    pub memory_candidates: Vec<InquiryGraphMemoryCandidate>,
    #[serde(default)]
    pub knowledge_seed_refs: Vec<String>,
    #[serde(default)]
    pub preference_updates: Vec<InquiryGraphPreferenceUpdate>,
    #[serde(default)]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphPromptFragment {
    pub key: String,
    pub text: String,
    pub source_node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphMemoryCandidate {
    pub key: String,
    pub value: String,
    pub source_node_id: String,
    pub confidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryGraphPreferenceUpdate {
    pub key: String,
    pub value: JsonValue,
    pub source_node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryGraphValidationReport {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub capabilities_detected: Vec<InquiryGraphCapability>,
}


