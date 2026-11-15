use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceErrorCode {
    CatalogStale,
    ConfigInvalid,
    TargetNotFound,
    TargetNotSsh,
    BindConflict,
    MountConflict,
    ResourceNotFound,
    AndroidNotEnabled,
}

impl ResourceErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CatalogStale => "resource_catalog_stale",
            Self::ConfigInvalid => "resource_config_invalid",
            Self::TargetNotFound => "resource_target_not_found",
            Self::TargetNotSsh => "resource_target_not_ssh",
            Self::BindConflict => "resource_bind_conflict",
            Self::MountConflict => "resource_mount_conflict",
            Self::ResourceNotFound => "resource_not_found",
            Self::AndroidNotEnabled => "resource_android_not_enabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceError {
    pub code: ResourceErrorCode,
    pub message: String,
}

impl ResourceError {
    pub fn new(code: ResourceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ResourceError {}
