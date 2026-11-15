//! Shared authoring intent. Audit construction and mutation rules belong to PromptService.
use super::{
    InhousePromptLayer, PromptAssetKey, PromptBindingRecord, PromptLifecycle, PromptTarget,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptAssetInput {
    pub asset_id: String,
    pub revision: NonZeroU64,
    pub title: String,
    pub body_text: String,
}

/// Editable binding intent. Audit information belongs to the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptBindingSpec {
    pub binding_id: String,
    pub target: PromptTarget,
    pub asset: PromptAssetKey,
    #[serde(with = "crate::prompt_lifecycle_wire")]
    #[schemars(with = "PromptLifecycle")]
    pub lifecycle: PromptLifecycle,
    pub backend_driver: Option<String>,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub prompt_layer: InhousePromptLayer,
    pub sort_order: i64,
    pub replaces_binding_id: Option<String>,
    #[serde(default)]
    pub required_tool_ids: Vec<String>,
    pub enabled: bool,
}

impl From<&PromptBindingRecord> for PromptBindingSpec {
    fn from(record: &PromptBindingRecord) -> Self {
        Self {
            binding_id: record.binding_id.clone(),
            target: record.target(),
            asset: record.asset_key(),
            lifecycle: record.lifecycle,
            backend_driver: record.backend_driver.clone(),
            model_provider: record.model_provider.clone(),
            model_id: record.model_id.clone(),
            prompt_layer: record.prompt_layer,
            sort_order: record.sort_order,
            replaces_binding_id: record.replaces_binding_id.clone(),
            required_tool_ids: record.required_tool_ids.clone(),
            enabled: record.enabled,
        }
    }
}

/// Submitted IDs are authoritative; omitted IDs are untouched. Deletion is
/// explicit and repeatable. There is no installation-vs-management policy flag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PromptChange {
    pub assets: Vec<PromptAssetInput>,
    pub bindings: Vec<PromptBindingSpec>,
    pub delete_binding_ids: Vec<String>,
    pub delete_assets: Vec<PromptAssetKey>,
}

/// Immutable instructions supplied by a native release or exact plugin package.
/// Reuses authoring types; this is compile input, not a database mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagedPromptMaterial {
    pub source_id: String,
    pub prompts: PromptChange,
}
