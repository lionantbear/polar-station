use crate::codex::{effective_model_provider, effective_provider_base_url, CodexAgentConfig};
use crate::connector_policy::ensure_resolved_connector_allowed;
use anyhow::Context;
use codex_app_server_client::EmbeddedNetworkPolicy;
use codex_app_server_client::EnvironmentManager;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessClientStartArgs;
use codex_arg0::Arg0DispatchPaths;
use codex_config::CloudConfigBundleLoader;
use codex_config::Constrained;
use codex_config::LoaderOverrides;
use codex_config::{CloudConfigBundle, CloudRequirementsFragment, CloudRequirementsTomlBundle};
use codex_core::config::{
    find_codex_home, resolve_profile_v2_config_path, Config, ConfigBuilder, ConfigOverrides,
};
use codex_features::Features;
use codex_feedback::CodexFeedback;
use codex_http_client::{HttpClientFactory, OutboundProxyPolicy};
use codex_models_manager::bundled_models_response;
use codex_protocol::config_types::{SandboxMode, WebSearchMode};
use codex_protocol::openai_models::{ModelInfo, ModelsResponse, ToolMode};
use codex_protocol::protocol::{AskForApproval, SessionSource};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use toml::Value as TomlValue;

const IN_PROCESS_NOTIFICATION_CHANNEL_CAPACITY: usize = 2048;

// Native TOML keys, not fields on codex_core::Config. Both startup and
// thread/start/resume reloads must pass these through the upstream loader.
pub(crate) const CONVERSATION_DISABLED_CONFIG_KEYS: &[&str] = &[
    "include_permissions_instructions",
    "include_apps_instructions",
    "include_collaboration_mode_instructions",
    "include_environment_context",
    "skills.include_instructions",
    "skills.bundled.enabled",
    "cloud.skills.enabled",
    "orchestrator.mcp.enabled",
    "tools.experimental_request_user_input.enabled",
    "tools.update_plan.enabled",
    "agents.enabled",
];

fn conversation_mode_cli_overrides(cfg: &CodexAgentConfig) -> Vec<(String, TomlValue)> {
    if !cfg.codex_mode.is_conversation() {
        return Vec::new();
    }
    let mut overrides: Vec<_> = CONVERSATION_DISABLED_CONFIG_KEYS
        .iter()
        .map(|key| ((*key).to_string(), TomlValue::Boolean(false)))
        .collect();
    overrides.extend(
        conversation_mode_default_feature_keys()
            .into_iter()
            .map(|key| (format!("features.{key}"), TomlValue::Boolean(false))),
    );
    overrides.push((
        "web_search".to_string(),
        TomlValue::String("disabled".to_string()),
    ));
    overrides
}

/// Native home discovery only; no auth reads, refreshes or profile creation.
pub fn native_codex_home() -> std::io::Result<std::path::PathBuf> {
    find_codex_home().map(|home| home.to_path_buf())
}

pub(crate) fn validate_native_auth(bytes: &[u8]) -> anyhow::Result<()> {
    let native: codex_login::AuthDotJson = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("native authentication source is invalid JSON or structure")
    })?;
    anyhow::ensure!(
        native.openai_api_key.is_some()
            || native.tokens.is_some()
            || native.agent_identity.is_some()
            || native.personal_access_token.is_some()
            || native.bedrock_api_key.is_some(),
        "native authentication source contains no credentials"
    );
    Ok(())
}

