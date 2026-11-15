//! User overrides for the two explicitly editable refinement policies.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptOverrides {
    pub cleanup: Option<String>,
    pub english: Option<String>,
}
impl PromptOverrides {
    pub fn get(&self, mode: crate::RefineMode) -> Option<&String> {
        match mode {
            crate::RefineMode::Cleanup => self.cleanup.as_ref(),
            crate::RefineMode::English => self.english.as_ref(),
        }
    }
    pub fn set(&mut self, mode: crate::RefineMode, value: Option<String>) {
        match mode {
            crate::RefineMode::Cleanup => self.cleanup = value,
            crate::RefineMode::English => self.english = value,
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [&self.cleanup, &self.english].into_iter().flatten() {
            anyhow::ensure!(
                !value.trim().is_empty()
                    && !value.contains('\0')
                    && value.len() <= crate::policy::MAX_TEXT_BYTES,
                "text.invalid_prompt_override"
            );
        }
        Ok(())
    }
}
