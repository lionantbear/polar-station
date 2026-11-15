//! Exact prompt asset versions and direct bindings. This module validates one
//! record at a time; storage integrity and compilation have separate owners.

use std::{collections::HashSet, fmt, num::NonZeroU64};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::PromptLifecycle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InhousePromptLayer {
    System,
    Developer,
    ToolUsageProtocol,
    UserProjectContext,
    TurnAugment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptContractError {
    EmptyField(&'static str),
    InvalidDigest,
    ContentDigestMismatch,
    NegativeTimestamp(&'static str),
    UnexpectedTargetOwner,
    ModelProviderRequired,
    SelfReplacement(String),
    DuplicateRequiredTool(String),
    TurnAugmentInSession(String),
}

impl fmt::Display for PromptContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "prompt contract requires {field}"),
            Self::InvalidDigest => write!(
                f,
                "prompt digest must be sha256 followed by 64 lowercase hex digits"
            ),
            Self::ContentDigestMismatch => write!(f, "prompt content does not match its digest"),
            Self::NegativeTimestamp(field) => {
                write!(f, "prompt {field} must be nonnegative Unix seconds")
            }
            Self::UnexpectedTargetOwner => {
                write!(f, "only channel/thread prompt targets have an owner")
            }
            Self::ModelProviderRequired => write!(f, "prompt model_id requires model_provider"),
            Self::SelfReplacement(id) => write!(f, "prompt binding cannot replace itself: {id}"),
            Self::DuplicateRequiredTool(id) => write!(f, "duplicate required prompt tool: {id}"),
            Self::TurnAugmentInSession(id) => {
                write!(f, "thread_init binding cannot use turn_augment layer: {id}")
            }
        }
    }
}

impl std::error::Error for PromptContractError {}

fn required(value: &str, field: &'static str) -> Result<(), PromptContractError> {
    if value.trim().is_empty() {
        return Err(PromptContractError::EmptyField(field));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), PromptContractError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(PromptContractError::InvalidDigest);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(PromptContractError::InvalidDigest);
    }
    Ok(())
}

