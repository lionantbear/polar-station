//! Launch adapter for the installed Codex daemon manager; no embedded runtime.
use super::codex_control::{CodexControlService,ControlResult};
use polar_station_settings::CodexControlModeV1;
use polar_tmux_protocol::CodexResultV1;
use std::{path::PathBuf,process::Stdio,time::Duration};
impl CodexControlService {
    /// Called under the existing operations lock, so repeated requests singleflight.
    pub(super) async fn start_local_server(&self,id:&str,may_configure:bool)->ControlResult<CodexResultV1>{
        if !may_configure{return Err("scope_denied")}
        if !self.config.read().await.enabled { return Err("control_disabled"); }
        self.discover_account_endpoints().await;
        let config=self.config.read().await;
        let endpoint=config.endpoints.iter().find(|e|e.id==id).cloned().ok_or("endpoint_not_found")?;
        drop(config);
        if endpoint.owner_host_id!=self.owner || endpoint.control_mode!=CodexControlModeV1::Local{return Err("local_codex_start_only")}
        if self.endpoint(id).await.is_ok(){return Ok(CodexResultV1::Settings{settings:self.config.read().await.clone()})}
        let home=std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(||std::env::var_os("HOME").map(|p|PathBuf::from(p).join(".codex"))).ok_or("codex_home_unavailable")?;
        let socket=codex_app_server_client::app_server_control_socket_path(&home).map_err(|_|"codex_home_unavailable")?;
        if socket.as_path()!=std::path::Path::new(&endpoint.socket_path){return Err("codex_start_requires_default_endpoint")}
        // Reuse the same login-shell executable resolution as existing session launch.
        // Codex owns the daemon's startup lock, package and readiness management.
        let mut command=tokio::process::Command::new("sh");
        command.args(["-lc","exec codex app-server daemon start"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        super::child_process::configure_parent_lease(&mut command).map_err(|_|"codex_launch_failed")?;
        let status=tokio::time::timeout(Duration::from_secs(30),command.status()).await.map_err(|_|"codex_start_timeout")?.map_err(|_|"codex_launcher_unavailable")?;
        if !status.success(){return Err("codex_start_failed")}
        tokio::time::timeout(Duration::from_secs(10),self.endpoint(id)).await.map_err(|_|"codex_handshake_timeout")??;
        self.publisher.publish(polar_tmux_protocol::ApplicationStateApiV1::CodexV1);
        Ok(CodexResultV1::Settings{settings:self.config.read().await.clone()})
    }
}
