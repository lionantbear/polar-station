//! Applied Host directory names, shared with configuration owners.
use crate::{validate_id, RuntimeArea};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDirectoryLayout {
    pub instances: String,
    pub cas: String,
    pub components: String,
    pub cache: String,
    pub tmp: String,
    pub run: String,
    pub logs: String,
}

impl Default for RuntimeDirectoryLayout {
    /// Compatibility for existing v2 installations and private portable roots.
    /// Managed bootstrap supplies its layout explicitly from HCM.
    fn default() -> Self {
        Self {
            instances: "instances".into(),
            cas: "cas".into(),
            components: "components".into(),
            cache: "cache".into(),
            tmp: "tmp".into(),
            run: "run".into(),
            logs: "logs".into(),
        }
    }
}

impl RuntimeDirectoryLayout {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn validate(&self) -> Result<()> {
        let names = [
            &self.instances,
            &self.cas,
            &self.components,
            &self.cache,
            &self.tmp,
            &self.run,
            &self.logs,
        ];
        for name in names {
            validate_id(name)?;
            ensure!(
                name != "layout.json",
                "Host directory conflicts with layout marker"
            );
        }
        let unique: std::collections::BTreeSet<_> = names.into_iter().collect();
        ensure!(
            unique.len() == names.len(),
            "Host directory names must be distinct"
        );
        Ok(())
    }

    pub(crate) fn area(&self, area: RuntimeArea) -> &str {
        match area {
            RuntimeArea::Cache => &self.cache,
            RuntimeArea::Tmp => &self.tmp,
            RuntimeArea::Run => &self.run,
            RuntimeArea::Logs => &self.logs,
            _ => area.name(),
        }
    }
}
