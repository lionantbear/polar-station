//! Backend-neutral migration identity and ordering.
//!
//! SQL text and database-driver execution deliberately remain in each backend
//! runtime. This crate only describes the stable identity of a migration.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const MIGRATION_METADATA_TABLE: &str = "_polar_schema_migrations";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    Sqlite,
    Postgres,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionMode {
    Required,
    Forbidden,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MigrationDescriptor {
    pub backend: StorageBackend,
    pub component: String,
    pub version: u32,
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub transaction_mode: TransactionMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationPlanError {
    ZeroVersion {
        component: String,
    },
    Duplicate {
        component: String,
        version: u32,
    },
    NonContiguous {
        component: String,
        expected: u32,
        actual: u32,
    },
}

impl fmt::Display for MigrationPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroVersion { component } => {
                write!(f, "migration component {component} uses version zero")
            }
            Self::Duplicate { component, version } => {
                write!(f, "duplicate migration {component} version {version}")
            }
            Self::NonContiguous {
                component,
                expected,
                actual,
            } => write!(
                f,
                "migration component {component} expected version {expected}, found {actual}"
            ),
        }
    }
}

impl Error for MigrationPlanError {}

pub fn validate_order(descriptors: &[MigrationDescriptor]) -> Result<(), MigrationPlanError> {
    let mut keys = BTreeSet::new();
    let mut ordered = descriptors.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|item| (item.backend as u8, item.component.as_str(), item.version));

    let mut previous: Option<(StorageBackend, &str, u32)> = None;
    for descriptor in ordered {
        if descriptor.version == 0 {
            return Err(MigrationPlanError::ZeroVersion {
                component: descriptor.component.clone(),
            });
        }
        let key = (
            descriptor.backend as u8,
            descriptor.component.as_str(),
            descriptor.version,
        );
        if !keys.insert(key) {
            return Err(MigrationPlanError::Duplicate {
                component: descriptor.component.clone(),
                version: descriptor.version,
            });
        }
        let expected = match previous {
            Some((backend, component, version))
                if backend == descriptor.backend && component == descriptor.component =>
            {
                version + 1
            }
            _ => 1,
        };
        if descriptor.version != expected {
            return Err(MigrationPlanError::NonContiguous {
                component: descriptor.component.clone(),
                expected,
                actual: descriptor.version,
            });
        }
        previous = Some((
            descriptor.backend,
            descriptor.component.as_str(),
            descriptor.version,
        ));
    }
    Ok(())
}


