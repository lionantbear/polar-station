use serde::{Deserialize, Serialize};

/// One policy for native and Wasm readers; budgets are on-demand, not allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    pub message_mib: u32,
    pub presentation_mib: u32,
}
impl Default for CachePolicy {
    fn default() -> Self {Self{message_mib:512,presentation_mib:32}}
}
impl CachePolicy {
    pub fn validate(self) -> Result<(), String> {
        if !(16..=2048).contains(&self.message_mib) || !(1..=256).contains(&self.presentation_mib)
            || self.message_mib.saturating_add(self.presentation_mib)>2048 {
            return Err("Chat memory requires 16–2048 MiB for messages, 1–256 MiB for rendered content, and at most 2048 MiB combined".into());
        }
        Ok(())
    }
    pub fn message_bytes(self) -> usize {self.message_mib as usize * 1024 * 1024}
    pub fn presentation_bytes(self) -> usize {self.presentation_mib as usize * 1024 * 1024}
}


