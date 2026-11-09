use std::collections::BTreeMap;

use crate::{AndroidMirrorSpec, FileMappingSpec, PortForwardSpec, ResourceCatalog, ResourceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceSpec {
    FileMapping(FileMappingSpec),
    PortForward(PortForwardSpec),
    AndroidMirror(AndroidMirrorSpec),
}

impl ResourceSpec {
    pub fn id(&self) -> ResourceId {
        match self {
            Self::FileMapping(spec) => spec.id,
            Self::PortForward(spec) => spec.id,
            Self::AndroidMirror(spec) => spec.id,
        }
    }

    pub fn target_id(&self) -> &str {
        match self {
            Self::FileMapping(spec) => &spec.execution_target_id,
            Self::PortForward(spec) => &spec.execution_target_id,
            Self::AndroidMirror(spec) => &spec.execution_target_id,
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            Self::FileMapping(spec) => spec.enabled,
            Self::PortForward(spec) => spec.enabled,
            Self::AndroidMirror(spec) => spec.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconciliationPlan {
    /// Exact receipts to stop, in the stable order of the previous catalog.
    pub stop: Vec<ResourceId>,
    /// Exact desired specifications to start, in the stable order of the new catalog.
    pub start: Vec<ResourceSpec>,
}

impl ReconciliationPlan {
    pub fn is_empty(&self) -> bool {
        self.stop.is_empty() && self.start.is_empty()
    }
}

pub fn plan_reconciliation(
    current: &ResourceCatalog,
    desired: &ResourceCatalog,
) -> ReconciliationPlan {
    let current_specs = specs(current);
    let desired_specs = specs(desired);
    let desired_by_id = desired_specs
        .iter()
        .map(|spec| (spec.id(), spec))
        .collect::<BTreeMap<_, _>>();
    let current_by_id = current_specs
        .iter()
        .map(|spec| (spec.id(), spec))
        .collect::<BTreeMap<_, _>>();

    let stop = current_specs
        .iter()
        .filter(|current_spec| {
            current_spec.enabled()
                && desired_by_id
                    .get(&current_spec.id())
                    .is_none_or(|desired_spec| {
                        !desired_spec.enabled() || desired_spec != current_spec
                    })
        })
        .map(ResourceSpec::id)
        .collect();

    let start = desired_specs
        .into_iter()
        .filter(|desired_spec| {
            desired_spec.enabled()
                && current_by_id
                    .get(&desired_spec.id())
                    .is_none_or(|current_spec| {
                        !current_spec.enabled() || *current_spec != desired_spec
                    })
        })
        .collect();

    ReconciliationPlan { stop, start }
}

pub fn specs(catalog: &ResourceCatalog) -> Vec<ResourceSpec> {
    catalog
        .file_mappings
        .iter()
        .cloned()
        .map(ResourceSpec::FileMapping)
        .chain(
            catalog
                .port_forwards
                .iter()
                .cloned()
                .map(ResourceSpec::PortForward),
        )
        .chain(
            catalog
                .android_mirrors
                .iter()
                .cloned()
                .map(ResourceSpec::AndroidMirror),
        )
        .collect()
}

/// Desired-state policy; the runtime retains cleanup receipts on failure and
/// must complete stop before start. No I/O or runtime scheduling belongs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceUpdate {
    pub stop: bool,
    pub start: bool,
}

pub fn plan_resource_update(
    previous: Option<&ResourceSpec>,
    desired: Option<&ResourceSpec>,
    observed_generation: Option<u64>,
    verified_generation: Option<u64>,
    restart: bool,
) -> ResourceUpdate {
    let enabled = desired.is_some_and(ResourceSpec::enabled);
    let stop = observed_generation.is_some()
        && (restart
            || previous != desired
            || !enabled
            || observed_generation != verified_generation);
    ResourceUpdate {
        stop,
        start: enabled && verified_generation.is_some() && (stop || observed_generation.is_none()),
    }
}
