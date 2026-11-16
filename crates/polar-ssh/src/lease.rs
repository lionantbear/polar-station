//! Verified master identity and exit notification; independent of tmux.
use crate::{SshEndpoint, SshRouteKind};
use std::{ffi::OsString, path::PathBuf};
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct SshConnectionLease {
    pub(crate) target_id: String,
    pub(crate) generation: u64,
    pub(crate) master_pid: u32,
    pub(crate) destination: String,
    pub(crate) destination_fingerprint: String,
    pub(crate) control_socket: PathBuf,
    pub(crate) route: SshRouteKind,
    pub(crate) operation_id: u64,
    pub(crate) exited: watch::Receiver<bool>,
}

impl PartialEq for SshConnectionLease {
    fn eq(&self, other: &Self) -> bool {
        self.target_id == other.target_id
            && self.generation == other.generation
            && self.master_pid == other.master_pid
            && self.destination == other.destination
            && self.destination_fingerprint == other.destination_fingerprint
            && self.control_socket == other.control_socket
            && self.route == other.route
            && self.operation_id == other.operation_id
    }
}
impl Eq for SshConnectionLease {}

impl SshConnectionLease {
    

    pub fn master_exited(&self) -> bool {
        *self.exited.borrow()
    }

    pub async fn wait_for_exit(&self) -> bool {
        let mut exited = self.exited.clone();
        loop {
            if *exited.borrow_and_update() {
                return true;
            }
            if exited.changed().await.is_err() {
                return false;
            }
        }
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn route(&self) -> SshRouteKind {
        self.route
    }

    

    pub fn endpoint(&self) -> SshEndpoint {
        // Constructed only from the manager's verified master identity.
        SshEndpoint {
            destination: self.destination.clone(),
            control_socket: self.control_socket.to_string_lossy().into_owned(),
        }
    }

    pub fn master_pid(&self) -> u32 {
        self.master_pid
    }

    pub fn forward_control_args(&self, cancel: bool, rule: &str) -> Vec<OsString> {
        vec![
            "-S".into(),
            self.control_socket.as_os_str().to_owned(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ProxyCommand=/usr/bin/false".into(),
            "-O".into(),
            if cancel { "cancel" } else { "forward" }.into(),
            "-L".into(),
            rule.into(),
            self.destination.clone().into(),
        ]
    }
}
