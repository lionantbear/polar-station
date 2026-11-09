use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ResourceCatalog, ResourceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    FileMapping,
    PortForward,
    AndroidMirror,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRuntimeState {
    Disabled,
    Stopped,
    WaitingForSsh,
    Starting,
    Running,
    Recovering,
    NeedsAttention,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRuntimeStatus {
    pub state: ResourceRuntimeState,
    pub generation: u64,
    pub message: String,
    pub can_restart: bool,
    pub can_open_mirror: bool,
}

impl ResourceRuntimeStatus {
    pub fn disabled() -> Self {
        Self {
            state: ResourceRuntimeState::Disabled,
            generation: 0,
            message: "Disabled".to_owned(),
            can_restart: false,
            can_open_mirror: false,
        }
    }

    pub fn stopped() -> Self {
        Self {
            state: ResourceRuntimeState::Stopped,
            generation: 0,
            message: "Ready to start".to_owned(),
            can_restart: true,
            can_open_mirror: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceEntryProjection {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub name: String,
    pub enabled: bool,
    pub execution_target_id: String,
    pub status: ResourceRuntimeStatus,
    pub can_reveal_local_path: bool,
    pub can_open_log: bool,
    pub action_disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcesControlProjection {
    pub schema_version: u32,
    pub revision: u64,
    pub catalog: ResourceCatalog,
    pub entries: Vec<ResourceEntryProjection>,
    pub available_target_ids: Vec<String>,
    pub has_pending_draft: bool,
    pub apply_label: String,
    pub aggregate_state: ResourceRuntimeState,
    pub aggregate_message: String,
}

pub fn project_resources(
    catalog: &ResourceCatalog,
    statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
) -> ResourcesControlProjection {
    project_resources_with_targets(catalog, statuses, Vec::new())
}

pub fn project_resources_with_targets(
    catalog: &ResourceCatalog,
    statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
    mut available_target_ids: Vec<String>,
) -> ResourcesControlProjection {
    available_target_ids.sort();
    let mut entries = Vec::new();
    for mapping in &catalog.file_mappings {
        let status = status(mapping.id, mapping.enabled, statuses);
        entries.push(ResourceEntryProjection {
            id: mapping.id,
            kind: ResourceKind::FileMapping,
            name: mapping.name.clone(),
            enabled: mapping.enabled,
            execution_target_id: mapping.execution_target_id.clone(),
            can_reveal_local_path: mapping.enabled && status.state == ResourceRuntimeState::Running,
            can_open_log: mapping.enabled,
            action_disabled_reason: action_disabled_reason(mapping.enabled, &status),
            status,
        });
    }
    for forward in &catalog.port_forwards {
        let status = status(forward.id, forward.enabled, statuses);
        entries.push(ResourceEntryProjection {
            id: forward.id,
            kind: ResourceKind::PortForward,
            name: forward.name.clone(),
            enabled: forward.enabled,
            execution_target_id: forward.execution_target_id.clone(),
            can_reveal_local_path: false,
            can_open_log: forward.enabled,
            action_disabled_reason: action_disabled_reason(forward.enabled, &status),
            status,
        });
    }
    for mirror in &catalog.android_mirrors {
        let status = status(mirror.id, mirror.enabled, statuses);
        entries.push(ResourceEntryProjection {
            id: mirror.id,
            kind: ResourceKind::AndroidMirror,
            name: mirror.name.clone(),
            enabled: mirror.enabled,
            execution_target_id: mirror.execution_target_id.clone(),
            can_reveal_local_path: false,
            can_open_log: mirror.enabled,
            action_disabled_reason: action_disabled_reason(mirror.enabled, &status),
            status,
        });
    }
    let aggregate_state = aggregate_state(&entries);
    let aggregate_message = match aggregate_state {
        ResourceRuntimeState::Disabled => "No Resources are enabled.",
        ResourceRuntimeState::Stopped => "Resources are stopped.",
        ResourceRuntimeState::WaitingForSsh => "Resources are waiting for SSH.",
        ResourceRuntimeState::Starting => "Resources are starting.",
        ResourceRuntimeState::Running => "Resources are running.",
        ResourceRuntimeState::Recovering => "Resources are recovering.",
        ResourceRuntimeState::NeedsAttention => "Resources need attention.",
        ResourceRuntimeState::Stopping => "Resources are stopping.",
    }
    .to_owned();
    ResourcesControlProjection {
        schema_version: catalog.schema_version,
        revision: catalog.revision,
        catalog: catalog.clone(),
        entries,
        available_target_ids,
        has_pending_draft: false,
        apply_label: "Apply".to_owned(),
        aggregate_state,
        aggregate_message,
    }
}

fn action_disabled_reason(enabled: bool, status: &ResourceRuntimeStatus) -> Option<String> {
    if !enabled {
        Some("Apply an enabled Resource before using runtime actions.".to_owned())
    } else if status.state == ResourceRuntimeState::WaitingForSsh {
        Some("Waiting for the shared SSH connection.".to_owned())
    } else {
        None
    }
}

fn aggregate_state(entries: &[ResourceEntryProjection]) -> ResourceRuntimeState {
    if entries.is_empty() || entries.iter().all(|entry| !entry.enabled) {
        return ResourceRuntimeState::Disabled;
    }
    for state in [
        ResourceRuntimeState::NeedsAttention,
        ResourceRuntimeState::Recovering,
        ResourceRuntimeState::Starting,
        ResourceRuntimeState::WaitingForSsh,
        ResourceRuntimeState::Stopping,
        ResourceRuntimeState::Running,
        ResourceRuntimeState::Stopped,
    ] {
        if entries
            .iter()
            .any(|entry| entry.enabled && entry.status.state == state)
        {
            return state;
        }
    }
    ResourceRuntimeState::Disabled
}

fn status(
    id: ResourceId,
    enabled: bool,
    statuses: &BTreeMap<ResourceId, ResourceRuntimeStatus>,
) -> ResourceRuntimeStatus {
    statuses.get(&id).cloned().unwrap_or_else(|| {
        if enabled {
            ResourceRuntimeStatus::stopped()
        } else {
            ResourceRuntimeStatus::disabled()
        }
    })
}