fn timestamp(value: i64, field: &'static str) -> Result<(), PromptContractError> {
    if value < 0 {
        return Err(PromptContractError::NegativeTimestamp(field));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAssetRef {
    pub asset_id: String,
    pub revision: NonZeroU64,
    pub digest: String,
}

impl PromptAssetRef {
    pub fn validate(&self) -> Result<(), PromptContractError> {
        required(&self.asset_id, "asset_id")?;
        validate_digest(&self.digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAssetRevision {
    pub asset_id: String,
    pub revision: NonZeroU64,
    pub title: String,
    pub body_text: String,
    pub digest: String,
    pub created_by: String,
    /// Unix seconds, supplied by the creating use-case rather than this model.
    pub created_at: i64,
}

impl PromptAssetRevision {
    pub fn new(
        asset_id: impl Into<String>,
        revision: NonZeroU64,
        title: impl Into<String>,
        body_text: impl Into<String>,
        created_by: impl Into<String>,
        created_at: i64,
    ) -> Result<Self, PromptContractError> {
        let mut asset = Self {
            asset_id: asset_id.into(),
            revision,
            title: title.into(),
            body_text: body_text.into(),
            digest: String::new(),
            created_by: created_by.into(),
            created_at,
        };
        asset.digest = asset.content_digest();
        asset.validate()?;
        Ok(asset)
    }

    /// JSON tuple field order is part of the digest contract; never hash a map.
    fn content_digest(&self) -> String {
        let bytes =
            serde_json::to_vec(&(&self.asset_id, self.revision, &self.title, &self.body_text))
                .expect("strings and nonzero integer serialize to JSON");
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    pub fn reference(&self) -> PromptAssetRef {
        PromptAssetRef {
            asset_id: self.asset_id.clone(),
            revision: self.revision,
            digest: self.digest.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), PromptContractError> {
        self.reference().validate()?;
        required(&self.title, "title")?;
        required(&self.body_text, "body_text")?;
        required(&self.created_by, "created_by")?;
        timestamp(self.created_at, "created_at")?;
        if self.digest != self.content_digest() {
            return Err(PromptContractError::ContentDigestMismatch);
        }
        Ok(())
    }
}

/// Base prompts are source-owned and intentionally have no assignment target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PromptBindingTargetKind {
    Role,
    InhousePolar,
    Channel,
    Thread,
    Tool,
    ToolFamily,
    Gear,
}

/// Exact lookup key, not a latest-version selector.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PromptAssetKey {
    pub asset_id: String,
    pub revision: NonZeroU64,
}

/// Channel and thread identities keep their own owner, independently.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PromptTarget {
    pub kind: PromptBindingTargetKind,
    pub owner_id: Option<String>,
    pub id: String,
}

impl PromptTarget {
    pub fn new(
        kind: PromptBindingTargetKind,
        owner_id: Option<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            owner_id,
            id: id.into(),
        }
    }

    pub fn validate(&self) -> Result<(), PromptContractError> {
        required(&self.id, "target_id")?;
        let needs_owner = matches!(
            self.kind,
            PromptBindingTargetKind::Channel | PromptBindingTargetKind::Thread
        );
        match (&self.owner_id, needs_owner) {
            (Some(owner), true) => required(owner, "target_owner_id"),
            (None, true) => Err(PromptContractError::EmptyField("target_owner_id")),
            (Some(_), false) => Err(PromptContractError::UnexpectedTargetOwner),
            (None, false) => Ok(()),
        }
    }
}

/// Direct reference plus applicability/assembly conditions, never a saved composition.
/// Call `validate()` after decoding and before use. It does not resolve references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBindingRecord {
    pub binding_id: String,
    pub target_kind: PromptBindingTargetKind,
    pub target_owner_id: Option<String>,
    pub target_id: String,
    pub asset_id: String,
    pub asset_revision: NonZeroU64,
    #[serde(with = "lifecycle_wire")]
    pub lifecycle: PromptLifecycle,
    pub backend_driver: Option<String>,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub prompt_layer: InhousePromptLayer,
    pub sort_order: i64,
    pub replaces_binding_id: Option<String>,
    /// SQLite encodes this typed list in required_tool_ids_json.
    #[serde(default)]
    pub required_tool_ids: Vec<String>,
    pub enabled: bool,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_by: String,
    /// Unix seconds.
    pub updated_at: i64,
}

impl PromptBindingRecord {
    pub fn target(&self) -> PromptTarget {
        PromptTarget::new(
            self.target_kind,
            self.target_owner_id.clone(),
            self.target_id.clone(),
        )
    }

    pub fn asset_key(&self) -> PromptAssetKey {
        PromptAssetKey {
            asset_id: self.asset_id.clone(),
            revision: self.asset_revision,
        }
    }

    pub fn validate(&self) -> Result<(), PromptContractError> {
        for (field, value) in [
            ("binding_id", self.binding_id.as_str()),
            ("target_id", self.target_id.as_str()),
            ("asset_id", self.asset_id.as_str()),
            ("created_by", self.created_by.as_str()),
            ("updated_by", self.updated_by.as_str()),
        ] {
            required(value, field)?;
        }
        for (field, value) in [
            ("target_owner_id", &self.target_owner_id),
            ("backend_driver", &self.backend_driver),
            ("model_provider", &self.model_provider),
            ("model_id", &self.model_id),
            ("replaces_binding_id", &self.replaces_binding_id),
        ] {
            if let Some(value) = value {
                required(value, field)?;
            }
        }
        let needs_owner = matches!(
            self.target_kind,
            PromptBindingTargetKind::Channel | PromptBindingTargetKind::Thread
        );
        match (needs_owner, self.target_owner_id.is_some()) {
            (true, false) => return Err(PromptContractError::EmptyField("target_owner_id")),
            (false, true) => {
                return Err(PromptContractError::UnexpectedTargetOwner);
            }
            _ => {}
        }
        if self.model_id.is_some() && self.model_provider.is_none() {
            return Err(PromptContractError::ModelProviderRequired);
        }
        if self.replaces_binding_id.as_ref() == Some(&self.binding_id) {
            return Err(PromptContractError::SelfReplacement(
                self.binding_id.clone(),
            ));
        }
        if self.lifecycle == PromptLifecycle::ThreadInit
            && self.prompt_layer == InhousePromptLayer::TurnAugment
        {
            return Err(PromptContractError::TurnAugmentInSession(
                self.binding_id.clone(),
            ));
        }
        let mut ids = HashSet::new();
        for id in &self.required_tool_ids {
            required(id, "required_tool_id")?;
            if !ids.insert(id) {
                return Err(PromptContractError::DuplicateRequiredTool(id.clone()));
            }
        }
        timestamp(self.created_at, "created_at")?;
        timestamp(self.updated_at, "updated_at")
    }
}

/// New direct bindings use the existing lifecycle's canonical storage names.
/// This is not an old-format alias or a second lifecycle type.
/// Canonical lifecycle encoding shared by records and mutation inputs.
pub mod lifecycle_wire {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    use super::PromptLifecycle;

    pub fn serialize<S: Serializer>(
        value: &PromptLifecycle,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PromptLifecycle, D::Error> {
        let value = String::deserialize(deserializer)?;
        PromptLifecycle::parse(&value)
            .ok_or_else(|| D::Error::custom("expected thread_init or turn_augment"))
    }
}
