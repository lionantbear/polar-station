//! Revision-bound Resource draft shared by native Settings surfaces.
//! Persistence, validation and lifecycle remain in the existing catalog service.
use serde::{Deserialize, Serialize};

use crate::{
    AndroidMirrorSpec, FileMappingSpec, PortForwardSpec, ResourceCatalog, ResourceId, ResourceKind,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileMappingPrefill {
    pub request_id: String,
    pub execution_target_id: String,
    pub remote_directory: String,
    pub source_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceEditorInput {
    Snapshot {
        catalog: ResourceCatalog,
    },
    Change {
        catalog: ResourceCatalog,
    },
    Add {
        kind: ResourceKind,
        execution_target_id: String,
    },
    Remove {
        id: ResourceId,
    },
    Prefill {
        prefill: FileMappingPrefill,
    },
    Apply,
    Reload,
    Close,
    Discard,
    KeepEditing,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceEditorConfirmation {
    Reload,
    Close,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceEditorEffect {
    Apply { catalog: ResourceCatalog },
    Reload,
    Close,
}

#[derive(Debug, Serialize)]
pub struct ResourceEditorView {
    pub catalog: ResourceCatalog,
    pub loaded: bool,
    pub dirty: bool,
    pub can_apply: bool,
    pub selected: Option<ResourceId>,
    pub confirmation: Option<ResourceEditorConfirmation>,
    pub confirmation_title: &'static str,
    pub discard_label: &'static str,
    pub keep_label: &'static str,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct ResourceEditor {
    baseline: Option<ResourceCatalog>,
    latest: Option<ResourceCatalog>,
    draft: ResourceCatalog,
    selected: Option<ResourceId>,
    confirmation: Option<ResourceEditorConfirmation>,
    prefill: Option<FileMappingPrefill>,
    consumed_prefill: Option<FileMappingPrefill>,
    submitted: Option<ResourceCatalog>,
    message: Option<String>,
}

fn same_configuration(a: &ResourceCatalog, b: &ResourceCatalog) -> bool {
    a.schema_version == b.schema_version
        && a.file_mappings == b.file_mappings
        && a.port_forwards == b.port_forwards
        && a.android_mirrors == b.android_mirrors
}

impl ResourceEditor {
    fn dirty(&self) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|base| !same_configuration(base, &self.draft))
    }
    pub fn view(&self) -> ResourceEditorView {
        ResourceEditorView {
            catalog: self.draft.clone(),
            loaded: self.baseline.is_some(),
            dirty: self.dirty(),
            can_apply: self.submitted.is_none()
                && self.dirty()
                && self
                    .latest
                    .as_ref()
                    .is_some_and(|latest| latest.revision == self.draft.revision),
            selected: self.selected,
            confirmation: self.confirmation,
            message: self.message.clone(),
            confirmation_title: "Discard unapplied Resource changes?",
            discard_label: match self.confirmation {
                Some(ResourceEditorConfirmation::Close) => "Discard and Close",
                _ => "Discard and Reload",
            },
            keep_label: "Keep Editing",
        }
    }
    fn snapshot(&mut self, catalog: ResourceCatalog) {
        if self
            .latest
            .as_ref()
            .is_some_and(|latest| latest.revision > catalog.revision)
        {
            return;
        }
        if self
            .submitted
            .as_ref()
            .is_some_and(|sent| same_configuration(sent, &catalog))
        {
            self.draft.revision = catalog.revision;
            self.baseline = Some(catalog.clone());
            self.submitted = None;
            self.message = None;
        } else if self.baseline.is_none()
            || !self.dirty()
            || same_configuration(&self.draft, &catalog)
        {
            self.draft = catalog.clone();
            self.baseline = Some(catalog.clone());
            self.message = None;
        } else if self.draft.revision != catalog.revision {
            self.message = Some(
                "The Host catalog changed. This draft is preserved; Reload discards it.".into(),
            );
        }
        self.latest = Some(catalog);
        if let Some(prefill) = self.prefill.take() {
            self.prefill(prefill);
        }
    }
    fn prefill(&mut self, prefill: FileMappingPrefill) {
        if self.baseline.is_none() {
            self.prefill = Some(prefill);
            return;
        }
        if let Some(mapping) = self.draft.file_mappings.iter().find(|mapping| {
            mapping.execution_target_id == prefill.execution_target_id
                && mapping.remote_path == prefill.remote_directory
        }) {
            self.selected = Some(mapping.id);
        } else if self.consumed_prefill.as_ref() != Some(&prefill) {
            let id = ResourceId::new();
            self.draft.file_mappings.push(FileMappingSpec {
                id,
                name: "File Mapping".into(),
                enabled: true,
                execution_target_id: prefill.execution_target_id.clone(),
                remote_path: prefill.remote_directory.clone(),
                local_path: format!("~/Polar/remote/{id}"),
            });
            self.selected = Some(id);
        }
        self.consumed_prefill = Some(prefill);
    }
    fn add(&mut self, kind: ResourceKind, target: String) {
        if self.baseline.is_none() {
            return;
        }
        let id = ResourceId::new();
        match kind {
            ResourceKind::FileMapping => self.draft.file_mappings.push(FileMappingSpec {
                id,
                name: "File Mapping".into(),
                enabled: false,
                execution_target_id: target,
                remote_path: "/home/operator".into(),
                local_path: format!("~/Polar/remote/{id}"),
            }),
            ResourceKind::PortForward => self.draft.port_forwards.push(PortForwardSpec {
                id,
                name: "Port Forward".into(),
                enabled: false,
                execution_target_id: target,
                bind_address: "127.0.0.1".into(),
                allow_non_loopback: false,
                local_port_start: 8080,
                local_port_end: 8080,
                remote_host: "127.0.0.1".into(),
                remote_port_start: 8080,
            }),
            ResourceKind::AndroidMirror => self.draft.android_mirrors.push(AndroidMirrorSpec {
                id,
                name: "Android Mirror".into(),
                enabled: false,
                execution_target_id: target,
                bind_address: "127.0.0.1".into(),
                allow_non_loopback: false,
                local_adb_port: 5555,
                remote_adb_host: "127.0.0.1".into(),
                remote_adb_port: 5555,
                scrcpy_arguments: vec![],
            }),
        }
        self.selected = Some(id);
    }
    pub fn dispatch(
        &mut self,
        input: ResourceEditorInput,
    ) -> (ResourceEditorView, Vec<ResourceEditorEffect>) {
        let mut effects = vec![];
        match input {
            ResourceEditorInput::Snapshot { catalog } => self.snapshot(catalog),
            ResourceEditorInput::Change { mut catalog } => {
                if let Some(base) = &self.baseline {
                    catalog.revision = base.revision;
                    catalog.schema_version = base.schema_version;
                    self.draft = catalog;
                }
            }
            ResourceEditorInput::Add {
                kind,
                execution_target_id,
            } => self.add(kind, execution_target_id),
            ResourceEditorInput::Remove { id } => {
                self.draft.file_mappings.retain(|v| v.id != id);
                self.draft.port_forwards.retain(|v| v.id != id);
                self.draft.android_mirrors.retain(|v| v.id != id);
                if self.selected == Some(id) {
                    self.selected = None;
                }
            }
            ResourceEditorInput::Prefill { prefill } => self.prefill(prefill),
            ResourceEditorInput::Apply => {
                if self.view().can_apply {
                    self.submitted = Some(self.draft.clone());
                    effects.push(ResourceEditorEffect::Apply {
                        catalog: self.draft.clone(),
                    });
                }
            }
            ResourceEditorInput::Reload | ResourceEditorInput::Close => {
                let reload = matches!(input, ResourceEditorInput::Reload);
                if self.dirty() {
                    self.confirmation = Some(if reload {
                        ResourceEditorConfirmation::Reload
                    } else {
                        ResourceEditorConfirmation::Close
                    });
                } else {
                    effects.push(if reload {
                        ResourceEditorEffect::Reload
                    } else {
                        ResourceEditorEffect::Close
                    });
                }
            }
            ResourceEditorInput::Discard => {
                if let Some(action) = self.confirmation.take() {
                    self.submitted = None;
                    if let Some(latest) = self.latest.clone() {
                        self.draft = latest.clone();
                        self.baseline = Some(latest);
                        self.message = None;
                    }
                    effects.push(match action {
                        ResourceEditorConfirmation::Reload => ResourceEditorEffect::Reload,
                        ResourceEditorConfirmation::Close => ResourceEditorEffect::Close,
                    });
                }
            }
            ResourceEditorInput::KeepEditing => self.confirmation = None,
            ResourceEditorInput::Failed { message } => {
                self.submitted = None;
                self.message = Some(message);
            }
        }
        (self.view(), effects)
    }
}