/// Offline native-auth import. The same profile lock as the app-server prevents
/// replacing live credentials; decoding errors never include credential bytes.
pub(crate) fn import_profile_credentials(
    instance: &polar_runtime_paths::InstancePathPlan,
    profile_id: &str,
    bytes: &str,
    overwrite: bool,
) -> anyhow::Result<polar_inhouse_contract::InhouseBackendAuthView> {
    use polar_runtime_paths::{RuntimeArea, RuntimeModule};
    use std::io::Write;
    anyhow::ensure!(bytes.len() <= 256 * 1024, "native auth document too large");
    let native: codex_login::AuthDotJson = serde_json::from_str(bytes)
        .map_err(|_| anyhow::anyhow!("invalid native Codex auth document"))?;
    anyhow::ensure!(
        native.openai_api_key.is_some()
            || native.tokens.is_some()
            || native.agent_identity.is_some()
            || native.personal_access_token.is_some()
            || native.bedrock_api_key.is_some(),
        "native auth document contains no credentials"
    );
    let paths = crate::profile_runtime::CodexProfilePaths::resolve(instance, profile_id)?;
    let run = instance.open_directory(RuntimeModule::Codex, RuntimeArea::Run)?;
    let lock_name = format!("{profile_id}.lock");
    // create_new uses private permissions and refuses symlinks.
    if !run.path().join(&lock_name).exists() {
        run.create_new(&lock_name, b"")?;
    }
    let metadata = std::fs::symlink_metadata(&paths.runtime_lock)?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "invalid profile lock"
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&paths.runtime_lock)?;
    lock.try_lock().map_err(|_| {
        anyhow::anyhow!("Codex profile is active; stop it before offline auth import")
    })?;
    let home = instance.open_backend_directory(profile_id)?;
    let destination = home.path().join("auth.json");
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "invalid profile auth file"
        );
        if !overwrite {
            return Ok(polar_inhouse_contract::InhouseBackendAuthView::Imported { changed: false });
        }
    }
    let serialized = serde_json::to_vec(&native)
        .map_err(|_| anyhow::anyhow!("cannot encode native auth document"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".auth-")
        .tempfile_in(home.path())?;
    staged.write_all(&serialized)?;
    staged.as_file().sync_all()?;
    staged.persist(&destination)?;
    Ok(polar_inhouse_contract::InhouseBackendAuthView::Imported { changed: true })
}
const IN_PROCESS_OPT_OUT_NOTIFICATION_METHODS: &[&str] = &[
    "item/reasoning/textDelta",
    "item/reasoning/summaryTextDelta",
];

pub(crate) fn start_in_process_client(
    cfg: &CodexAgentConfig,
) -> impl std::future::Future<Output = anyhow::Result<InProcessAppServerClient>> + Send + '_ {
    // Native config loading makes this future large. Keep it out of every
    // caller's state machine: nested debug poll frames otherwise exhaust a
    // normal block_on caller's stack during upstream startup config reloads.
    // This only changes storage, not scheduling, cancellation or runtime policy.
    Box::pin(start_in_process_client_inner(cfg))
}

async fn start_in_process_client_inner(
    cfg: &CodexAgentConfig,
) -> anyhow::Result<InProcessAppServerClient> {
    let (mut config, startup_cli_overrides) = build_validated_in_process_config(cfg, None).await?;
    let loader_overrides = profile_loader_overrides(cfg)?;
    let embedded_network_policy = EmbeddedNetworkPolicy::load(&loader_overrides).await;
    embedded_network_policy.activate(&mut config);
    let environment_http_client =
        embedded_network_policy.bind(HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault));
    let state_db = codex_core::init_state_db(&config)
        .await
        .with_context(|| {
            format!(
                "initialize Codex state database for profile {} at {}",
                cfg.profile
                    .as_ref()
                    .map_or("native", |paths| paths.profile_id.as_str()),
                config.sqlite.home().display()
            )
        })
        .context(crate::thread_lifecycle::LifecycleFailure::Database)?;

    let socket = cfg.socket_path.clone().map(TryInto::try_into).transpose()?;
    let result = Box::pin(InProcessAppServerClient::start_with_socket(
        InProcessClientStartArgs {
            required_human_approval: false,
            arg0_paths: Arg0DispatchPaths::default(),
            config: Arc::new(config),
            cli_overrides: startup_cli_overrides,
            loader_overrides,
            strict_config: false,
            cloud_config_bundle: in_process_requirements(cfg),
            embedded_network_policy,
            feedback: CodexFeedback::new(),
            log_db: None,
            state_db: Some(Arc::clone(&state_db)),
            environment_manager: Arc::new(EnvironmentManager::without_environments(
                environment_http_client,
            )),
            config_warnings: Vec::new(),
            session_source: SessionSource::Custom("polar-inhouse".to_string()),
            enable_codex_api_key_env: cfg.profile.is_none(),
            client_name: "polar-inhouse-agent".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: in_process_opt_out_notification_methods(),
            channel_capacity: IN_PROCESS_NOTIFICATION_CHANNEL_CAPACITY,
        },
        socket,
    ))
    .await
    .context("start codex in-process app-server");
    if result.is_err() {
        state_db.close().await;
    }
    result
}

