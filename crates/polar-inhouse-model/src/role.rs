//! Role identity and the single Role association owned by the Inhouse directory.
//! Capability assignments and prompt bindings are separate domain records.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InhouseRoleRecord {
    pub role_id: String,
    pub display_name: String,
    pub enabled: bool,
}

/// Absence of this row means no Role. The store must enforce unique Polar IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InhousePolarRoleBinding {
    pub inhouse_polar_id: String,
    pub role_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleContractError {
    EmptyField(&'static str),
    DuplicateRole(String),
    MultipleRolesForPolar(String),
    UnknownRole(String),
}

impl fmt::Display for RoleContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "Role contract requires {field}"),
            Self::DuplicateRole(id) => write!(f, "duplicate Role: {id}"),
            Self::MultipleRolesForPolar(id) => write!(f, "multiple Role bindings for Polar: {id}"),
            Self::UnknownRole(id) => write!(f, "unknown Role: {id}"),
        }
    }
}

impl std::error::Error for RoleContractError {}

impl InhouseRoleRecord {
    pub fn validate(&self) -> Result<(), RoleContractError> {
        required(&self.role_id, "role_id")?;
        required(&self.display_name, "display_name")
    }
}

impl InhousePolarRoleBinding {
    pub fn validate(&self) -> Result<(), RoleContractError> {
        required(&self.inhouse_polar_id, "inhouse_polar_id")?;
        required(&self.role_id, "role_id")
    }
}

/// Structural check for a complete Role catalog; does not load storage or expand tools.
pub fn validate_role_catalog(
    roles: &[InhouseRoleRecord],
    bindings: &[InhousePolarRoleBinding],
) -> Result<(), RoleContractError> {
    let mut ids = HashSet::new();
    for role in roles {
        role.validate()?;
        if !ids.insert(&role.role_id) {
            return Err(RoleContractError::DuplicateRole(role.role_id.clone()));
        }
    }
    let mut polars = HashSet::new();
    for binding in bindings {
        binding.validate()?;
        if !polars.insert(&binding.inhouse_polar_id) {
            return Err(RoleContractError::MultipleRolesForPolar(
                binding.inhouse_polar_id.clone(),
            ));
        }
    }
    for binding in bindings {
        if !ids.contains(&binding.role_id) {
            return Err(RoleContractError::UnknownRole(binding.role_id.clone()));
        }
    }
    Ok(())
}

fn required(value: &str, field: &'static str) -> Result<(), RoleContractError> {
    if value.trim().is_empty() {
        return Err(RoleContractError::EmptyField(field));
    }
    Ok(())
}
