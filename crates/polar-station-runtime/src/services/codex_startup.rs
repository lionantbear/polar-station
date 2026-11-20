//! Launch adapter for the installed Codex daemon manager.
use super::{
    codex_connections::ConnectionKey,
    codex_control::{CodexControlService, ControlResult},
    codex_endpoint::CodexEndpoint,
};
use codex_app_server_protocol::GetAccountResponse;
use polar_station_protocol::{ApplicationStateApiV1, CodexResultV1};
use polar_station_settings::CodexControlModeV1;
use std::{process::Stdio, sync::Arc, time::Duration};

impl CodexControlService {
    pub(super) async fn start_local_server(
        &self,
        id: &str,
        may_configure: bool,
    ) -> ControlResult<CodexResultV1> {
        if !may_configure {
            return Err("scope_denied");
        }
        self.connect_or_start_local(id).await?;
        Ok(CodexResultV1::Settings {
            settings: self.config.read().await.clone(),
        })
    }

    pub(super) async fn prepare_local_codex(&self, id: &str) -> ControlResult<bool> {
        if self.local_account_endpoint_id().await.as_deref() != Some(id) {
            return Err("codex_start_requires_default_endpoint");
        }
        let client = self.connect_or_start_local(id).await?;
        let value = tokio::time::timeout(
            Duration::from_secs(5),
            client.request("account/read", serde_json::json!({"refreshToken": false})),
        )
        .await
        .map_err(|_| "codex_account_check_timeout")?
        .map_err(|_| "codex_account_check_failed")?;
        let account: GetAccountResponse =
            serde_json::from_value(value).map_err(|_| "codex_account_check_failed")?;
        Ok(!account.requires_openai_auth || account.account.is_some())
    }

    async fn connect_or_start_local(&self, id: &str) -> ControlResult<Arc<CodexEndpoint>> {
        self.discover_account_endpoint(crate::target_registry::LOCAL_TARGET_ID).await;
        let is_local = self
            .config
            .read()
            .await
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id == id)
            .map(|endpoint| endpoint.owner_host_id == self.owner
                && endpoint.execution_target_id == crate::target_registry::LOCAL_TARGET_ID
                && endpoint.control_mode == CodexControlModeV1::Local)
            .ok_or("endpoint_not_found")?;
        if !is_local {
            return Err("local_codex_start_only");
        }

        // Serialize daemon launch without holding the general Codex operations lock.
        let gate = self.connection_attempts.gate(ConnectionKey::LocalDaemonStart).await;
        let (_guard, already_attempted) = match gate.try_lock() {
            Ok(guard) => (guard, false),
            Err(_) => (gate.lock().await, true),
        };
        match tokio::time::timeout(Duration::from_secs(5), self.endpoint(id)).await {
            Ok(Ok(client)) => return Ok(client),
            Ok(Err("codex_unavailable")) => (),
            Ok(Err(code)) => return Err(code),
            Err(_) => return Err("codex_handshake_timeout"),
        };
        if self.local_account_endpoint_id().await.as_deref() != Some(id) {
            return Err("codex_start_requires_default_endpoint");
        }
        if already_attempted {
            return Err("codex_start_failed");
        }

        // Use the same account directory and executable resolution as discovery.
        let home = polar_inhouse_backend_codex::native_codex_home()
            .map_err(|_| "codex_home_unavailable")?;
        let script = format!(
            "{}; command -v \"$station_codex\" >/dev/null 2>&1 || exit 127; \
             exec env CODEX_HOME=\"$1\" \"$station_codex\" app-server daemon start",
            super::target_tools::CODEX_RESOLVE
        );
        let mut command = tokio::process::Command::new("sh");
        command
            .args(["-lc", &script, "station-codex"])
            .arg(home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        super::child_process::configure_parent_lease(&mut command)
            .map_err(|_| "codex_launch_failed")?;
        let status = tokio::time::timeout(Duration::from_secs(30), command.status())
            .await
            .map_err(|_| "codex_start_timeout")?
            .map_err(|_| "codex_launcher_unavailable")?;
        if status.code() == Some(127) {
            return Err("codex_not_installed");
        }
        if !status.success() {
            return Err("codex_start_failed");
        }
        let client = tokio::time::timeout(Duration::from_secs(5), self.endpoint(id))
            .await
            .map_err(|_| "codex_handshake_timeout")?
            .map_err(|_| "codex_handshake_failed")?;
        self.publisher.publish(ApplicationStateApiV1::CodexV1);
        Ok(client)
    }
}
