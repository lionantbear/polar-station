use polar_lgp_model::{
    InhousePolarSystemRole, ManagementActorClassRecord, ManagementChannelPolicyRecord,
    ManagementConnectorPolicyRecord, ManagementThreadPolicyRecord, ManagementToolPolicyRecord,
    ManagementTransportPolicyRecord, PromptActivationRecord, PromptAssetClass, PromptAssetRecord,
    PromptBackend, PromptLifecycle,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptToolRef {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSupplementalSection {
    pub asset_class: PromptAssetClass,
    pub title: String,
    pub body_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCompileContext {
    /// Product-provided session material, composed independently of role assets.
    #[serde(default)]
    pub session_prompt_blocks: Vec<polar_inhouse_contract::InhousePromptBlock>,
    pub inhouse_polar_id: String,
    pub system_role: Option<InhousePolarSystemRole>,
    pub management_class: Option<String>,
    pub channel_id: Option<String>,
    pub thread_id: Option<String>,
    pub user_project_context: Option<String>,
    pub backend: PromptBackend,
    pub available_dynamic_tools: Vec<PromptToolRef>,
    pub supplemental_turn_sections: Vec<PromptSupplementalSection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptSemanticLayer {
    BaseIdentity,
    DeveloperContract,
    ToolUsageProtocol,
    UserProjectContext,
    TurnAugment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCandidatePatchTargetLayer {
    BaseIdentity,
    DeveloperContract,
    ToolUsageProtocol,
    UserProjectContext,
    TurnAugment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCandidatePatchOperation {
    ReplaceBody,
    AppendBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCandidatePatch {
    pub asset_id: String,
    pub target_layer: PromptCandidatePatchTargetLayer,
    pub operation: PromptCandidatePatchOperation,
    pub body_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOptimizationCandidate {
    pub candidate_id: String,
    pub patches: Vec<PromptCandidatePatch>,
    pub rationale: String,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptCandidateResponse {
    pub candidates: Vec<PromptOptimizationCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCandidateValidationTarget {
    SystemAlphaBuilder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCandidateValidationScope {
    pub expected_target: PromptCandidateValidationTarget,
    pub require_bundle_authoring_protocol: bool,
    pub forbid_activation_attempts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledPromptSection {
    pub asset_class: PromptAssetClass,
    pub asset_id: Option<String>,
    pub semantic_layer: PromptSemanticLayer,
    pub title: String,
    pub body_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompiledThreadPromptIr {
    pub base_identity: Vec<CompiledPromptSection>,
    pub developer_contract: Vec<CompiledPromptSection>,
    pub tool_usage_protocol: Vec<CompiledPromptSection>,
    pub user_project_context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompiledTurnPromptIr {
    pub tool_usage_protocol: Vec<CompiledPromptSection>,
    pub turn_augment: Vec<CompiledPromptSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledThreadPromptBundle {
    pub sections: Vec<CompiledPromptSection>,
    pub ir: CompiledThreadPromptIr,
    pub combined_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledTurnPromptBundle {
    pub sections: Vec<CompiledPromptSection>,
    pub ir: CompiledTurnPromptIr,
    pub combined_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexThreadPromptRender {
    pub prompt_override_enabled: bool,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub user_instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTurnPromptRender {
    pub developer_instructions: Option<String>,
    pub input_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamThreadPromptRender {
    pub prompt_override_enabled: bool,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub user_project_context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamTurnPromptRender {
    pub developer_instructions: Option<String>,
    pub input_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiThreadPromptRender {
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiTurnPromptRender {
    pub input_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolarBundleEnvelope {
    pub payload_type: String,
    pub schema_version: String,
    pub bundle: PolarBundle,
}

impl Default for PolarBundleEnvelope {
    fn default() -> Self {
        Self {
            payload_type: "polar_bundle".to_string(),
            schema_version: "2026-04-25".to_string(),
            bundle: PolarBundle::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolarBundle {
    pub management_policy_patch: ManagementPolicyPatch,
    pub prompt_package: PromptPackage,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagementPolicyPatch {
    pub transport: Option<ManagementTransportPolicyRecord>,
    pub actor_classes: Vec<ManagementActorClassRecord>,
    pub tool_policies: Vec<ManagementToolPolicyRecord>,
    pub channels: Vec<ManagementChannelPolicyRecord>,
    pub threads: Vec<ManagementThreadPolicyRecord>,
    pub connectors: Vec<ManagementConnectorPolicyRecord>,
}

impl From<polar_lgp_model::ManagementPolicyRecordSet> for ManagementPolicyPatch {
    fn from(value: polar_lgp_model::ManagementPolicyRecordSet) -> Self {
        Self {
            transport: value.transport,
            actor_classes: value.actor_classes,
            tool_policies: value.tool_policies,
            channels: value.channels,
            threads: value.threads,
            connectors: value.connectors,
        }
    }
}

impl From<&polar_lgp_model::ManagementPolicyRecordSet> for ManagementPolicyPatch {
    fn from(value: &polar_lgp_model::ManagementPolicyRecordSet) -> Self {
        Self {
            transport: value.transport.clone(),
            actor_classes: value.actor_classes.clone(),
            tool_policies: value.tool_policies.clone(),
            channels: value.channels.clone(),
            threads: value.threads.clone(),
            connectors: value.connectors.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompileTarget {
    pub target_id: String,
    pub lifecycle: PromptLifecycle,
    pub backend: PromptBackend,
    pub inhouse_polar_id: Option<String>,
    pub system_role: Option<InhousePolarSystemRole>,
    pub management_class: Option<String>,
    pub channel_id: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptDynamicTurnSectionSource {
    DynamicTools,
    SupplementalTurnSection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDynamicTurnSection {
    pub section_id: String,
    pub source: PromptDynamicTurnSectionSource,
    pub asset_class: PromptAssetClass,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRenderExpectation {
    pub lifecycle: PromptLifecycle,
    pub backend: PromptBackend,
    pub field_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptPackageAuthoritySource {
    ManagementPolicyPatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPackageValidation {
    pub authority_source: PromptPackageAuthoritySource,
    pub require_asset_references: bool,
    pub require_lifecycle_class_match: bool,
    pub require_target_consistency: bool,
    pub require_backend_override_target_match: bool,
    pub require_management_class_alignment: bool,
}

impl Default for PromptPackageValidation {
    fn default() -> Self {
        Self {
            authority_source: PromptPackageAuthoritySource::ManagementPolicyPatch,
            require_asset_references: true,
            require_lifecycle_class_match: true,
            require_target_consistency: true,
            require_backend_override_target_match: true,
            require_management_class_alignment: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptPackage {
    pub assets: Vec<PromptAssetRecord>,
    pub activations: Vec<PromptActivationRecord>,
    pub compile_targets: Vec<PromptCompileTarget>,
    pub dynamic_turn_sections: Vec<PromptDynamicTurnSection>,
    pub render_expectations: Vec<PromptRenderExpectation>,
    pub validation: PromptPackageValidation,
}

impl PromptPackage {
    pub fn from_records(
        assets: Vec<PromptAssetRecord>,
        activations: Vec<PromptActivationRecord>,
    ) -> Self {
        Self {
            assets,
            activations,
            ..Self::default()
        }
    }
}
