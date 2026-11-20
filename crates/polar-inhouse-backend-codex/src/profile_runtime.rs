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
                        (crate::client::start_embedded(cfg).await?, None)
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
        cfg.profile = Some(CodexProfilePaths::resolve(&self.root, profile_id)?);
        let runtime = self.runtime(profile_id, cfg.codex_mode)?;
        CodexAgentRunner::new_managed(cfg, runtime)
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
        cfg.profile = Some(CodexProfilePaths::resolve(&self.root, profile_id)?);
        bootstrap::resolve_native_profile_settings(&mut cfg).await?;
        let runtime = self.runtime(profile_id, mode)?;
        let client = runtime.lease(&cfg).await?;
        crate::codex_app_server_compat::account_operation(&client, action).await
    }
}


