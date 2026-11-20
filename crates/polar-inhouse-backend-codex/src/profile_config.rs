//! Managed settings are supplied by the persisted Polar Backend Profile.
use crate::CodexAgentConfig;
use anyhow::{ensure, Result};

pub(crate) fn validate(cfg: &CodexAgentConfig) -> Result<()> {
    ensure!(cfg.config_profile.is_none(), "managed Codex config_profile file selectors are unsupported; configure the Backend Profile in Bundle");
    if let Some(metadata) = &cfg.conversation_model_metadata {
        ensure!(
            cfg.codex_mode.is_conversation()
                && matches!(cfg.backend, crate::CodexRuntimeBackend::InProcess),
            "conversation_model_metadata requires Codex conversation mode and runtime_backend=in_process"
        );
        crate::validate_conversation_model_metadata(
            cfg.model.as_deref().unwrap_or_default(),
            metadata,
        )?;
    }
    if cfg.codex_mode.is_conversation() {
        ensure!(
            cfg.model.as_deref().is_some_and(|m| !m.trim().is_empty()),
            "conversation Backend Profile requires an explicit model in backend_config_json"
        );
    }
    Ok(())
}

/// Excludes actor and tool bindings: these remain independently thread-scoped.
pub(crate) fn snapshot(cfg: &CodexAgentConfig) -> serde_json::Value {
    serde_json::json!({
        "transport": format!("{:?}",cfg.backend), "mode": format!("{:?}", cfg.codex_mode), "cwd": cfg.cwd,
        "auth_home": cfg.auth_home, "socket_path": cfg.socket_path,
        "model": cfg.model, "effort": cfg.effort,
        "conversation_model_metadata": cfg.conversation_model_metadata,
        "provider": cfg.model_provider, "endpoint": cfg.provider_base_url,
        "oss": cfg.oss_mode, "lmstudio": cfg.lmstudio_host,
        "approval": cfg.approval_policy, "sandbox": cfg.sandbox_policy,
        "memory": cfg.memory_tool_enabled, "generate": cfg.generate_memories,
        "use": cfg.use_memories, "idle": cfg.min_rollout_idle_hours,
        "extract": cfg.memory_extract_model, "consolidate": cfg.memory_consolidation_model,
    })
}
