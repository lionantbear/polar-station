//! Private StreamLocal forwarding over the existing Station SSH supervisor.
//! This module never starts another SSH master or sends shell commands remotely.
use std::{path::PathBuf, process::Stdio, sync::{Arc, Mutex}, time::Duration};
use anyhow::{ensure, Context, Result};
use tokio::{process::Command, sync::{oneshot, watch}, task::JoinSet};
use crate::{local_extensions::LocalExtensionContext, local_ipc,
    ssh_connection_manager::{platform_ssh_runtime_root, SshConnectionManager, SshConnectionLease, SshConnectionStatus},
    target_registry::TargetRegistry};

pub(super) struct ForwardOwner { stop: watch::Sender<bool>, tasks: Mutex<JoinSet<()>> }
impl Default for ForwardOwner {
    fn default() -> Self { Self {stop: watch::channel(false).0, tasks: Mutex::new(JoinSet::new())} }
}
impl Drop for ForwardOwner {
    fn drop(&mut self) {self.stop.send_replace(true);}
}
pub(super) struct RouteLease {
    pub socket_path: PathBuf,
    stop: watch::Sender<bool>,
    pub revoked: watch::Receiver<bool>,
    target_id: String,
    target_revision: u64,
    ssh_lease: SshConnectionLease,
    targets: TargetRegistry,
    manager: SshConnectionManager,
}
impl RouteLease {
    pub fn is_current(&self) -> bool {
        !*self.revoked.borrow() && self.targets.get(&self.target_id)
            .is_some_and(|target| target.revision() == self.target_revision) &&
        self.manager.current_status_change(&self.target_id).is_some_and(|status|
            status.status == SshConnectionStatus::Connected && status.generation == Some(self.ssh_lease.generation()))
    }
}
impl Drop for RouteLease {
    fn drop(&mut self) { self.stop.send_replace(true); }
}
impl ForwardOwner {
    pub async fn connect(&self, manager: &SshConnectionManager, targets: &TargetRegistry,
        execution_target_id: &str, remote_socket_path: &std::path::Path,
        context: &LocalExtensionContext) -> Result<Arc<RouteLease>> {
        ensure!(!*self.stop.borrow(), "Polar forwarding owner stopped");
        super::config::validate_socket_path(remote_socket_path)?;
        let target = targets.get(execution_target_id).context("SSH execution target missing")?;
        let revision = target.revision();
        let lease = target.resource_ssh_lease(manager).await?
            .context("Polar route requires an SSH execution target")?;
        manager.validate(&lease).await?;
        ensure!(targets.get(execution_target_id).is_some_and(|t| t.revision() == revision), "SSH target changed while connecting");
        let root = platform_ssh_runtime_root().join("polar");
        local_ipc::prepare_private_directory(&root)?;
        let directory = tempfile::Builder::new().prefix("p-").tempdir_in(&root)?;
        let socket_path = directory.path().join("p.sock");
        super::config::validate_socket_path(&socket_path)?;
        let rule = format!("{}:{}", socket_path.display(), remote_socket_path.display());
        let (stop, mut stopped) = watch::channel(false);
        let (revoked, revocation) = watch::channel(false);
        let route = Arc::new(RouteLease {socket_path, stop, revoked: revocation,
            target_id: execution_target_id.to_owned(), target_revision: revision,
            ssh_lease: lease.clone(), targets: targets.clone(), manager: manager.clone()});
        let weak_route = Arc::downgrade(&route);
        let mut changes = manager.subscribe_status_changes();
        let mut global_stop = self.stop.subscribe();
        let lifetime = context.lifetime.clone();
        let (ready, readiness) = oneshot::channel();
        {
        let mut tasks = self.tasks.lock().map_err(|_| anyhow::anyhow!("forward owner lock failed"))?;
        ensure!(!*self.stop.borrow(), "Polar forwarding owner stopped");
        while tasks.try_join_next().is_some() {}
        ensure!(tasks.len() < 64, "Polar forward capacity exceeded");
        tasks.spawn(async move {
            // This owner holds directory and start/cancel operations. Caller
            // timeout cannot race a detached start against an earlier cancel.
            let _directory = directory;
            let started = change_forward(&lease, &rule, false).await;
            let current = weak_route.upgrade().is_some_and(|route| route.is_current());
            let usable = started.is_ok() && current;
            let observed = ready.send(if usable {Ok(())} else {Err(anyhow::anyhow!("SSH Polar forwarding failed or became stale"))}).is_ok();
            if usable && observed {
                // Only lease health, not message polling. Every network request
                // and received frame also validates this exact route generation.
                let mut health = tokio::time::interval(Duration::from_secs(5));
                health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    if *global_stop.borrow() || *stopped.borrow() || !lifetime.is_live() ||
                        !weak_route.upgrade().is_some_and(|route| route.is_current()) {break;}
                    tokio::select! {
                        biased;
                        _ = lifetime.cancelled() => break,
                        _ = stopped.changed() => break,
                        _ = global_stop.changed() => break,
                        changed = changes.recv() => {
                            if changed.is_err() {break;}
                            // Recheck the manager state at the top, never trust
                            // an old event's generation over the manager itself.
                        },
                        _ = health.tick() => {},
                    }
                }
            }
            let _ = revoked.send(true);
            // Cancellation uses this lease's verified ControlPath and rule.
            // It never retires the master used by other terminals/resources.
            if change_forward(&lease, &rule, true).await.is_err() {
                tracing::warn!("Polar private forward cancellation failed; its private path is being removed");
            }
        });
        }
        tokio::select! {
            biased;
            _ = context.lifetime.cancelled() => anyhow::bail!("desktop disconnected during SSH forward setup"),
            result = tokio::time::timeout_at(context.deadline, readiness) => {result.context("SSH forward deadline")?.context("SSH forward owner stopped")??;}
        }
        ensure!(route.is_current(), "SSH route changed before Polar handshake");
        Ok(route)
    }

    pub async fn shutdown(&self) {
        self.stop.send_replace(true);
        let mut tasks = match self.tasks.lock() {
            Ok(mut tasks) => std::mem::take(&mut *tasks),
            Err(_) => return,
        };
        // Start and cancel are each bounded at five seconds. Drain before the
        // Host shuts down its SSH supervisor and removes shared runtime paths.
        if tokio::time::timeout(Duration::from_secs(12), async {
            while tasks.join_next().await.is_some() {}
        }).await.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
    }
}
async fn change_forward(lease: &SshConnectionLease, rule: &str, cancel: bool) -> Result<()> {
    let mut command = Command::new("/usr/bin/ssh");
    command.args(lease.forward_control_args(cancel, rule)).kill_on_drop(true)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(5), command.status()).await??;
    ensure!(status.success(), "SSH StreamLocal operation failed");
    Ok(())
}
