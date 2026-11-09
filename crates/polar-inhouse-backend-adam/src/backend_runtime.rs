//! Process-wide Adam backend composition.
//!
//! `AdamBackendRuntime` is the single lifecycle owner for Adam engine control
//! and projection resources. Full-host adapters load this runtime as one
//! opaque backend resource; individual Inhouse Polar profiles only request a
//! runner and never construct managers, stores, or slot state.

use crate::{
    compaction_physical_port::AdamCompactionPhysicalPort, normalize_connector_url,
    parse_adam_agent_config, AdamAgentConfig, AdamAgentRunner, AdamEngineStore,
    AdamInferenceEngineManager, AdamManagedLlamaEndpoint, AdamOAuthCredentialResolver,
    AdamRuntimeProjectionStore, LlamaAdamEngineOperations, LlamaTransport,
};
use anyhow::{bail, Context, Result};
use polar_inhouse_contract::{InhouseCompactionPhysicalPort, InhouseConnectorDenyPolicy};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

pub const ADAM_MANAGED_ENDPOINT_DECLARATIONS_FILE: &str = "adam-managed-endpoints.json";
pub const ADAM_MANAGED_ENDPOINT_DECLARATIONS_SCHEMA_VERSION: u32 = 1;

/// Host-owned physical endpoint declaration. Runtime identity and compatibility
/// hashes are intentionally absent because the live probe is their authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdamManagedEndpointDeclaration {
    pub endpoint_id: String,
    pub connector_base_url: String,
    pub transport: String,
    pub slot_offset: i64,
    pub slot_count: i64,
    pub model_id: String,
    pub adam_managed_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdamManagedEndpointDeclarations {
    pub schema_version: u32,
    pub managed_endpoints: Vec<AdamManagedEndpointDeclaration>,
}

pub fn parse_adam_managed_endpoint_declarations(
    input: &str,
) -> Result<Vec<AdamManagedLlamaEndpoint>> {
    let declarations: AdamManagedEndpointDeclarations =
        serde_json::from_str(input).context("parse Adam host endpoint declarations")?;
    if declarations.schema_version != ADAM_MANAGED_ENDPOINT_DECLARATIONS_SCHEMA_VERSION {
        bail!(
            "unsupported Adam managed endpoint declaration schema_version {}; expected {}",
            declarations.schema_version,
            ADAM_MANAGED_ENDPOINT_DECLARATIONS_SCHEMA_VERSION
        );
    }
    Ok(declarations
        .managed_endpoints
        .into_iter()
        .map(|declaration| AdamManagedLlamaEndpoint {
            endpoint_id: declaration.endpoint_id,
            runtime_instance_id: String::new(),
            connector_base_url: declaration.connector_base_url,
            transport: declaration.transport,
            slot_offset: declaration.slot_offset,
            slot_count: declaration.slot_count,
            model_id: declaration.model_id,
            tokenizer_hash: String::new(),
            context_params_hash: String::new(),
            adam_managed_only: declaration.adam_managed_only,
        })
        .collect())
}

/// Opaque loader configuration for one process-wide Adam runtime.
///
/// Managed endpoints are runtime resources. They are deliberately separate
/// from Inhouse Polar profiles, which may select a connector but must not own
/// physical slot or snapshot identity.
#[derive(Debug, Clone)]
pub struct AdamBackendRuntimeConfig {
    pub database_url: String,
    pub managed_endpoints: Vec<AdamManagedLlamaEndpoint>,
}

