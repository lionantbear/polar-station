//! Polar owns profile paths and native runtime lifetimes, not Codex TOML/OAuth.

use crate::codex_app_server_bootstrap as bootstrap;
use crate::shared_client::{SharedCodexClient, ThreadClient};
use crate::{CodexAgentConfig, CodexAgentRunner};
use polar_runtime_paths::InstancePathPlan;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProfilePaths {
    pub root: PathBuf,
    pub profile_id: String,
    pub config_file: PathBuf,
    pub requirements_file: PathBuf,
    pub home: PathBuf,
    pub runtime_lock: PathBuf,
}

impl CodexProfilePaths {
    pub fn resolve(instance: &InstancePathPlan, profile_id: &str) -> anyhow::Result<Self> {
        let config = instance.backend_config(profile_id)?;
        Ok(Self {
            root: instance.path(),
            profile_id: profile_id.to_string(),
            config_file: config.join("codex.toml"),
            requirements_file: config.join("requirements.toml"),
            home: instance.backend_home(profile_id)?,
            runtime_lock: instance
                .directory_path(
                    polar_runtime_paths::RuntimeModule::Codex,
                    polar_runtime_paths::RuntimeArea::Run,
                )
                .join(format!("{profile_id}.lock")),
        })
    }
}

#[derive(Default)]
pub(crate) struct ProfileRuntime {
    client: AsyncMutex<Option<ProfileClient>>,
    mode: Mutex<Option<crate::CodexMode>>,
}

struct ProfileClient {
    shared: Arc<SharedCodexClient>,
    configuration: serde_json::Value,
}

impl ProfileRuntime {
    pub(crate) async fn lease(&self, cfg: &CodexAgentConfig) -> anyhow::Result<ThreadClient> {
        let mut current = self.client.lock().await;
        if current
            .as_ref()
            .is_none_or(|client| client.shared.is_closed())
        {
            let paths = cfg
                .profile
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing managed Codex profile"))?;
            let (client, guard) = match (cfg.backend, cfg.socket_path.as_deref()) {
                (crate::CodexRuntimeBackend::UnixSocket, Some(path)) => (
                    crate::client::connect_unix(path, "polar-inhouse-agent", Vec::new()).await?,
                    None,
                ),
                (backend, _) => {
                    let lock = lock_profile(paths).await?;
                    let (client, child) = if backend == crate::CodexRuntimeBackend::UnixSocket {
                        let (client, child) = crate::client::start_private(cfg, paths).await?;
                        (client, Some(child))
                    } else {
                        (start_profile_app_server(cfg).await?, None)
                    };
                    (client, Some((lock, child)))
                }
            };
            *current = Some(ProfileClient {
                shared: SharedCodexClient::start_guarded(client, guard),
                configuration: crate::profile_config::snapshot(cfg),
            });
        }
        let client = current.as_ref().expect("profile client initialized");
        anyhow::ensure!(
            client.configuration == crate::profile_config::snapshot(cfg),
            "Changing the backend profile configuration requires a profile runtime restart"
        );
        Ok(client.shared.lease())
    }
}

async fn start_profile_app_server(cfg: &CodexAgentConfig) -> anyhow::Result<crate::client::AppServerClient> {
    let result = crate::client::start_embedded(cfg).await;
    let shared_address = cfg.profile.as_ref().is_some_and(|profile| profile.profile_id == CodexBackendRegistry::STATION_PROFILE_ID)
        && std::env::home_dir().and_then(|home| crate::client::polar_control_socket(&home).ok()).as_ref() == cfg.socket_path.as_ref()
        && cfg.socket_path.is_some();
    if shared_address && result.as_ref().err().is_some_and(|error| error.chain().any(|cause|
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    )) {
        // The existing listener remains the account endpoint. A second Polar
        // instance continues using its own runtime and profile state.
        tracing::warn!("The account's Polar Codex socket is already served by another instance");
        let mut private = cfg.clone();
        private.socket_path = None;
        return crate::client::start_embedded(&private).await;
    }
    result
}

async fn lock_profile(paths: &CodexProfilePaths) -> anyhow::Result<std::fs::File> {
    tokio::fs::create_dir_all(&paths.home).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&paths.home, std::fs::Permissions::from_mode(0o700)).await?;
    }
    tokio::fs::create_dir_all(
        paths
            .runtime_lock
            .parent()
            .expect("planned runtime lock directory"),
    )
    .await?;
    let guard = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&paths.runtime_lock)?;
    guard.try_lock().map_err(|err| {
        anyhow::anyhow!("Codex profile {} is already open: {err}", paths.profile_id)
    })?;
    Ok(guard)
}

/// One registry per Polar instance, keyed by backend profile rather than role/slot.
pub struct CodexBackendRegistry {
    root: InstancePathPlan,
    profiles: Mutex<HashMap<String, Arc<ProfileRuntime>>>,
}

