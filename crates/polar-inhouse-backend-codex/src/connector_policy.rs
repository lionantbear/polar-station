use crate::codex::{effective_model_provider, effective_provider_base_url, CodexAgentConfig};
use anyhow::{bail, Context, Result};
use polar_inhouse_contract::InhouseConnectorDenyPolicy;

pub(crate) fn canonical_connector_base_url(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let url = reqwest::Url::parse(value).context("parse backend connector URL")?;
    let scheme = url.scheme().to_ascii_lowercase();
    anyhow::ensure!(
        matches!(scheme.as_str(), "http" | "https"),
        "backend connector URL must use http or https"
    );
    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .context("backend connector URL requires a host")?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let port = url
        .port_or_known_default()
        .context("backend connector URL requires an effective port")?;
    Ok(Some(format!("{scheme}://{host}:{port}")))
}

pub(crate) fn ensure_resolved_connector_allowed(
    policy: &InhouseConnectorDenyPolicy,
    resolved_base_url: Option<&str>,
) -> Result<()> {
    if policy.reserved_base_urls.is_empty() {
        return Ok(());
    }
    let Some(candidate) = resolved_base_url else {
        return Ok(());
    };
    let candidate = canonical_connector_base_url(candidate)?
        .context("resolved backend connector URL must not be blank")?;
    for reserved in &policy.reserved_base_urls {
        let reserved = canonical_connector_base_url(reserved)?
            .context("host-reserved backend connector URL must not be blank")?;
        if candidate == reserved {
            bail!("resolved Codex model provider uses a host-reserved backend connector");
        }
    }
    Ok(())
}

pub(crate) fn validate_socket_connector_policy(cfg: &CodexAgentConfig) -> Result<()> {
    if cfg.connector_deny_policy.reserved_base_urls.is_empty() {
        return Ok(());
    }
    let provider = effective_model_provider(cfg.model_provider.as_deref(), cfg.oss_mode.as_deref());
    let base_url = effective_provider_base_url(
        cfg.provider_base_url.as_deref(),
        cfg.lmstudio_host.as_deref(),
    );
    if provider.is_none() || base_url.is_none() {
        bail!(
            "cannot verify the effective Codex socket model-provider connector against host-reserved connectors; configure an explicit model_provider and provider_base_url or use the in-process backend"
        );
    }
    ensure_resolved_connector_allowed(&cfg.connector_deny_policy, base_url.as_deref())
}