/// Keep conversation restrictions in the upstream requirements layer, including
/// subsequent app-server config reloads. This is an in-memory host policy; it
/// performs no cloud request and does not replace system/user configuration.
fn in_process_requirements(cfg: &CodexAgentConfig) -> CloudConfigBundleLoader {
    if !cfg.codex_mode.is_conversation() {
        return CloudConfigBundleLoader::default();
    }
    CloudConfigBundleLoader::new(async {
        Ok(Some(CloudConfigBundle {
            requirements_toml: CloudRequirementsTomlBundle {
                enterprise_managed: vec![CloudRequirementsFragment {
                    id: "polar-conversation".to_string(),
                    name: "Polar conversation execution boundary".to_string(),
                    contents: "[features]\nunified_exec = false\n".to_string(),
                }],
            },
            ..Default::default()
        }))
    })
}

fn profile_loader_overrides(cfg: &CodexAgentConfig) -> anyhow::Result<LoaderOverrides> {
    let Some(paths) = &cfg.profile else {
        return Ok(LoaderOverrides::default());
    };
    let isolated = paths.runtime_lock.with_extension("config-inputs");
    Ok(LoaderOverrides {
        ignore_user_config: true,
        ignore_project_config: true,
        ignore_user_and_project_exec_policy_rules: true,
        system_config_path: Some(isolated.join("config.toml")),
        managed_config_path: Some(isolated.join("managed_config.toml")),
        system_requirements_path: Some(isolated.join("requirements.toml")),
        #[cfg(target_os = "macos")]
        managed_preferences_base64: Some(String::new()),
        #[cfg(target_os = "macos")]
        macos_managed_config_requirements_base64: Some(String::new()),
        ..LoaderOverrides::default()
    })
}

async fn load_database_profile_config(cfg: &CodexAgentConfig) -> anyhow::Result<Config> {
    crate::profile_config::validate(cfg)?;
    let paths = cfg
        .profile
        .as_ref()
        .context("Missing managed Codex profile")?;
    tokio::fs::create_dir_all(&paths.home).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&paths.home, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let mut config = ConfigBuilder::default()
        .codex_home(paths.home.clone())
        .fallback_cwd(Some(paths.home.clone()))
        .loader_overrides(profile_loader_overrides(cfg)?)
        .cli_overrides(managed_profile_overrides(cfg)?)
        .cloud_config_bundle(in_process_requirements(cfg))
        .build()
        .await
        .context("load native Codex backend profile")?;
    anyhow::ensure!(
        config.cli_auth_credentials_store_mode
            == codex_config::types::AuthCredentialsStoreMode::File,
        "Managed Codex profiles require cli_auth_credentials_store = 'file'"
    );
    anyhow::ensure!(
        config.mcp_oauth_credentials_store_mode
            == codex_config::types::OAuthCredentialsStoreMode::File,
        "Managed Codex profiles require mcp_oauth_credentials_store = 'file'"
    );
    for path in [config.sqlite.home(), config.log_dir.as_path()] {
        anyhow::ensure!(
            path.starts_with(&paths.home)
                && !path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
            "Codex runtime data must remain inside profile home: {}",
            path.display()
        );
    }
    if cfg.codex_mode.is_conversation() {
        apply_conversation_mode_config(cfg, &mut config)?;
    }
    validate_in_process_config_connector(cfg, &config)?;
    Ok(config)
}

