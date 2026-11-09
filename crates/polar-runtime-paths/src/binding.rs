use crate::{CasPaths, InstancePathPlan, RuntimeDirectoryLayout};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Persisted identity of one managed runtime, independent of config filename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceBinding {
    pub instance_id: String,
    pub storage: RuntimeDataBinding,
}

impl RuntimeInstanceBinding {
    pub fn validate_config_path(&self, config: impl AsRef<Path>) -> Result<InstancePathPlan> {
        let plan = self.storage.instance_plan(&self.instance_id)?;
        anyhow::ensure!(
            plan.config_file() == config.as_ref(),
            "runtime configuration path does not match its explicit instance binding"
        );
        Ok(plan)
    }
}

/// Validated input for the coordinated install-layout cutover. This is not a
/// fallback configuration source and does not open or initialize a live root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBinding")]
pub struct RuntimeDataBinding {
    runtime_data_root: PathBuf,
    storage_space_id: String,
    #[serde(skip_serializing_if = "RuntimeDirectoryLayout::is_default")]
    directories: RuntimeDirectoryLayout,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    runtime_data_root: PathBuf,
    storage_space_id: String,
    #[serde(default)]
    directories: RuntimeDirectoryLayout,
}

impl TryFrom<RawBinding> for RuntimeDataBinding {
    type Error = anyhow::Error;
    fn try_from(raw: RawBinding) -> Result<Self> {
        Self::with_layout(raw.runtime_data_root, raw.storage_space_id, raw.directories)
    }
}

impl RuntimeDataBinding {
    pub fn new(root: impl AsRef<Path>, space: impl Into<String>) -> Result<Self> {
        Self::with_layout(root, space, RuntimeDirectoryLayout::default())
    }

    pub fn with_layout(
        root: impl AsRef<Path>,
        space: impl Into<String>,
        directories: RuntimeDirectoryLayout,
    ) -> Result<Self> {
        let storage_space_id = space.into();
        // Reuse the topology owner's root and storage identifier validation.
        InstancePathPlan::with_layout(&root, "binding-validation", directories.clone())?
            .cas_paths(&storage_space_id)?;
        Ok(Self {
            runtime_data_root: root.as_ref().to_path_buf(),
            storage_space_id,
            directories,
        })
    }
    pub fn runtime_data_root(&self) -> &Path {
        &self.runtime_data_root
    }
    pub fn storage_space_id(&self) -> &str {
        &self.storage_space_id
    }
    pub fn directory_layout(&self) -> &RuntimeDirectoryLayout {
        &self.directories
    }
    pub fn instance_plan(&self, instance: &str) -> Result<InstancePathPlan> {
        InstancePathPlan::with_layout(&self.runtime_data_root, instance, self.directories.clone())
    }
    pub fn instance_plan_from_config(&self, config: impl AsRef<Path>) -> Result<InstancePathPlan> {
        InstancePathPlan::from_config_path_with_layout(
            &self.runtime_data_root,
            config,
            self.directories.clone(),
        )
    }
    pub fn cas_paths(&self, instance: &str) -> Result<CasPaths> {
        self.instance_plan(instance)?
            .cas_paths(&self.storage_space_id)
    }
}
