//! Station adapter for the shared SSH owner; no transport implementation here.
pub use polar_ssh::*;

#[cfg(unix)]
pub fn run_sftp_channel(target_id: &str, generation: u64, args: &[String]) -> std::io::Result<()> {
    polar_ssh::run_sftp_channel_before_exec(target_id, generation, args, || {
        polar_local_access_runtime::operation_trace::finish_process_trace()
    })
}

/// Station wire adaptation only. Lifecycle remains exclusively in polar-ssh.
#[derive(Clone)]
pub struct SshConnectionManager(polar_ssh::SshConnectionManager);
impl std::ops::Deref for SshConnectionManager {
    type Target = polar_ssh::SshConnectionManager;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl SshConnectionManager {
    pub fn from_shared(manager: polar_ssh::SshConnectionManager) -> Self { Self(manager) }
    pub fn system(root: std::path::PathBuf) -> Self {
        Self(polar_ssh::SshConnectionManager::system(root))
    }
    pub async fn transport_generation(
        &self,
        target: &str,
    ) -> Option<polar_station_protocol::SshTransportGenerationV1> {
        self.0.transport_generation(target).await.map(Into::into)
    }
    pub fn transport_generation_for_lease(
        &self,
        lease: &SshConnectionLease,
    ) -> polar_station_protocol::SshTransportGenerationV1 {
        self.0.transport_generation_for_lease(lease).into()
    }
    pub async fn wait_for_connected_after(
        &self,
        target: &str,
        destination: &str,
        after: Option<polar_station_protocol::SshTransportGenerationV1>,
    ) -> Result<polar_station_protocol::SshTransportGenerationV1, SshConnectionError> {
        self.0
            .wait_for_connected_after(target, destination, after.map(Into::into))
            .await
            .map(Into::into)
    }
    pub async fn wait_for_connected_after_routed(
        &self,
        target: &str,
        destination: &str,
        profile: &SshRouteProfile,
        after: Option<polar_station_protocol::SshTransportGenerationV1>,
    ) -> Result<polar_station_protocol::SshTransportGenerationV1, SshConnectionError> {
        self.0
            .wait_for_connected_after_routed(target, destination, profile, after.map(Into::into))
            .await
            .map(Into::into)
    }
}