pub(crate) async fn resolve_native_profile_settings(
    cfg: &mut CodexAgentConfig,
) -> anyhow::Result<()> {
    // Validate the database-derived snapshot; never read or overwrite it from files.
    crate::profile_config::validate(cfg)?;
    Ok(())
}

pub(crate) fn managed_profile_overrides(cfg: &CodexAgentConfig) -> anyhow::Result<Vec<(String, TomlValue)>> {
    crate::profile_config::validate(cfg)?;
    let mut values = build_in_process_cli_overrides(cfg);
    if let Some(paths) = &cfg.profile {
        let home = crate::auth_source::resolve(cfg.auth_home.as_deref(), &paths.home)?;
        values.push((
            "auth_home".into(),
            TomlValue::String(home.to_string_lossy().into_owned()),
        ));
    }
    if let Some(model) = &cfg.model {
        values.push(("model".into(), TomlValue::String(model.clone())));
    }
    if let Some(effort) = &cfg.effort {
        values.push((
            "model_reasoning_effort".into(),
            TomlValue::String(effort.clone()),
        ));
    }
    if let Some(policy) = &cfg.approval_policy {
        values.push(("approval_policy".into(), TomlValue::String(policy.clone())));
    }
    if let Some(mode) = startup_sandbox_mode_from_runtime_policy(cfg.sandbox_policy.as_ref()) {
        values.push(("sandbox_mode".into(), TomlValue::try_from(mode)?));
    }
    for key in ["cli_auth_credentials_store", "mcp_oauth_credentials_store"] {
        values.push((key.into(), TomlValue::String("file".into())));
    }
    values.extend(conversation_mode_cli_overrides(cfg));
    Ok(values)
}

fn build_validated_in_process_config<'a>(
    cfg: &'a CodexAgentConfig,
    codex_home: Option<&'a Path>,
) -> impl std::future::Future<Output = anyhow::Result<(Config, Vec<(String, TomlValue)>)>> + Send + 'a
{
    // Construct the large native loader before polling its caller. Boxing
    // inside an async body still leaves construction temporaries in that
    // body's debug poll frame during the later app-server config reload.
    Box::pin(build_validated_in_process_config_inner(cfg, codex_home))
}

async fn build_validated_in_process_config_inner(
    cfg: &CodexAgentConfig,
    codex_home: Option<&Path>,
) -> anyhow::Result<(Config, Vec<(String, TomlValue)>)> {
    if cfg.conversation_model_metadata.is_some() {
        crate::profile_config::validate(cfg)?;
    }
    if cfg.profile.is_some() {
        let config = load_database_profile_config(cfg).await?;
        return Ok((config, managed_profile_overrides(cfg)?));
    }
    let mut startup_cli_overrides = build_in_process_cli_overrides(cfg);
    startup_cli_overrides.extend(conversation_mode_cli_overrides(cfg));
    let mut config = build_in_process_config(
        cfg.config_profile.as_deref(),
        cfg.model_provider.as_deref(),
        cfg.provider_base_url.as_deref(),
        cfg.oss_mode.as_deref(),
        cfg.lmstudio_host.as_deref(),
        cfg.approval_policy.as_deref(),
        cfg.sandbox_policy.as_ref(),
        &startup_cli_overrides,
        codex_home,
        &in_process_requirements(cfg),
    )
    .await?;
    if cfg.codex_mode.is_conversation() {
        let enabled_from_non_default_config = config.features.enabled_features();
        if !enabled_from_non_default_config.is_empty() {
            for feature in enabled_from_non_default_config {
                set_cli_override(
                    &mut startup_cli_overrides,
                    format!("features.{}", feature.key()),
                    TomlValue::Boolean(false),
                );
            }
            config = build_in_process_config(
                cfg.config_profile.as_deref(),
                cfg.model_provider.as_deref(),
                cfg.provider_base_url.as_deref(),
                cfg.oss_mode.as_deref(),
                cfg.lmstudio_host.as_deref(),
                cfg.approval_policy.as_deref(),
                cfg.sandbox_policy.as_ref(),
                &startup_cli_overrides,
                codex_home,
                &in_process_requirements(cfg),
            )
            .await?;
        }
        apply_conversation_mode_config(cfg, &mut config)?;
    }
    validate_in_process_config_connector(cfg, &config)?;
    Ok((config, startup_cli_overrides))
}

