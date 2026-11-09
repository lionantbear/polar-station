use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ResourceError, ResourceErrorCode};

pub const RESOURCE_CATALOG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(Uuid);

impl ResourceId {
    pub fn new() -> Self {
        Self(polar_identity::generation::new_uuid_v4())
    }
}

impl Default for ResourceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ResourceId {
    type Err = ResourceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self).map_err(|_| {
            ResourceError::new(
                ResourceErrorCode::ConfigInvalid,
                "Resource ID is not a canonical UUID.",
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceTargetKind {
    Local,
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMappingSpec {
    pub id: ResourceId,
    pub name: String,
    pub enabled: bool,
    pub execution_target_id: String,
    pub remote_path: String,
    pub local_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForwardSpec {
    pub id: ResourceId,
    pub name: String,
    pub enabled: bool,
    pub execution_target_id: String,
    pub bind_address: String,
    pub allow_non_loopback: bool,
    pub local_port_start: u16,
    pub local_port_end: u16,
    pub remote_host: String,
    pub remote_port_start: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidMirrorSpec {
    pub id: ResourceId,
    pub name: String,
    pub enabled: bool,
    pub execution_target_id: String,
    pub bind_address: String,
    pub allow_non_loopback: bool,
    pub local_adb_port: u16,
    pub remote_adb_host: String,
    pub remote_adb_port: u16,
    pub scrcpy_arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCatalog {
    pub schema_version: u32,
    pub revision: u64,
    pub file_mappings: Vec<FileMappingSpec>,
    pub port_forwards: Vec<PortForwardSpec>,
    pub android_mirrors: Vec<AndroidMirrorSpec>,
}

impl ResourceCatalog {
    pub fn empty() -> Self {
        Self {
            schema_version: RESOURCE_CATALOG_SCHEMA_VERSION,
            revision: 0,
            file_mappings: Vec::new(),
            port_forwards: Vec::new(),
            android_mirrors: Vec::new(),
        }
    }
}

impl Default for ResourceCatalog {
    fn default() -> Self {
        Self::empty()
    }
}
