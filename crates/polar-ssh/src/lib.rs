//! Shared OpenSSH transport: master lifecycle, leases, channels and forwarding.
#[cfg(feature = "runtime")]
mod lease;
#[cfg(feature = "runtime")]
mod manager;
#[cfg(feature = "runtime")]
pub use lease::*;
mod command;
pub use command::*;
#[cfg(feature = "runtime")]
pub use manager::*;
#[cfg(feature = "runtime")]
mod setup;
#[cfg(feature = "runtime")]
pub use setup::{check_setup_destination, configured_aliases, setup_login_command};
use serde::{Deserialize, Serialize};

/// Identity of one verified transport generation, independent of any consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTransportGeneration {
    pub supervisor_instance_id: u64,
    pub generation: u64,
}
impl SshTransportGeneration {
    pub fn supervisor_instance_id(&self) -> u64 {
        self.supervisor_instance_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

pub fn is_canonical_ssh_destination(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=255).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes.iter().any(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'_' | b'-' | b'@' | b':' | b'%' | b'+' | b'[' | b']'
                )
        })
}

/// Validate the Unix-domain socket identity used by a Host-owned OpenSSH
/// ControlMaster. The bounded absolute grammar keeps the path representable in
/// `sockaddr_un` and prevents a frontend from substituting a relative path.
pub fn is_canonical_ssh_control_socket(value: &str) -> bool {
    let bytes = value.as_bytes();
    (2..=103).contains(&bytes.len())
        && bytes[0] == b'/'
        && !value.ends_with('/')
        && !value.contains("//")
        && !value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
}
