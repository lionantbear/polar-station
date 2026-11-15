use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Listener configuration contains no authorization policy. Grants live in Vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalChatConfig {
    pub enabled: bool,
    pub socket_path: PathBuf,
}