pub(crate) fn conversation_mode_default_feature_keys() -> Vec<&'static str> {
    Features::with_defaults()
        .enabled_features()
        .into_iter()
        .map(|feature| feature.key())
        .collect()
}

fn set_cli_override(overrides: &mut Vec<(String, TomlValue)>, key: String, value: TomlValue) {
    if let Some((_, current)) = overrides
        .iter_mut()
        .find(|(current_key, _)| current_key == &key)
    {
        *current = value;
    } else {
        overrides.push((key, value));
    }
}

/// Validate with the pinned upstream type; no model lookup or auth access.
pub fn validate_conversation_model_metadata(
    model: &str,
    metadata: &JsonValue,
) -> anyhow::Result<()> {
    parse_conversation_model_metadata(model, metadata).map(|_| ())
}

fn parse_conversation_model_metadata(
    model: &str,
    metadata: &JsonValue,
) -> anyhow::Result<ModelInfo> {
    // Do not echo arbitrary metadata (which may contain instructions) in errors.
    let info: ModelInfo = serde_json::from_value(metadata.clone()).map_err(|_| {
        anyhow::anyhow!("conversation_model_metadata must be a native Codex ModelInfo object")
    })?;
    anyhow::ensure!(
        !model.trim().is_empty() && info.slug == model,
        "conversation_model_metadata slug must exactly match the configured model"
    );
    Ok(info)
}

fn apply_conversation_mode_config(
    cfg: &CodexAgentConfig,
    config: &mut Config,
) -> anyhow::Result<()> {
    let fixed_model = cfg
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .context("codex conversation mode requires an explicit fixed model")?;
    let mut catalog = if let Some(metadata) = &cfg.conversation_model_metadata {
        ModelsResponse {
            models: vec![parse_conversation_model_metadata(
                cfg.model.as_deref().unwrap_or_default(),
                metadata,
            )?],
        }
    } else {
        let mut catalog = bundled_models_response().context("load bundled Codex model catalog")?;
        catalog.models.retain(|model| model.slug == fixed_model);
        catalog
    };
    anyhow::ensure!(
        catalog.models.len() == 1,
        "codex conversation mode model '{fixed_model}' is absent from the bundled model catalog"
    );
    let model = &mut catalog.models[0];
    model.tool_mode = Some(ToolMode::Direct);
    model.supports_search_tool = false;
    model.experimental_supported_tools.clear();
    model.apply_patch_tool_type = None;
    model.shell_type = codex_protocol::openai_models::ConfigShellToolType::Disabled;
    model.multi_agent_version = Some(codex_protocol::protocol::MultiAgentVersion::Disabled);
    model.node_repl_disabled = true;
    model.include_skills_usage_instructions = false;
    model.include_plugin_usage_instructions = false;
    model.include_apps_usage_instructions = false;

    config
        .features
        .set(Features::default())
        .context("disable Codex features for conversation mode")?;
    anyhow::ensure!(
        config.features.enabled_features().is_empty(),
        "codex conversation mode conflicts with a managed requirement that enables Codex features"
    );
    config.mcp_servers = Constrained::allow_only(HashMap::new());
    config.non_prefixed_mcp_tool_servers = None;
    config.web_search_mode = Constrained::allow_only(WebSearchMode::Disabled);
    config.web_search_config = None;
    config.model_catalog = Some(catalog);

    config.base_instructions = None;
    config.developer_instructions = None;
    config.compact_prompt = None;
    config.personality = None;
    config.notify = None;
    config.include_permissions_instructions = false;
    config.include_apps_instructions = false;
    config.include_collaboration_mode_instructions = false;
    config.include_skill_instructions = false;
    config.cloud_skill_enabled = false;
    config.orchestrator_mcp_enabled = false;
    config.include_environment_context = false;
    config.project_doc_max_bytes = 0;
    config.project_doc_fallback_filenames.clear();
    config.experimental_request_user_input_enabled = false;
    config.update_plan_enabled = false;
    config.agents_enabled = false;
    config.agent_max_threads = None;
    Ok(())
}