impl AdamBackendRuntimeConfig {
    pub fn durable(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
            managed_endpoints: Vec::new(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.database_url.trim().is_empty() {
            bail!("Adam backend runtime database_url must not be blank");
        }
        let mut endpoint_ids = HashSet::new();
        let mut connector_slots = HashSet::new();
        for endpoint in &self.managed_endpoints {
            if endpoint.endpoint_id.trim().is_empty() {
                bail!("Adam managed endpoint endpoint_id must not be blank");
            }
            if !endpoint_ids.insert(endpoint.endpoint_id.clone()) {
                bail!(
                    "Adam managed endpoint_id '{}' appears more than once",
                    endpoint.endpoint_id
                );
            }
            if endpoint.connector_base_url.trim().is_empty() {
                bail!(
                    "Adam managed endpoint '{}' connector_base_url must not be blank",
                    endpoint.endpoint_id
                );
            }
            if endpoint.transport != LlamaTransport::LlamacppSlots.as_str() {
                bail!(
                    "Adam managed endpoint '{}' transport must be llamacpp_slots",
                    endpoint.endpoint_id
                );
            }
            let Some(slot_range) = endpoint.slot_range() else {
                bail!(
                    "Adam managed endpoint '{}' slot_offset/slot_count is invalid; slot_count must be greater than zero",
                    endpoint.endpoint_id
                );
            };
            if u32::try_from(endpoint.slot_offset).is_err()
                || u32::try_from(endpoint.slot_count).is_err()
            {
                bail!(
                    "Adam managed endpoint '{}' slot range exceeds the native connector range",
                    endpoint.endpoint_id
                );
            }
            if endpoint.model_id.trim().is_empty() {
                bail!(
                    "Adam managed endpoint '{}' requires a nonblank model_id",
                    endpoint.endpoint_id
                );
            }
            if !endpoint.adam_managed_only {
                bail!(
                    "Adam managed endpoint '{}' must set adam_managed_only=true",
                    endpoint.endpoint_id
                );
            }
            let normalized_url = normalize_connector_url(&endpoint.connector_base_url);
            for slot_id in slot_range {
                if !connector_slots.insert((normalized_url.clone(), slot_id)) {
                    bail!(
                        "Adam managed endpoint '{}' overlaps another managed slot at {} slot {}",
                        endpoint.endpoint_id,
                        normalized_url,
                        slot_id
                    );
                }
            }
        }
        Ok(())
    }
}

/// Adam-owned process resource shared by every Adam runner.
#[derive(Clone)]
pub struct AdamBackendRuntime {
    engine_store: Arc<dyn AdamEngineStore>,
    projection_store: Arc<dyn AdamRuntimeProjectionStore>,
    engine_manager: Arc<AdamInferenceEngineManager>,
}

impl std::fmt::Debug for AdamBackendRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdamBackendRuntime")
            .field("engine_store", &"<durable-sqlite-store>")
            .field("projection_store", &"<durable-sqlite-store>")
            .field("engine_manager", &"<shared-manager>")
            .finish()
    }
}

impl AdamBackendRuntime {
    /// Composes backend semantics from stores already opened by a storage
    /// adapter. Adam never selects a driver or opens a database itself.
    pub async fn from_stores(
        config: AdamBackendRuntimeConfig,
        engine_store: Arc<dyn AdamEngineStore>,
        projection_store: Arc<dyn AdamRuntimeProjectionStore>,
    ) -> Result<Self> {
        config.validate()?;
        let declared_endpoint_ids = config
            .managed_endpoints
            .iter()
            .map(|endpoint| endpoint.endpoint_id.clone())
            .collect::<HashSet<_>>();
        let configured_endpoints = config.managed_endpoints;
        let engine_store_for_bootstrap = Arc::clone(&engine_store);
        tokio::task::spawn_blocking(move || -> Result<()> {
            for persisted in engine_store_for_bootstrap.endpoints() {
                if !declared_endpoint_ids.contains(persisted.endpoint_id.as_str()) {
                    let _ =
                        engine_store_for_bootstrap.remove_managed_endpoint(&persisted.endpoint_id);
                }
            }
            for endpoint in configured_endpoints {
                engine_store_for_bootstrap.upsert_managed_endpoint(endpoint);
            }
            Ok(())
        })
        .await
        .context("join Adam durable-store bootstrap")??;
        let engine_operations = Arc::new(LlamaAdamEngineOperations::default());
        let engine_manager = Arc::new(AdamInferenceEngineManager::with_operations(
            Arc::clone(&engine_store),
            engine_operations.clone(),
        ));
        Ok(Self {
            engine_store,
            projection_store,
            engine_manager,
        })
    }

    /// Returns the provider-neutral physical-compaction port while keeping all
    /// Adam manager, store, projection, slot, and KV types private.
    pub fn compaction_physical_port(
        &self,
        inhouse_polar_id: impl Into<String>,
        session_id: impl Into<String>,
        backend_config_json: &str,
    ) -> Result<Arc<dyn InhouseCompactionPhysicalPort>> {
        let inhouse_polar_id = inhouse_polar_id.into();
        let config = self.resolve_native_endpoint(parse_adam_agent_config(
            inhouse_polar_id.clone(),
            backend_config_json,
        )?)?;
        let native = config
            .backend
            .llama()
            .filter(|llama| llama.transport == LlamaTransport::LlamacppSlots);
        let managed_endpoint_id = native.and_then(|llama| llama.managed_endpoint_id.clone());
        let enable_thinking = native.is_none_or(|llama| llama.llama_enable_thinking);
        Ok(Arc::new(AdamCompactionPhysicalPort::new(
            Arc::clone(&self.engine_manager),
            Arc::clone(&self.engine_store),
            Arc::clone(&self.projection_store),
            inhouse_polar_id,
            session_id,
            managed_endpoint_id,
            enable_thinking,
        )))
    }

