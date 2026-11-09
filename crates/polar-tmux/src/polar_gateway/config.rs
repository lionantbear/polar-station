//! Explicit Polar routes are separate from SSH/tmux execution target identity.
use std::{collections::BTreeSet, path::{Path, PathBuf}};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolarTargetConfig {
    pub id: String,
    pub display_name: String,
    pub expected_node_id: String,
    pub route: PolarRoute,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub terminal_targets: std::collections::BTreeMap<String,String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PolarRoute {
    LocalUds { socket_path: PathBuf, #[serde(default)] expected_peer_uid: Option<u32> },
    SshUds { execution_target_id: String, remote_socket_path: PathBuf },
}
impl PolarTargetConfig {
    pub fn validate_all(targets: &[Self]) -> Result<()> {
        ensure!(targets.len() <= 32, "too many configured Polar targets");
        let mut ids = BTreeSet::new();
        for target in targets {
            ensure!(!target.id.is_empty() && target.id.len() <= 128 && target.id != "embedded" &&
                target.id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
                "invalid or reserved Polar target ID");
            ensure!(ids.insert(&target.id), "duplicate Polar target ID");
            ensure!(!target.display_name.is_empty() && target.display_name.len() <= 128 &&
                !target.display_name.chars().any(char::is_control), "invalid Polar target name");
            ensure!(target.expected_node_id.len() == 64 && target.expected_node_id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "Polar target identity must be an explicitly pinned canonical Node ID");
            ensure!(target.terminal_targets.len() <= 64, "too many terminal target bindings");
            for (provider, local) in &target.terminal_targets {
                for id in [provider,local] { polar_tmux_protocol::validate_stable_identifier(id,"terminal target binding").map_err(|e|anyhow::anyhow!(e.to_string()))?; }
            }
            match &target.route {
                PolarRoute::LocalUds {socket_path, ..} => validate_socket_path(socket_path)?,
                PolarRoute::SshUds {execution_target_id, remote_socket_path} => {
                    ensure!(!execution_target_id.is_empty() && execution_target_id.len() <= 128,
                        "SSH execution target is required");
                    validate_socket_path(remote_socket_path)?;
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn validate_socket_path(path: &Path) -> Result<()> {
    let text = path.to_str().ok_or_else(|| anyhow::anyhow!("socket path must be UTF-8"))?;
    ensure!(path.is_absolute() && text.len() <= 103 &&
        !text.chars().any(|c| c.is_control() || c.is_whitespace() || c == ':') &&
        !path.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir)),
        "socket path must be absolute, canonical and safe for Unix forwarding");
    Ok(())
}
