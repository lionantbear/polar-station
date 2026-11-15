//! One owned streamlocal forward on the existing managed SSH master.
use crate::ssh_connection_manager::{
    platform_ssh_runtime_root, SshConnectionLease, SshConnectionManager, SshConnectionStatus,
};
use crate::target_registry::TargetRegistry;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct CodexNetwork {
    pub targets: TargetRegistry,
    pub ssh: SshConnectionManager,
}

pub(crate) struct SshSocketForward {
    manager: SshConnectionManager,
    lease: SshConnectionLease,
    rule: String,
    directory: Option<tempfile::TempDir>,
    socket: PathBuf,
}

impl SshSocketForward {
    pub(crate) async fn connect(
        network: &CodexNetwork,
        target_id: &str,
        remote_socket: &str,
    ) -> Result<Self, &'static str> {
        validate_remote_socket(remote_socket)?;
        Self::connect_destination(network, target_id, remote_socket).await
    }

    pub(crate) async fn connect_host(
        network: &CodexNetwork,
        target_id: &str,
        port: u16,
    ) -> Result<Self, &'static str> {
        if port == 0 {
            return Err("invalid_forward_port");
        }
        Self::connect_destination(network, target_id, &format!("127.0.0.1:{port}")).await
    }

    async fn connect_destination(
        network: &CodexNetwork,
        target_id: &str,
        destination: &str,
    ) -> Result<Self, &'static str> {
        let target = network
            .targets
            .get(target_id)
            .ok_or("ssh_target_unavailable")?;
        let lease = target
            .resource_ssh_lease(&network.ssh)
            .await
            .map_err(|e| e.code().as_str())?
            .ok_or("ssh_target_unavailable")?;
        let root = platform_ssh_runtime_root()
            .parent()
            .ok_or("forward_path_unavailable")?
            .join("control");
        crate::local_ipc::prepare_private_directory(&root)
            .map_err(|_| "forward_path_unavailable")?;
        let directory = tempfile::Builder::new()
            .prefix("f-")
            .tempdir_in(root)
            .map_err(|_| "forward_path_unavailable")?;
        let socket = directory.path().join("s");
        // The only callers pass either a validated absolute UDS or a typed
        // loopback port. No arbitrary host/bind/forward syntax is accepted.
        let local = socket.to_str().ok_or("invalid_forward_socket")?;
        validate_local_socket(local)?;
        let rule = format!("{local}:{destination}");
        network
            .ssh
            .validate(&lease)
            .await
            .map_err(|e| e.code().as_str())?;
        // Claim ownership before the command: a lost control reply may still have
        // created the forward, so cancellation must also run on failed startup.
        let forward = Self {
            manager: network.ssh.clone(),
            lease,
            rule,
            directory: Some(directory),
            socket,
        };
        change_forward(&forward.lease, false, &forward.rule).await?;
        Ok(forward)
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub(crate) fn current(&self) -> bool {
        self.manager
            .current_status_change(self.lease.target_id())
            .is_some_and(|s| {
                s.status == SshConnectionStatus::Connected
                    && s.generation == Some(self.lease.generation())
            })
    }
}

fn validate_remote_socket(remote: &str) -> Result<(), &'static str> {
    if !remote.starts_with('/') || remote.contains(':') || remote.chars().any(char::is_control) {
        return Err("invalid_forward_socket");
    }
    Ok(())
}

fn validate_local_socket(local: &str) -> Result<(), &'static str> {
    if !local.starts_with('/')
        || local.len() > 100
        || local.contains(':')
        || local.chars().any(char::is_control)
    {
        return Err("invalid_forward_socket");
    }
    Ok(())
}

async fn change_forward(
    lease: &SshConnectionLease,
    cancel: bool,
    rule: &str,
) -> Result<(), &'static str> {
    let mut command = tokio::process::Command::new("/usr/bin/ssh");
    command
        .args(lease.forward_control_args(cancel, rule))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(5), command.status())
        .await
        .map_err(|_| "ssh_forward_timeout")?
        .map_err(|_| "ssh_forward_failed")?;
    if status.success() {
        Ok(())
    } else {
        Err("ssh_forward_failed")
    }
}

impl Drop for SshSocketForward {
    fn drop(&mut self) {
        let (lease, rule, directory) =
            (self.lease.clone(), self.rule.clone(), self.directory.take());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // This private ControlPath belongs to the old generation even
                // after replacement. Never cancel against the new master.
                let _ = change_forward(&lease, true, &rule).await;
                drop(directory);
            });
        }
        // During runtime shutdown the manager owns master teardown. The owned
        // directory is still removed; no shared socket or Session is touched.
    }
}




