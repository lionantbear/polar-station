use polar_identity::UnixTimestamp;
use serde::{Deserialize, Serialize};

use crate::InhousePolarSystemRole;

mod input;
pub use input::*;

mod records;
pub use records::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum PromptLifecycle {
    ThreadInit,
    TurnAugment,
}

impl PromptLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreadInit => "thread_init",
            Self::TurnAugment => "turn_augment",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "thread_init" => Some(Self::ThreadInit),
            "turn_augment" => Some(Self::TurnAugment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptAssetClass {
    BaseIdentity,
    ManagementSemantics,
    ChatBridgeGuide,
    SystemOps,
    PromptAuthor,
    BackendOverrideCodex,
    BackendOverridePi,
    BackendOverrideAdam,
}

impl PromptAssetClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BaseIdentity => "base_identity",
            Self::ManagementSemantics => "management_semantics",
            Self::ChatBridgeGuide => "chat_bridge_guide",
            Self::SystemOps => "system_ops",
            Self::PromptAuthor => "prompt_author",
            Self::BackendOverrideCodex => "backend_override_codex",
            Self::BackendOverridePi => "backend_override_pi",
            Self::BackendOverrideAdam => "backend_override_adam",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "base_identity" => Some(Self::BaseIdentity),
            "management_semantics" => Some(Self::ManagementSemantics),
            "chat_bridge_guide" => Some(Self::ChatBridgeGuide),
            "system_ops" => Some(Self::SystemOps),
            "prompt_author" => Some(Self::PromptAuthor),
            "backend_override_codex" => Some(Self::BackendOverrideCodex),
            "backend_override_pi" => Some(Self::BackendOverridePi),
            "backend_override_adam" => Some(Self::BackendOverrideAdam),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptBackend {
    Codex,
    Pi,
    Adam,
}

impl PromptBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Adam => "adam",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            "adam" => Some(Self::Adam),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssetRecord {
    pub asset_id: String,
    pub title: String,
    pub lifecycle: PromptLifecycle,
    pub asset_class: PromptAssetClass,
    pub body_text: String,
    pub target_inhouse_polar_id: Option<String>,
    pub target_system_role: Option<InhousePolarSystemRole>,
    pub target_management_class: Option<String>,
    pub target_channel_id: Option<String>,
    pub target_thread_id: Option<String>,
    pub target_backend: Option<PromptBackend>,
    pub updated_by_inhouse_polar_id: String,
    pub updated_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptActivationRecord {
    pub lifecycle: PromptLifecycle,
    pub asset_class: PromptAssetClass,
    pub target_inhouse_polar_id: Option<String>,
    pub target_system_role: Option<InhousePolarSystemRole>,
    pub target_management_class: Option<String>,
    pub target_channel_id: Option<String>,
    pub target_thread_id: Option<String>,
    pub target_backend: Option<PromptBackend>,
    pub asset_id: String,
    pub updated_by_inhouse_polar_id: String,
    pub updated_at: UnixTimestamp,
}


