//! Station execution resources, shared by terminal, file and supervisor callers.
//! No socket listener or native layout ownership.
#[cfg(unix)]
pub mod terminal_io;

#[derive(Clone)]
pub struct StationExecutionResources {
    ssh: polar_ssh::SshConnectionManager,
    targets: Option<target_registry::TargetRegistry>,
}
impl StationExecutionResources {
    pub fn system(root: std::path::PathBuf) -> Self {
        Self { ssh: polar_ssh::SshConnectionManager::system(root), targets: None }
    }
    pub fn from_ssh(ssh: polar_ssh::SshConnectionManager) -> Self { Self { ssh, targets: None } }
    pub fn with_targets(mut self, targets: target_registry::TargetRegistry) -> Self {
        self.targets = Some(targets); self
    }
    pub fn targets(&self) -> Option<target_registry::TargetRegistry> { self.targets.clone() }
    pub fn ssh(&self) -> polar_ssh::SshConnectionManager { self.ssh.clone() }
}

pub mod target_registry;
pub mod polar_target_config;
pub mod ssh_connection_manager;

pub mod services;
pub mod host_identity;
#[cfg(unix)]
pub mod local_ipc;
pub mod native_account;
pub mod dictate_host;
pub mod local_extensions;
#[cfg(unix)]
pub mod polar_gateway;
#[cfg(unix)]
pub mod polar_access_transport;
#[cfg(unix)]
pub mod pane_host;
pub use polar_local_access_runtime::operation_trace;


