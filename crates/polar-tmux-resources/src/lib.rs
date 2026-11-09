//! Global Polar Station Resource domain.
//!
//! Resources are deliberately independent from Workspace state.

mod command;
mod error;
mod model;
mod projection;
mod reconciliation;
mod validation;

pub use command::apply_catalog;
pub use error::{ResourceError, ResourceErrorCode};
pub use model::{
    AndroidMirrorSpec, FileMappingSpec, PortForwardSpec, ResourceCatalog, ResourceId,
    ResourceTargetKind, RESOURCE_CATALOG_SCHEMA_VERSION,
};
pub use projection::{
    project_resources, project_resources_with_targets, ResourceEntryProjection, ResourceKind,
    ResourceRuntimeState, ResourceRuntimeStatus, ResourcesControlProjection,
};
pub use reconciliation::{
    plan_reconciliation, plan_resource_update, specs, ReconciliationPlan, ResourceSpec,
    ResourceUpdate,
};
pub use validation::validate_catalog;