impl std::fmt::Debug for CodexBackendRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexBackendRegistry")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl CodexBackendRegistry {
    pub const STATION_PROFILE_ID: &'static str = "system.eve.codex";

    pub fn new(root: InstancePathPlan) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            profiles: Mutex::new(HashMap::new()),
        })
    }

    pub fn build_runner(
        &self,
        profile_id: &str,
        mut cfg: CodexAgentConfig,
    ) -> anyhow::Result<CodexAgentRunner> {
        self.configure_profile(profile_id, &mut cfg)?;
        let runtime = self.runtime(profile_id, cfg.codex_mode)?;
        CodexAgentRunner::new_managed(cfg, runtime)
    }

    fn configure_profile(&self, profile_id: &str, cfg: &mut CodexAgentConfig) -> anyhow::Result<()> {
        cfg.profile = Some(CodexProfilePaths::resolve(&self.root, profile_id)?);
        if profile_id == Self::STATION_PROFILE_ID
            && cfg.backend == crate::CodexRuntimeBackend::InProcess
            && cfg.codex_mode == crate::CodexMode::Agentic
            && cfg.socket_path.is_none() {
            if let Some(home) = std::env::home_dir() {
                cfg.socket_path = Some(crate::client::polar_control_socket(&home)?);
            }
        }
        Ok(())
    }

    /// Start the same registered runtime used by Eve's own Threads.
    pub async fn start_station_app_server(&self, mut cfg: CodexAgentConfig) -> anyhow::Result<()> {
        if cfg.backend != crate::CodexRuntimeBackend::InProcess || cfg.codex_mode != crate::CodexMode::Agentic {
            return Ok(());
        }
        self.configure_profile(Self::STATION_PROFILE_ID, &mut cfg)?;
        bootstrap::resolve_native_profile_settings(&mut cfg).await?;
        crate::codex::validate_codex_mode_config(&cfg)?;
        let runtime = self.runtime(Self::STATION_PROFILE_ID, cfg.codex_mode)?;
        runtime.lease(&cfg).await?;
        Ok(())
    }

    /// Reuse the native conversation boundary with separate runtime state and
    /// the original profile's auth source. Never copy credentials or change the
    /// active agentic profile's mode.
    pub fn build_memory_runner(
        &self,
        profile_id: &str,
        mut cfg: CodexAgentConfig,
    ) -> anyhow::Result<CodexAgentRunner> {
        let original = CodexProfilePaths::resolve(&self.root, profile_id)?;
        if cfg.auth_home.is_none() {
            cfg.auth_home = Some(crate::auth_source::resolve(None, &original.home)?);
        }
        cfg.codex_mode = crate::CodexMode::Conversation;
        cfg.backend = crate::CodexRuntimeBackend::InProcess;
        cfg.socket_path = None;
        cfg.memory_tool_enabled = false;
        cfg.generate_memories = None;
        cfg.use_memories = None;
        cfg.min_rollout_idle_hours = None;
        cfg.memory_extract_model = None;
        cfg.memory_consolidation_model = None;
        cfg.approval_handler = None;
        self.build_runner(&format!("{profile_id}.thread-memory"), cfg)
    }

    fn runtime(
        &self,
        profile_id: &str,
        codex_mode: crate::CodexMode,
    ) -> anyhow::Result<Arc<ProfileRuntime>> {
        let runtime = self
            .profiles
            .lock()
            .expect("Codex profiles poisoned")
            .entry(profile_id.to_string())
            .or_default()
            .clone();
        {
            let mut mode = runtime.mode.lock().expect("Codex mode poisoned");
            anyhow::ensure!(
                mode.is_none_or(|mode| mode == codex_mode),
                "Changing Codex profile mode requires a drained runtime restart"
            );
            *mode = Some(codex_mode);
        }
        Ok(runtime)
    }

    pub async fn authenticate(
        &self,
        profile_id: &str,
        mut cfg: CodexAgentConfig,
        action: polar_inhouse_contract::InhouseBackendAuthAction,
    ) -> anyhow::Result<polar_inhouse_contract::InhouseBackendAuthView> {
        if let polar_inhouse_contract::InhouseBackendAuthAction::Import {
            auth_json,
            overwrite,
        } = action
        {
            anyhow::ensure!(cfg.backend != crate::CodexRuntimeBackend::UnixSocket || cfg.socket_path.is_none(), "Import credentials on the Host that owns the external Codex app-server");
            return bootstrap::import_profile_credentials(
                &self.root, profile_id, &auth_json, overwrite,
            );
        }
        let mode = cfg.codex_mode;
        self.configure_profile(profile_id, &mut cfg)?;
        bootstrap::resolve_native_profile_settings(&mut cfg).await?;
        let runtime = self.runtime(profile_id, mode)?;
        let client = runtime.lease(&cfg).await?;
        crate::codex_app_server_compat::account_operation(&client, action).await
    }
}