fn validate_in_process_config_connector(
    cfg: &CodexAgentConfig,
    config: &Config,
) -> anyhow::Result<()> {
    ensure_resolved_connector_allowed(
        &cfg.connector_deny_policy,
        config.model_provider.base_url.as_deref(),
    )
    .context("validate resolved codex in-process provider connector")
}

fn in_process_opt_out_notification_methods() -> Vec<String> {
    IN_PROCESS_OPT_OUT_NOTIFICATION_METHODS
        .iter()
        .map(|method| (*method).to_string())
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn build_in_process_config(
    config_profile: Option<&str>,
    model_provider: Option<&str>,
    provider_base_url: Option<&str>,
    oss_mode: Option<&str>,
    lmstudio_host: Option<&str>,
    approval_policy: Option<&str>,
    sandbox_policy: Option<&JsonValue>,
    cli_overrides: &[(String, TomlValue)],
    codex_home: Option<&Path>,
    requirements: &CloudConfigBundleLoader,
) -> anyhow::Result<Config> {
    let normalized_profile = config_profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let normalized_provider = model_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    match build_base_in_process_config(
        normalized_profile.clone(),
        normalized_provider,
        approval_policy,
        sandbox_policy,
        cli_overrides,
        codex_home,
        requirements,
    )
    .await
    {
        Ok(config) => Ok(config),
        Err(err) => {
            let self_contained_provider = effective_model_provider(model_provider, oss_mode)
                .is_some()
                && effective_provider_base_url(provider_base_url, lmstudio_host).is_some();
            if let Some(profile) = normalized_profile.as_deref() {
                if !self_contained_provider {
                    return Err(err);
                }
                tracing::warn!(
                    %profile,
                    error = %err,
                    "failed to load requested codex config profile for in-process app-server; retrying without profile"
                );
                build_base_in_process_config(
                    None,
                    effective_model_provider(model_provider, oss_mode),
                    approval_policy,
                    sandbox_policy,
                    cli_overrides,
                    codex_home,
                    requirements,
                )
                .await
            } else if self_contained_provider {
                build_base_in_process_config(
                    None,
                    effective_model_provider(model_provider, oss_mode),
                    approval_policy,
                    sandbox_policy,
                    cli_overrides,
                    codex_home,
                    requirements,
                )
                .await
            } else {
                Err(err)
            }
        }
    }
}

async fn build_base_in_process_config(
    config_profile: Option<String>,
    model_provider: Option<String>,
    approval_policy: Option<&str>,
    sandbox_policy: Option<&JsonValue>,
    cli_overrides: &[(String, TomlValue)],
    codex_home: Option<&Path>,
    requirements: &CloudConfigBundleLoader,
) -> anyhow::Result<Config> {
    let overrides = ConfigOverrides {
        model_provider,
        approval_policy: parse_startup_approval_policy(approval_policy)?,
        sandbox_mode: startup_sandbox_mode_from_runtime_policy(sandbox_policy),
        ..Default::default()
    };
    let codex_home = match codex_home {
        Some(codex_home) => codex_home.to_path_buf(),
        None => find_codex_home()
            .context("resolve Codex home for in-process app-server")?
            .to_path_buf(),
    };
    let user_config_profile = config_profile.map(|profile| profile.parse()).transpose()?;
    let user_config_path = user_config_profile
        .as_ref()
        .map(|profile| resolve_profile_v2_config_path(&codex_home, profile));
    let loader_overrides = LoaderOverrides {
        user_config_profile,
        user_config_path,
        ..Default::default()
    };
    ConfigBuilder::default()
        .codex_home(codex_home)
        .cli_overrides(cli_overrides.to_vec())
        .harness_overrides(overrides)
        .loader_overrides(loader_overrides)
        .cloud_config_bundle(requirements.clone())
        .build()
        .await
        .context("load codex config for in-process app-server")
}

fn parse_startup_approval_policy(policy: Option<&str>) -> anyhow::Result<Option<AskForApproval>> {
    policy
        .map(|policy| serde_json::from_value(JsonValue::String(policy.to_string())))
        .transpose()
        .context("parse codex in-process startup approval policy")
}

fn startup_sandbox_mode_from_runtime_policy(policy: Option<&JsonValue>) -> Option<SandboxMode> {
    let policy = policy?;
    let policy_type = policy.get("type").and_then(JsonValue::as_str)?;
    match policy_type {
        "readOnly" => Some(SandboxMode::ReadOnly),
        "workspaceWrite" => Some(SandboxMode::WorkspaceWrite),
        "dangerFullAccess" => Some(SandboxMode::DangerFullAccess),
        _ => None,
    }
}

fn build_in_process_cli_overrides(cfg: &CodexAgentConfig) -> Vec<(String, TomlValue)> {
    let mut overrides = Vec::new();

    if let Some(provider_id) =
        effective_model_provider(cfg.model_provider.as_deref(), cfg.oss_mode.as_deref())
    {
        overrides.push((
            "model_provider".to_string(),
            TomlValue::String(provider_id.clone()),
        ));
        if let Some(base_url) = effective_provider_base_url(
            cfg.provider_base_url.as_deref(),
            cfg.lmstudio_host.as_deref(),
        ) {
            overrides.push((
                format!("model_providers.{provider_id}.name"),
                TomlValue::String(provider_id.clone()),
            ));
            overrides.push((
                format!("model_providers.{provider_id}.base_url"),
                TomlValue::String(base_url),
            ));
            overrides.push((
                format!("model_providers.{provider_id}.wire_api"),
                TomlValue::String("responses".to_string()),
            ));
            overrides.push((
                format!("model_providers.{provider_id}.requires_openai_auth"),
                TomlValue::Boolean(false),
            ));
        }
    }

    if cfg.memory_tool_enabled
        || cfg.generate_memories.is_some()
        || cfg.use_memories.is_some()
        || cfg.min_rollout_idle_hours.is_some()
        || cfg.memory_extract_model.is_some()
        || cfg.memory_consolidation_model.is_some()
    {
        overrides.push(("features.memories".to_string(), TomlValue::Boolean(true)));
        if let Some(generate_memories) = cfg.generate_memories {
            overrides.push((
                "memories.generate_memories".to_string(),
                TomlValue::Boolean(generate_memories),
            ));
        }
        if let Some(use_memories) = cfg.use_memories {
            overrides.push((
                "memories.use_memories".to_string(),
                TomlValue::Boolean(use_memories),
            ));
        }
        if let Some(min_rollout_idle_hours) = cfg.min_rollout_idle_hours {
            overrides.push((
                "memories.min_rollout_idle_hours".to_string(),
                TomlValue::Integer(min_rollout_idle_hours),
            ));
        }
        if let Some(extract_model) = cfg.memory_extract_model.as_deref() {
            overrides.push((
                "memories.extract_model".to_string(),
                TomlValue::String(extract_model.to_string()),
            ));
        }
        if let Some(consolidation_model) = cfg.memory_consolidation_model.as_deref() {
            overrides.push((
                "memories.consolidation_model".to_string(),
                TomlValue::String(consolidation_model.to_string()),
            ));
        }
    }

    overrides
}