    /// Projects only canonical connector reservations for shared backend
    /// isolation. Endpoint ids, models, slots, runtime ids, and compatibility
    /// hashes remain inside Adam.
    pub fn reserved_connector_policy(&self) -> Result<InhouseConnectorDenyPolicy> {
        let mut reserved_base_urls = self
            .engine_store
            .endpoints()
            .into_iter()
            .filter(|endpoint| endpoint.adam_managed_only)
            .map(|endpoint| {
                let connector_url = normalize_connector_url(&endpoint.connector_base_url);
                if connector_url.is_empty() {
                    bail!(
                        "Adam managed endpoint '{}' has a blank connector URL",
                        endpoint.endpoint_id
                    );
                }
                Ok(connector_url)
            })
            .collect::<Result<Vec<_>>>()?;
        reserved_base_urls.sort();
        reserved_base_urls.dedup();
        Ok(InhouseConnectorDenyPolicy { reserved_base_urls })
    }

    /// Builds the redacted operator report and attaches durable per-turn run
    /// evidence from the runtime projection store.
    pub async fn status_report(&self) -> Result<crate::AdamEngineStatusReport> {
        let mut report = self.engine_manager.status_report().await;
        report.recent_runs = self.projection_store.recent_run_reports(100).await?;
        Ok(report)
    }

    /// Creates a runner wired only to runtime-owned shared resources.
    pub fn build_runner(
        &self,
        config: AdamAgentConfig,
        oauth_credential_resolver: Option<Arc<dyn AdamOAuthCredentialResolver>>,
    ) -> Result<AdamAgentRunner> {
        let config = self.resolve_native_endpoint(config)?;
        let compatibility = self.native_endpoint_compatibility(&config)?;
        let projection_store: Arc<dyn AdamRuntimeProjectionStore> = self.projection_store.clone();
        let mut runner = AdamAgentRunner::new(config).with_projection_store(projection_store);
        if let Some(resolver) = oauth_credential_resolver {
            runner = runner.with_oauth_credential_resolver(resolver);
        }
        if let Some((endpoint_id, tokenizer_hash, context_params_hash)) = compatibility {
            runner = runner
                .with_engine_manager(Arc::clone(&self.engine_manager))
                .with_managed_endpoint_binding(endpoint_id)
                .with_engine_compatibility(tokenizer_hash, context_params_hash);
        }
        Ok(runner)
    }

    /// Resolves an opaque profile reference against the host-owned pool.
    /// Profiles never contribute connector URLs, models, or slot policy.
    fn resolve_native_endpoint(&self, mut config: AdamAgentConfig) -> Result<AdamAgentConfig> {
        let Some(llama) = config.backend.llama_mut() else {
            return Ok(config);
        };
        if llama.transport != LlamaTransport::LlamacppSlots {
            return Ok(config);
        }
        let endpoint_id = llama
            .managed_endpoint_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .context("Adam native profile requires an opaque managed_endpoint_id")?;
        let endpoint = self
            .engine_store
            .endpoints()
            .into_iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .with_context(|| {
                format!(
                    "Adam native profile references host-unadmitted managed endpoint '{endpoint_id}'"
                )
            })?;
        if !endpoint.adam_managed_only
            || endpoint.transport != LlamaTransport::LlamacppSlots.as_str()
        {
            bail!(
                "Adam native profile endpoint '{}' is not an Adam-exclusive native endpoint",
                endpoint.endpoint_id
            );
        }
        llama.base_url = normalize_connector_url(&endpoint.connector_base_url);
        llama.model.clone_from(&endpoint.model_id);
        llama.slot_offset = u32::try_from(endpoint.slot_offset)
            .context("host-owned Adam endpoint slot_offset exceeds native connector range")?;
        llama.slot_count = Some(
            u32::try_from(endpoint.slot_count)
                .context("host-owned Adam endpoint slot_count exceeds native connector range")?,
        );
        config.backend.model.clone_from(&endpoint.model_id);
        Ok(config)
    }

    fn native_endpoint_compatibility(
        &self,
        config: &AdamAgentConfig,
    ) -> Result<Option<(String, String, String)>> {
        let Some(llama) = config.backend.llama() else {
            return Ok(None);
        };
        if llama.transport != LlamaTransport::LlamacppSlots {
            return Ok(None);
        }
        let endpoint_id = llama
            .managed_endpoint_id
            .as_deref()
            .context("resolved Adam native runner is missing managed_endpoint_id")?;
        let endpoint = self
            .engine_store
            .endpoints()
            .into_iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .with_context(|| {
                format!(
                    "Adam native runner {} lost host endpoint binding {}",
                    config.inhouse_polar_id, endpoint_id
                )
            })?;
        Ok(Some((
            endpoint.endpoint_id,
            endpoint.tokenizer_hash,
            endpoint.context_params_hash,
        )))
    }
}
