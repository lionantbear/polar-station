use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::model::{WorkspaceCatalog, WORKSPACE_SCHEMA_VERSION};
use crate::validation::validate_catalog;

pub fn encode_catalog_json(catalog: &WorkspaceCatalog) -> Result<Vec<u8>, WorkspaceError> {
    validate_catalog(catalog)?;
    serde_json::to_vec_pretty(catalog).map_err(|error| {
        WorkspaceError::new(
            WorkspaceErrorCode::InvalidCatalog,
            format!("failed to encode Workspace catalog: {error}"),
        )
    })
}

pub fn decode_catalog_json(bytes: &[u8]) -> Result<WorkspaceCatalog, WorkspaceError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        WorkspaceError::new(
            WorkspaceErrorCode::InvalidCatalog,
            format!("failed to decode Workspace catalog: {error}"),
        )
    })?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorCode::InvalidCatalog,
                "Workspace catalog has no schema_version",
            )
        })?;
    if schema_version != u64::from(WORKSPACE_SCHEMA_VERSION) {
        return Err(WorkspaceError::new(
            WorkspaceErrorCode::SchemaUnsupported,
            format!("unsupported Workspace schema version {schema_version}"),
        ));
    }
    let mut catalog: WorkspaceCatalog = serde_json::from_value(value).map_err(|error| {
        WorkspaceError::new(
            WorkspaceErrorCode::InvalidCatalog,
            format!("failed to decode Workspace catalog: {error}"),
        )
    })?;
    for workspace in &mut catalog.workspaces {
        for tab in &mut workspace.tabs {
            if tab.visual_pane_ids.is_empty() && !tab.panes.is_empty() {
                tab.visual_pane_ids = tab.panes.iter().map(|pane| pane.id).collect();
            }
        }
    }
    validate_catalog(&catalog)?;
    Ok(catalog)
}
