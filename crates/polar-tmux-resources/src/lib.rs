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

mod file_access;
pub use file_access::{
    image_path, plan_file_access, resolve_file, resolve_reference_path, FileAccess,
};

mod file_path;
mod file_reference;
pub use file_path::{PathFlavor, PathForm, SourcePath};
mod file_continuation;
mod file_input;
mod file_text;
pub use file_input::{
    DisplayRowBoundary, FileAccessPlan, FileOpenFailure, FileOpenResolution, FileOpenStage,
    FileReferenceInput, ParsedFileReference, ResolvedFile, ResolvedFileSource, TextRange,
};
pub use file_reference::{
    file_link_rules, parse_file_reference, parse_selected_file_reference, select_opener,
    FileLocation, FileReference, FileTypeRule, OpenStrategy,
};
pub use file_text::{parse_file_input, parse_reference_range, parse_source_reference};

mod file_open;
pub use file_open::{FileOpenEffect, FileOpenFlow, FileOpenInput, FileOrigin};

mod reference_source;
pub use reference_source::{
    complete_source_file_reference, source_file_references, source_file_search_terms,
};

mod file_candidates;
pub use file_candidates::{
    click_file_candidates, file_open_candidates, FileClickContext, FILE_CONTEXT_PREFIX,
    MAX_FILE_CANDIDATES, MAX_FILE_CANDIDATE_BYTES,
};
