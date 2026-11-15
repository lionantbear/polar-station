//! Adam-exclusive llama endpoint, slot, cache, and prepared-prefix control.

use crate::cache_scheduler::{
    AdamCacheResident, AdamCacheScheduleRequest, AdamCacheScheduler, CacheAffinityKey,
};
use crate::engine_store::{
    AdamEngineCompletionCommit, AdamEngineCompletionEvidence, AdamEngineStore,
    AdamKvSnapshotRecord, AdamLiveSlotResidencyRecord, AdamManagedLlamaEndpoint,
    AdamPreparedCompactSlot, AdamPromotedPreparedLookup, AdamSlotLeaseCasRequest,
    AdamSnapshotCompatibility, AdamTurnSlotReservationRequest,
};
use crate::llama::{LlamaWireProjection, LlamaWireProjectionRequest};
use async_trait::async_trait;
use base64::Engine as _;
use polar_inhouse_contract::InhouseTurnControl;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_SLOT_LEASE_TTL_MS: i64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdamEngineAllocationWaitConfig {
    pub max_wait_ms: i64,
    pub poll_interval_ms: i64,
}

impl Default for AdamEngineAllocationWaitConfig {
    fn default() -> Self {
        Self {
            // Longer than the default protected-activity TTL, but still below
            // the normal five-minute provider request budget.
            max_wait_ms: 180_000,
            poll_interval_ms: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamEngineTurnPriority {
    ActiveThread,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEngineControlIntent {
    /// Opaque runtime-owned binding for the admitted managed endpoint. Inhouse
    /// semantic state never owns slot ranges; the backend runtime resolves the
    /// profile resource to this identifier before constructing a runner.
    #[serde(default)]
    pub managed_endpoint_id: String,
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub projection_id: String,
    pub exact_prefix_hash: String,
    pub model_id: String,
    pub tokenizer_hash: String,
    #[serde(default)]
    pub connector_base_url: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub context_params_hash: String,
    #[serde(default)]
    pub context_compaction_epoch: i64,
    pub priority: AdamEngineTurnPriority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamSlotLease {
    pub lease_id: String,
    pub fencing_token: String,
    pub endpoint_id: String,
    pub slot_id: i64,
    pub runtime_instance_id: String,
    pub lease_generation: i64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdamCachePlan {
    UseLiveSlot {
        residency: AdamLiveSlotResidencyRecord,
    },
    LoadSnapshot {
        snapshot: AdamKvSnapshotRecord,
    },
    RewarmPrefix {
        exact_prefix_hash: String,
    },
    ColdPrefill {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEngineControlEvidence {
    pub endpoint_id: String,
    pub slot_id: i64,
    pub runtime_instance_id: String,
    pub cache_plan: String,
    pub exact_prefix_hash: String,
    pub allocation_reason: String,
    #[serde(default)]
    pub eviction_decision: String,
    #[serde(default)]
    pub protected_activity_ttl_ms: i64,
    #[serde(default)]
    pub scheduling_priority: String,
    #[serde(default)]
    pub scheduler_request_id: String,
    #[serde(default)]
    pub initial_wait_queue_position: Option<usize>,
    #[serde(default)]
    pub initial_wait_queue_depth: Option<usize>,
    #[serde(default)]
    pub wait_observation_count: u32,
    #[serde(default)]
    pub waited_ms: i64,
    #[serde(default)]
    pub wait_limit_ms: i64,
    #[serde(default)]
    pub admitted_after_wait: bool,
    #[serde(default)]
    pub evicted_active_residency: bool,
    #[serde(default)]
    pub snapshot_before_eviction: bool,
    #[serde(default)]
    pub previous_runtime_instance_id: Option<String>,
    #[serde(default)]
    pub runtime_generation_changed: bool,
    #[serde(default)]
    pub old_runtime_invalidated: bool,
    #[serde(default)]
    pub cache_epoch_transition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEngineAllocation {
    pub intent: AdamEngineControlIntent,
    pub endpoint: AdamManagedLlamaEndpoint,
    pub lease: AdamSlotLease,
    pub cache_plan: AdamCachePlan,
    pub evidence: AdamEngineControlEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamLlamaEngineProbe {
    pub runtime_instance_id: String,
    pub llama_build_fingerprint: String,
    /// Physical tokenizer/model fingerprint derived from the live `/props`
    /// response rather than from an Inhouse Polar profile.
    #[serde(default)]
    pub tokenizer_hash: String,
    /// Physical context-shaping fingerprint derived from the complete live
    /// `/props` response.
    #[serde(default)]
    pub context_params_hash: String,
    /// Verified physical context capacity reported by the live `/props`
    /// response. Zero means unavailable and must fail admission closed.
    #[serde(default)]
    pub max_context_tokens: i64,
    pub total_slots: u32,
    pub idle_slot_ids: Vec<i64>,
    pub busy_slot_ids: Vec<i64>,
    pub supports_snapshot_save: bool,
    pub supports_snapshot_restore: bool,
    pub supports_slot_erase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamSnapshotSaveResult {
    pub n_saved: i64,
    pub n_written: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamSnapshotRestoreResult {
    pub n_restored: i64,
    pub n_read: i64,
}

impl AdamEngineAllocation {
    pub fn provider_slot_id(&self) -> i64 {
        self.lease.slot_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamPreparedCompactIntent {
    pub managed_endpoint_id: String,
    pub session_id: String,
    pub thread_id: String,
    pub candidate_compaction_id: String,
    pub source_digest_before: String,
    pub checkpoint_source_ref: String,
    pub exact_prefix_hash: String,
    /// Exact rendered prefix bytes (as UTF-8 text) that the physical engine
    /// operation must warm. The manager verifies this payload against
    /// `exact_prefix_hash` before any reservation, operation, or metadata write.
    #[serde(default)]
    pub exact_prefix_payload: String,
    pub target_context_compaction_epoch: i64,
    pub model_id: String,
    pub tokenizer_hash: String,
    pub context_params_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AdamPreparedCompactResult {
    Prepared(Box<AdamPreparedCompactSlot>),
    NoSpareIdleSlot { active_slot_ids: Vec<i64> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AdamPreparedCompactPromotion {
    Promoted(AdamPreparedCompactSlot),
    Discarded {
        prepared: Box<AdamPreparedCompactSlot>,
        reason: String,
        cache_plan: AdamCachePlan,
    },
    Missing {
        candidate_compaction_id: String,
        cache_plan: AdamCachePlan,
    },
}

/// Physical engine-control operations. Metadata is updated only after this
/// boundary confirms that the corresponding provider operation ran.
#[async_trait]
pub trait AdamEngineOperations: Send + Sync {
    async fn probe_endpoint(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
    ) -> anyhow::Result<AdamLlamaEngineProbe> {
        let slot_range = endpoint
            .slot_range()
            .ok_or_else(|| anyhow::anyhow!("invalid Adam-managed slot range"))?;
        Ok(AdamLlamaEngineProbe {
            runtime_instance_id: endpoint.runtime_instance_id.clone(),
            llama_build_fingerprint: endpoint.runtime_instance_id.clone(),
            tokenizer_hash: endpoint.tokenizer_hash.clone(),
            context_params_hash: endpoint.context_params_hash.clone(),
            max_context_tokens: 0,
            total_slots: u32::try_from(endpoint.slot_count).unwrap_or_default(),
            idle_slot_ids: slot_range.collect(),
            busy_slot_ids: Vec::new(),
            supports_snapshot_save: false,
            supports_snapshot_restore: false,
            supports_slot_erase: false,
        })
    }

    async fn load_snapshot(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _slot_id: i64,
        _snapshot: &AdamKvSnapshotRecord,
    ) -> anyhow::Result<AdamSnapshotRestoreResult> {
        Ok(AdamSnapshotRestoreResult {
            n_restored: 0,
            n_read: 0,
        })
    }

    /// Render provider-neutral no-tool prompt text into the exact llama.cpp
    /// wire bytes used by ordinary and prepared-compaction KV warmups.
    async fn project_wire_prompt(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _request: &LlamaWireProjectionRequest,
    ) -> anyhow::Result<LlamaWireProjection> {
        anyhow::bail!("Adam llama wire projection operation is not configured")
    }

    /// Counts the exact final wire prompt with the managed llama tokenizer.
    async fn count_wire_prompt_tokens(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _prompt: &str,
    ) -> anyhow::Result<i64> {
        anyhow::bail!("Adam live exact tokenizer operation is not configured")
    }

    async fn warm_prepared_compact(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
        intent: &AdamPreparedCompactIntent,
    ) -> anyhow::Result<()>;

    async fn promote_prepared_compact(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        prepared: &AdamPreparedCompactSlot,
    ) -> anyhow::Result<()>;

    async fn save_snapshot(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _slot_id: i64,
        _snapshot: &AdamKvSnapshotRecord,
    ) -> anyhow::Result<AdamSnapshotSaveResult> {
        anyhow::bail!("Adam snapshot save operation is not configured")
    }

    async fn erase_slot(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _slot_id: i64,
    ) -> anyhow::Result<i64> {
        anyhow::bail!("Adam slot erase operation is not configured")
    }
}

#[derive(Debug, Default)]
struct MetadataOnlyAdamEngineOperations;

#[async_trait]
impl AdamEngineOperations for MetadataOnlyAdamEngineOperations {
    async fn warm_prepared_compact(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _slot_id: i64,
        _intent: &AdamPreparedCompactIntent,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Adam prepared-prefix warm operation is not configured")
    }

    async fn promote_prepared_compact(
        &self,
        _endpoint: &AdamManagedLlamaEndpoint,
        _prepared: &AdamPreparedCompactSlot,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Adam prepared-prefix promotion operation is not configured")
    }
}

#[derive(Clone)]
pub struct AdamInferenceEngineManager {
    store: Arc<dyn AdamEngineStore>,
    operations: Arc<dyn AdamEngineOperations>,
    cache_scheduler: Arc<AdamCacheScheduler>,
}

impl AdamInferenceEngineManager {
    pub fn new(store: Arc<dyn AdamEngineStore>) -> Self {
        Self {
            store,
            operations: Arc::new(MetadataOnlyAdamEngineOperations),
            cache_scheduler: Arc::new(AdamCacheScheduler::default()),
        }
    }

    pub fn with_operations(
        store: Arc<dyn AdamEngineStore>,
        operations: Arc<dyn AdamEngineOperations>,
    ) -> Self {
        Self {
            store,
            operations,
            cache_scheduler: Arc::new(AdamCacheScheduler::default()),
        }
    }

    pub fn with_cache_scheduler(mut self, cache_scheduler: Arc<AdamCacheScheduler>) -> Self {
        self.cache_scheduler = cache_scheduler;
        self
    }

    pub async fn status_report(&self) -> crate::status::AdamEngineStatusReport {
        crate::status::build_adam_engine_status_with_scheduler(
            self.store.as_ref(),
            self.operations.as_ref(),
            self.cache_scheduler.as_ref(),
        )
        .await
    }

    /// Projects exact wire prompt bytes through the host-owned endpoint while
    /// keeping endpoint URLs and template dialect out of shared runtime code.
    pub async fn project_wire_prompt(
        &self,
        managed_endpoint_id: &str,
        request: &LlamaWireProjectionRequest,
    ) -> anyhow::Result<LlamaWireProjection> {
        let mut endpoint = self
            .store
            .endpoints()
            .into_iter()
            .find(|endpoint| {
                endpoint.endpoint_id == managed_endpoint_id
                    && endpoint.adam_managed_only
                    && endpoint.transport == "llamacpp_slots"
                    && endpoint.slot_range().is_some()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Adam wire projection requires managed llama endpoint {managed_endpoint_id}"
                )
            })?;
        let before = self
            .probe_complete_endpoint_identity(&endpoint, "wire projection")
            .await?;
        endpoint = self.persist_live_endpoint_identity(endpoint, &before);
        let projection = self
            .operations
            .project_wire_prompt(&endpoint, request)
            .await?;
        let after = self
            .probe_complete_endpoint_identity(&endpoint, "wire projection")
            .await?;
        if !same_physical_identity(&before, &after) {
            self.persist_live_endpoint_identity(endpoint, &after);
            anyhow::bail!("Adam llama runtime identity changed during wire projection");
        }
        Ok(projection)
    }

    pub async fn count_wire_prompt_tokens(
        &self,
        managed_endpoint_id: &str,
        prompt: &str,
    ) -> anyhow::Result<(AdamLlamaEngineProbe, i64)> {
        let endpoint = self
            .store
            .endpoints()
            .into_iter()
            .find(|endpoint| {
                endpoint.endpoint_id == managed_endpoint_id && endpoint.adam_managed_only
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Adam tokenizer requires managed llama endpoint {managed_endpoint_id}"
                )
            })?;
        let probe = self
            .probe_complete_endpoint_identity(&endpoint, "exact token count")
            .await?;
        if probe.max_context_tokens <= 0 {
            anyhow::bail!("Adam live /props omitted a verified max context capacity");
        }
        let count = self
            .operations
            .count_wire_prompt_tokens(&endpoint, prompt)
            .await?;
        if count < 0 {
            anyhow::bail!("Adam live /tokenize returned an invalid token count");
        }
        Ok((probe, count))
    }

    async fn probe_complete_endpoint_identity(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        operation: &str,
    ) -> anyhow::Result<AdamLlamaEngineProbe> {
        let probe = self.operations.probe_endpoint(endpoint).await?;
        if probe.runtime_instance_id.trim().is_empty()
            || probe.tokenizer_hash.trim().is_empty()
            || probe.context_params_hash.trim().is_empty()
        {
            anyhow::bail!(
                "Adam live engine identity probe was incomplete during {operation} for endpoint {}",
                endpoint.endpoint_id
            );
        }
        Ok(probe)
    }

    fn persist_live_endpoint_identity(
        &self,
        mut endpoint: AdamManagedLlamaEndpoint,
        probe: &AdamLlamaEngineProbe,
    ) -> AdamManagedLlamaEndpoint {
        endpoint.runtime_instance_id = probe.runtime_instance_id.clone();
        endpoint.tokenizer_hash = probe.tokenizer_hash.clone();
        endpoint.context_params_hash = probe.context_params_hash.clone();
        self.store.upsert_managed_endpoint(endpoint.clone());
        endpoint
    }

    async fn endpoint_identity_is_current(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        operation: &str,
    ) -> anyhow::Result<bool> {
        let probe = self
            .probe_complete_endpoint_identity(endpoint, operation)
            .await?;
        if probe_matches_endpoint_identity(&probe, endpoint) {
            return Ok(true);
        }
        self.persist_live_endpoint_identity(endpoint.clone(), &probe);
        Ok(false)
    }

    pub async fn allocate(
        &self,
        intent: AdamEngineControlIntent,
    ) -> anyhow::Result<AdamEngineAllocation> {
        self.allocate_internal(intent, None, None).await
    }

    /// Waits inside the Adam manager when the cache scheduler protects every
    /// usable slot. Queue retry, timeout, FIFO identity, and cancellation
    /// cleanup are Adam-owned; callers provide only the semantic priority and
    /// the shared turn cancellation signal.
    pub async fn allocate_with_wait(
        &self,
        intent: AdamEngineControlIntent,
        wait_config: AdamEngineAllocationWaitConfig,
        turn_control: Option<Arc<InhouseTurnControl>>,
    ) -> anyhow::Result<AdamEngineAllocation> {
        if wait_config.max_wait_ms <= 0 {
            anyhow::bail!("Adam allocation max_wait_ms must be greater than zero");
        }
        if wait_config.poll_interval_ms <= 0
            || wait_config.poll_interval_ms > wait_config.max_wait_ms
        {
            anyhow::bail!(
                "Adam allocation poll_interval_ms must be positive and no greater than max_wait_ms"
            );
        }
        self.allocate_internal(intent, Some(wait_config), turn_control)
            .await
    }

    async fn allocate_internal(
        &self,
        mut intent: AdamEngineControlIntent,
        wait_config: Option<AdamEngineAllocationWaitConfig>,
        turn_control: Option<Arc<InhouseTurnControl>>,
    ) -> anyhow::Result<AdamEngineAllocation> {
        let mut endpoint = self.compatible_endpoint(&intent)?;
        let previous_runtime_instance_id = endpoint.runtime_instance_id.clone();
        let probe = self.operations.probe_endpoint(&endpoint).await?;
        if probe.tokenizer_hash.trim().is_empty()
            || probe.context_params_hash.trim().is_empty()
            || probe.runtime_instance_id.trim().is_empty()
        {
            anyhow::bail!(
                "Adam live engine identity probe was incomplete for endpoint {}",
                endpoint.endpoint_id
            );
        }
        let compatibility_changed = !endpoint.tokenizer_hash.trim().is_empty()
            && (endpoint.tokenizer_hash != probe.tokenizer_hash
                || endpoint.context_params_hash != probe.context_params_hash);
        let runtime_generation_changed = !previous_runtime_instance_id.trim().is_empty()
            && previous_runtime_instance_id != probe.runtime_instance_id;
        if compatibility_changed {
            self.store.invalidate_live_residencies_for_runtime(
                &endpoint.endpoint_id,
                "adam-invalidate-physical-compatibility-change",
            );
        }
        endpoint.runtime_instance_id = probe.runtime_instance_id.clone();
        endpoint.tokenizer_hash = probe.tokenizer_hash.clone();
        endpoint.context_params_hash = probe.context_params_hash.clone();
        self.store.upsert_managed_endpoint(endpoint.clone());
        intent.tokenizer_hash = probe.tokenizer_hash.clone();
        intent.context_params_hash = probe.context_params_hash.clone();
        self.store.invalidate_live_residencies_for_runtime(
            &endpoint.endpoint_id,
            &endpoint.runtime_instance_id,
        );
        self.store.invalidate_live_residencies_for_context_epoch(
            &intent.inhouse_polar_id,
            &intent.thread_id,
            intent.context_compaction_epoch,
        );
        let mut live_residency = self.store.live_residency(
            &intent.inhouse_polar_id,
            &intent.thread_id,
            &intent.projection_id,
            &intent.exact_prefix_hash,
        );
        if live_residency.is_none() {
            live_residency = self
                .store
                .promoted_prepared_compact_slot(&AdamPromotedPreparedLookup {
                    endpoint_id: &endpoint.endpoint_id,
                    runtime_instance_id: &endpoint.runtime_instance_id,
                    model_id: &intent.model_id,
                    tokenizer_hash: &intent.tokenizer_hash,
                    context_params_hash: &intent.context_params_hash,
                    exact_prefix_hash: &intent.exact_prefix_hash,
                    context_compaction_epoch: intent.context_compaction_epoch,
                })
                .map(|prepared| AdamLiveSlotResidencyRecord {
                    inhouse_polar_id: intent.inhouse_polar_id.clone(),
                    thread_id: intent.thread_id.clone(),
                    projection_id: intent.projection_id.clone(),
                    endpoint_id: prepared.endpoint_id,
                    slot_id: prepared.slot_id,
                    exact_prefix_hash: prepared.exact_prefix_hash,
                    runtime_instance_id: prepared.runtime_instance_id,
                    prefix_token_count: 0,
                    context_compaction_epoch: prepared.target_context_compaction_epoch,
                    last_activity_unix_ms: crate::now_unix_ms(),
                });
        }
        let live_residency = live_residency.filter(|residency| {
            residency.endpoint_id == endpoint.endpoint_id
                && residency.runtime_instance_id == endpoint.runtime_instance_id
                && residency.context_compaction_epoch == intent.context_compaction_epoch
        });
        if let Some(residency) = live_residency.as_ref() {
            self.cache_scheduler.observe_residency(AdamCacheResident {
                endpoint_id: residency.endpoint_id.clone(),
                runtime_instance_id: residency.runtime_instance_id.clone(),
                affinity: CacheAffinityKey {
                    inhouse_polar_id: residency.inhouse_polar_id.clone(),
                    thread_id: residency.thread_id.clone(),
                    projection_id: residency.projection_id.clone(),
                    exact_prefix_hash: residency.exact_prefix_hash.clone(),
                    context_compaction_epoch: residency.context_compaction_epoch,
                },
                slot_id: residency.slot_id,
                last_activity_unix_ms: residency.last_activity_unix_ms.max(crate::now_unix_ms()),
                prefix_token_count: residency.prefix_token_count,
                in_flight: false,
            });
        }
        for residency in self
            .store
            .live_residencies(&endpoint.endpoint_id, &endpoint.runtime_instance_id)
        {
            self.cache_scheduler.observe_residency(AdamCacheResident {
                endpoint_id: endpoint.endpoint_id.clone(),
                runtime_instance_id: endpoint.runtime_instance_id.clone(),
                affinity: CacheAffinityKey {
                    inhouse_polar_id: residency.inhouse_polar_id,
                    thread_id: residency.thread_id,
                    projection_id: residency.projection_id,
                    exact_prefix_hash: residency.exact_prefix_hash,
                    context_compaction_epoch: residency.context_compaction_epoch,
                },
                slot_id: residency.slot_id,
                last_activity_unix_ms: residency.last_activity_unix_ms,
                prefix_token_count: residency.prefix_token_count,
                in_flight: false,
            });
        }
        let requested_at_unix_ms = crate::now_unix_ms();
        self.store
            .reconcile_slot_reservations(requested_at_unix_ms, false);
        let lease_id = format!(
            "adam-lease-{}",
            short_hash(&format!(
                "{}:{}:{}:{}:{}",
                endpoint.endpoint_id,
                endpoint.runtime_instance_id,
                intent.projection_id,
                intent.exact_prefix_hash,
                now_unix_nanos()
            ))
        );
        let endpoint_slot_ids = endpoint
            .slot_range()
            .ok_or_else(|| anyhow::anyhow!("invalid Adam-managed endpoint slot range"))?
            .collect::<Vec<_>>();
        let schedule_request = AdamCacheScheduleRequest {
            request_id: format!(
                "adam-schedule-{}",
                short_hash(&format!(
                    "{}:{}:{}:{}:{}",
                    endpoint.endpoint_id,
                    intent.inhouse_polar_id,
                    intent.thread_id,
                    intent.projection_id,
                    intent.exact_prefix_hash
                ))
            ),
            endpoint_id: endpoint.endpoint_id.clone(),
            runtime_instance_id: endpoint.runtime_instance_id.clone(),
            affinity: CacheAffinityKey {
                inhouse_polar_id: intent.inhouse_polar_id.clone(),
                thread_id: intent.thread_id.clone(),
                projection_id: intent.projection_id.clone(),
                exact_prefix_hash: intent.exact_prefix_hash.clone(),
                context_compaction_epoch: intent.context_compaction_epoch,
            },
            priority: intent.priority.clone(),
            requested_at_unix_ms,
        };
        let wait_deadline = wait_config.map(|config| {
            tokio::time::Instant::now()
                + Duration::from_millis(u64::try_from(config.max_wait_ms).unwrap_or(u64::MAX))
        });
        let mut initial_wait_queue_position = None;
        let mut initial_wait_queue_depth = None;
        let mut wait_observation_count = 0_u32;
        let mut selected = None;
        let mut immediate_reservation_attempts = 0_usize;
        loop {
            if turn_control
                .as_ref()
                .is_some_and(|control| control.stop_requested())
            {
                self.cache_scheduler.cancel_waiting(
                    &endpoint.endpoint_id,
                    &endpoint.runtime_instance_id,
                    &schedule_request.request_id,
                );
                anyhow::bail!(
                    "Adam allocation interrupted while waiting for scheduler request {}",
                    schedule_request.request_id
                );
            }
            if let (Some(config), Some(deadline)) = (wait_config, wait_deadline) {
                if tokio::time::Instant::now() >= deadline {
                    self.cache_scheduler.cancel_waiting(
                        &endpoint.endpoint_id,
                        &endpoint.runtime_instance_id,
                        &schedule_request.request_id,
                    );
                    anyhow::bail!(
                        "Adam cache scheduler timed out request {} after {} ms",
                        schedule_request.request_id,
                        config.max_wait_ms
                    );
                }
            }
            let evaluated_at_unix_ms = crate::now_unix_ms();
            self.store
                .reconcile_slot_reservations(evaluated_at_unix_ms, false);
            let reserved_slot_ids = self
                .store
                .slot_reservations()
                .into_iter()
                .filter(|reservation| {
                    reservation.endpoint_id == endpoint.endpoint_id
                        && reservation.runtime_instance_id == endpoint.runtime_instance_id
                })
                .map(|reservation| reservation.slot_id)
                .collect::<Vec<_>>();
            let usable_slot_ids = endpoint_slot_ids
                .iter()
                .copied()
                .filter(|slot_id| !reserved_slot_ids.contains(slot_id))
                .collect::<Vec<_>>();
            let schedule = self.cache_scheduler.schedule_at(
                &schedule_request,
                &usable_slot_ids,
                evaluated_at_unix_ms,
            );
            let Some(scheduled_slot_id) = schedule.slot_id else {
                wait_observation_count = wait_observation_count.saturating_add(1);
                if wait_observation_count == 1 {
                    initial_wait_queue_position = schedule.wait_queue_position;
                    initial_wait_queue_depth = schedule.wait_queue_depth;
                }
                let Some(config) = wait_config else {
                    self.cache_scheduler.cancel_waiting(
                        &endpoint.endpoint_id,
                        &endpoint.runtime_instance_id,
                        &schedule_request.request_id,
                    );
                    anyhow::bail!(
                        "Adam cache scheduler queued request {} at position {}",
                        schedule_request.request_id,
                        schedule.wait_queue_position.unwrap_or_default()
                    );
                };
                if wait_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    self.cache_scheduler.cancel_waiting(
                        &endpoint.endpoint_id,
                        &endpoint.runtime_instance_id,
                        &schedule_request.request_id,
                    );
                    anyhow::bail!(
                        "Adam cache scheduler timed out request {} after {} ms",
                        schedule_request.request_id,
                        config.max_wait_ms
                    );
                }
                let sleep = tokio::time::sleep(Duration::from_millis(
                    u64::try_from(config.poll_interval_ms).unwrap_or(1),
                ));
                tokio::pin!(sleep);
                if let Some(control) = turn_control.as_ref() {
                    tokio::select! {
                        _ = &mut sleep => {}
                        _ = control.wait_for_stop() => {
                            self.cache_scheduler.cancel_waiting(
                                &endpoint.endpoint_id,
                                &endpoint.runtime_instance_id,
                                &schedule_request.request_id,
                            );
                            anyhow::bail!(
                                "Adam allocation interrupted while waiting for scheduler request {}",
                                schedule_request.request_id
                            );
                        }
                    }
                } else {
                    sleep.await;
                }
                continue;
            };
            let admitted_at_unix_ms = crate::now_unix_ms();
            if let Some(reservation) =
                self.store
                    .reserve_turn_slot(&AdamTurnSlotReservationRequest {
                        lease_id: &lease_id,
                        endpoint_id: &endpoint.endpoint_id,
                        runtime_instance_id: &endpoint.runtime_instance_id,
                        slot_offset: endpoint.slot_offset,
                        slot_count: endpoint.slot_count,
                        preferred_slot_id: live_residency
                            .as_ref()
                            .map(|residency| residency.slot_id),
                        allowed_slot_ids: &[scheduled_slot_id],
                        reserved_at_unix_ms: admitted_at_unix_ms,
                        ttl_ms: DEFAULT_SLOT_LEASE_TTL_MS,
                    })
            {
                selected = Some((schedule, reservation, admitted_at_unix_ms));
                break;
            }
            immediate_reservation_attempts = immediate_reservation_attempts.saturating_add(1);
            if wait_config.is_none()
                && immediate_reservation_attempts >= endpoint_slot_ids.len().max(1)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let (schedule, reservation, admitted_at_unix_ms) = selected.ok_or_else(|| {
            self.cache_scheduler.cancel_waiting(
                &endpoint.endpoint_id,
                &endpoint.runtime_instance_id,
                &schedule_request.request_id,
            );
            anyhow::anyhow!(
                "no allocatable slot on Adam-managed endpoint {}",
                endpoint.endpoint_id
            )
        })?;
        let slot_id = reservation.slot_id;
        let Some(persistent_lease) =
            self.store
                .compare_and_swap_slot_lease(&AdamSlotLeaseCasRequest {
                    lease_id: &lease_id,
                    endpoint_id: &endpoint.endpoint_id,
                    runtime_instance_id: &endpoint.runtime_instance_id,
                    slot_id,
                    inhouse_polar_id: &intent.inhouse_polar_id,
                    thread_id: &intent.thread_id,
                    projection_id: &intent.projection_id,
                    acquired_at_unix_ms: admitted_at_unix_ms,
                    ttl_ms: DEFAULT_SLOT_LEASE_TTL_MS,
                })
        else {
            self.store.release_slot_reservation(&lease_id);
            anyhow::bail!(
                "persistent Adam slot lease compare_and_swap failed for endpoint {} slot {}",
                endpoint.endpoint_id,
                slot_id
            );
        };
        if schedule.snapshot_before_eviction {
            if let Some(evicted) = schedule.evicted.as_ref() {
                if let Err(error) = self
                    .save_evicted_residency_snapshot(&endpoint, evicted, &probe)
                    .await
                {
                    self.store
                        .upsert_live_residency(AdamLiveSlotResidencyRecord {
                            inhouse_polar_id: evicted.affinity.inhouse_polar_id.clone(),
                            thread_id: evicted.affinity.thread_id.clone(),
                            projection_id: evicted.affinity.projection_id.clone(),
                            endpoint_id: endpoint.endpoint_id.clone(),
                            slot_id: evicted.slot_id,
                            exact_prefix_hash: evicted.affinity.exact_prefix_hash.clone(),
                            runtime_instance_id: endpoint.runtime_instance_id.clone(),
                            prefix_token_count: evicted.prefix_token_count,
                            context_compaction_epoch: evicted.affinity.context_compaction_epoch,
                            last_activity_unix_ms: evicted.last_activity_unix_ms,
                        });
                    self.store.release_slot_lease(
                        &persistent_lease.lease_id,
                        &persistent_lease.fencing_token,
                        crate::now_unix_ms(),
                    );
                    self.store.release_slot_reservation(&lease_id);
                    return Err(error);
                }
                self.cache_scheduler.remove_residency(
                    &endpoint.endpoint_id,
                    &endpoint.runtime_instance_id,
                    evicted.slot_id,
                );
            }
        }
        let cache_plan = if reservation.used_preferred_slot {
            AdamCachePlan::UseLiveSlot {
                residency: live_residency.expect("preferred slot requires live residency"),
            }
        } else {
            let compatibility = AdamSnapshotCompatibility {
                model_id: &intent.model_id,
                tokenizer_hash: &intent.tokenizer_hash,
                context_params_hash: &intent.context_params_hash,
                exact_prefix_hash: &intent.exact_prefix_hash,
                context_compaction_epoch: intent.context_compaction_epoch,
                llama_build_fingerprint: &probe.llama_build_fingerprint,
            };
            let compatible_snapshot = self.store.compatible_snapshot(&compatibility);
            let loaded_snapshot = if let Some(snapshot) = compatible_snapshot {
                match self
                    .operations
                    .load_snapshot(&endpoint, slot_id, &snapshot)
                    .await
                {
                    Ok(restored)
                        if restored.n_restored > 0
                            && restored.n_restored == snapshot.prefix_token_count
                            && restored.n_read > 0
                            && restored.n_read == snapshot.snapshot_bytes_written =>
                    {
                        let post_restore = match self.operations.probe_endpoint(&endpoint).await {
                            Ok(probe) => probe,
                            Err(error) => {
                                // The restore may have mutated the leased old
                                // generation, but without a successful probe we
                                // cannot prove which process now owns the slot.
                                // Quarantine through the lease TTL instead of
                                // issuing an unsafe erase against an unknown
                                // generation.
                                self.store.remove_snapshot(&snapshot.snapshot_id);
                                anyhow::bail!(
                                    "cannot verify llama runtime after snapshot restore: {error}"
                                );
                            }
                        };
                        if post_restore.runtime_instance_id != endpoint.runtime_instance_id {
                            self.store.remove_snapshot(&snapshot.snapshot_id);
                            let mut restarted_endpoint = endpoint.clone();
                            restarted_endpoint.runtime_instance_id =
                                post_restore.runtime_instance_id;
                            restarted_endpoint.tokenizer_hash = post_restore.tokenizer_hash;
                            restarted_endpoint.context_params_hash =
                                post_restore.context_params_hash;
                            self.store.upsert_managed_endpoint(restarted_endpoint);
                            anyhow::bail!(
                                "llama runtime changed during snapshot restore for endpoint {}",
                                endpoint.endpoint_id
                            );
                        }
                        Some(snapshot)
                    }
                    Ok(_) | Err(_) => {
                        // A successful HTTP response is not sufficient proof that
                        // the durable snapshot bytes match their metadata. A
                        // partial/mismatched restore may have mutated the slot, so
                        // erase it before falling back to an honest prefix rewarm.
                        self.store.remove_snapshot(&snapshot.snapshot_id);
                        self.erase_physical_slot_for_runtime(
                            &endpoint,
                            slot_id,
                            &endpoint.runtime_instance_id,
                        )
                        .await?;
                        None
                    }
                }
            } else {
                None
            };
            match loaded_snapshot {
                Some(snapshot) => AdamCachePlan::LoadSnapshot { snapshot },
                None if !intent.exact_prefix_hash.is_empty() => AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: intent.exact_prefix_hash.clone(),
                },
                None => AdamCachePlan::ColdPrefill {
                    reason: "no_live_residency_loaded_snapshot_or_prefix".to_string(),
                },
            }
        };
        let lease = AdamSlotLease {
            lease_id,
            fencing_token: persistent_lease.fencing_token,
            endpoint_id: endpoint.endpoint_id.clone(),
            slot_id,
            runtime_instance_id: endpoint.runtime_instance_id.clone(),
            lease_generation: persistent_lease.lease_generation,
            expires_at_unix_ms: persistent_lease.expires_at_unix_ms,
        };
        let evidence = AdamEngineControlEvidence {
            endpoint_id: endpoint.endpoint_id.clone(),
            slot_id,
            runtime_instance_id: endpoint.runtime_instance_id.clone(),
            cache_plan: cache_plan.as_str().to_string(),
            exact_prefix_hash: intent.exact_prefix_hash.clone(),
            allocation_reason: schedule.allocation_reason.as_str().to_string(),
            eviction_decision: if schedule.evicted.is_some() {
                "expired_residency_fairness_eviction"
            } else {
                "none"
            }
            .to_string(),
            protected_activity_ttl_ms: self.cache_scheduler.protected_activity_ttl_ms(),
            scheduling_priority: match &intent.priority {
                AdamEngineTurnPriority::ActiveThread => "active_thread",
                AdamEngineTurnPriority::Background => "background",
            }
            .to_string(),
            scheduler_request_id: schedule_request.request_id,
            initial_wait_queue_position,
            initial_wait_queue_depth,
            wait_observation_count,
            waited_ms: admitted_at_unix_ms.saturating_sub(requested_at_unix_ms),
            wait_limit_ms: wait_config.map_or(0, |config| config.max_wait_ms),
            admitted_after_wait: wait_observation_count > 0,
            evicted_active_residency: schedule.evicted_active_residency,
            snapshot_before_eviction: schedule.snapshot_before_eviction,
            previous_runtime_instance_id: runtime_generation_changed
                .then_some(previous_runtime_instance_id),
            runtime_generation_changed,
            old_runtime_invalidated: runtime_generation_changed,
            cache_epoch_transition: if runtime_generation_changed {
                match &cache_plan {
                    AdamCachePlan::LoadSnapshot { .. } => {
                        "runtime_generation_invalidated_then_snapshot_loaded"
                    }
                    AdamCachePlan::RewarmPrefix { .. } => {
                        "runtime_generation_invalidated_then_prefix_rewarmed"
                    }
                    AdamCachePlan::ColdPrefill { .. } => {
                        "runtime_generation_invalidated_then_cold_prefill"
                    }
                    AdamCachePlan::UseLiveSlot { .. } => {
                        "runtime_generation_invalidated_then_live_slot_rejected"
                    }
                }
            } else {
                "unchanged"
            }
            .to_string(),
        };
        Ok(AdamEngineAllocation {
            intent,
            endpoint,
            lease,
            cache_plan,
            evidence,
        })
    }

    pub fn release_allocation(&self, allocation: &AdamEngineAllocation) -> bool {
        let released_lease = self.store.release_slot_lease(
            &allocation.lease.lease_id,
            &allocation.lease.fencing_token,
            crate::now_unix_ms(),
        );
        let released_reservation = self
            .store
            .release_slot_reservation(&allocation.lease.lease_id);
        released_lease || released_reservation
    }

    pub fn heartbeat_allocation(&self, allocation: &AdamEngineAllocation) -> bool {
        self.store.heartbeat_slot_lease(
            &allocation.lease.lease_id,
            &allocation.lease.fencing_token,
            crate::now_unix_ms(),
            DEFAULT_SLOT_LEASE_TTL_MS,
        )
    }

    /// Waits for the provider to stop processing the leased physical slot
    /// after client-side cancellation. The caller keeps the fencing lease and
    /// reservation while this check runs. A timeout returns `false`, signaling
    /// that the guard must quarantine the slot until its durable lease expires
    /// rather than immediately recycling it.
    pub async fn wait_until_allocation_idle(
        &self,
        allocation: &AdamEngineAllocation,
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.operations.probe_endpoint(&allocation.endpoint).await {
                Ok(probe) if probe.runtime_instance_id != allocation.lease.runtime_instance_id => {
                    return true;
                }
                Ok(probe) if !probe.busy_slot_ids.contains(&allocation.lease.slot_id) => {
                    return true;
                }
                Ok(_) | Err(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Saves physical KV state and records snapshot metadata only after llama
    /// confirms that non-empty bytes were written.
    pub async fn save_snapshot(
        &self,
        allocation: &AdamEngineAllocation,
    ) -> anyhow::Result<AdamKvSnapshotRecord> {
        let now_unix_ms = crate::now_unix_ms();
        if !self.store.validate_slot_lease(
            &allocation.lease.lease_id,
            &allocation.lease.fencing_token,
            &allocation.lease.runtime_instance_id,
            now_unix_ms,
        ) {
            anyhow::bail!("cannot save snapshot for stale Adam slot lease");
        }
        let probe = self.operations.probe_endpoint(&allocation.endpoint).await?;
        if probe.runtime_instance_id != allocation.lease.runtime_instance_id {
            anyhow::bail!("cannot save snapshot after llama runtime identity changed");
        }
        let snapshot_id = format!(
            "adam-snapshot-{}",
            short_hash(&format!(
                "{}:{}:{}:{}:{}",
                allocation.intent.model_id,
                allocation.intent.exact_prefix_hash,
                allocation.intent.context_compaction_epoch,
                allocation.lease.slot_id,
                now_unix_ms
            ))
        );
        let mut snapshot = AdamKvSnapshotRecord {
            snapshot_id: snapshot_id.clone(),
            blob_ref: format!("{snapshot_id}.bin"),
            model_id: allocation.intent.model_id.clone(),
            tokenizer_hash: allocation.intent.tokenizer_hash.clone(),
            context_params_hash: allocation.intent.context_params_hash.clone(),
            exact_prefix_hash: allocation.intent.exact_prefix_hash.clone(),
            context_compaction_epoch: allocation.intent.context_compaction_epoch,
            prefix_token_count: 0,
            snapshot_checksum: String::new(),
            snapshot_bytes_written: 0,
            snapshot_format_version: "llama-slot-v1".to_string(),
            llama_build_fingerprint: probe.llama_build_fingerprint,
        };
        let saved = self
            .operations
            .save_snapshot(&allocation.endpoint, allocation.lease.slot_id, &snapshot)
            .await?;
        if saved.n_saved <= 0 || saved.n_written <= 0 {
            anyhow::bail!("Adam snapshot save returned an empty physical receipt");
        }
        let post_save_probe = self.operations.probe_endpoint(&allocation.endpoint).await?;
        if post_save_probe.runtime_instance_id != allocation.lease.runtime_instance_id {
            anyhow::bail!("cannot record snapshot after llama restarted during physical save");
        }
        snapshot.prefix_token_count = saved.n_saved;
        snapshot.snapshot_bytes_written = saved.n_written;
        self.store.upsert_snapshot(snapshot.clone());
        Ok(snapshot)
    }

    /// Erases a physical slot only for the still-current fencing generation,
    /// then removes matching live metadata so the next turn cannot claim a hit.
    pub async fn erase_slot(&self, allocation: &AdamEngineAllocation) -> anyhow::Result<i64> {
        if !self.store.validate_slot_lease(
            &allocation.lease.lease_id,
            &allocation.lease.fencing_token,
            &allocation.lease.runtime_instance_id,
            crate::now_unix_ms(),
        ) {
            anyhow::bail!("cannot erase slot for stale Adam slot lease");
        }
        let erased = self
            .erase_physical_slot_for_runtime(
                &allocation.endpoint,
                allocation.lease.slot_id,
                &allocation.lease.runtime_instance_id,
            )
            .await?;
        self.store.remove_live_residency(
            &allocation.lease.endpoint_id,
            &allocation.lease.runtime_instance_id,
            allocation.lease.slot_id,
        );
        self.cache_scheduler.remove_residency(
            &allocation.lease.endpoint_id,
            &allocation.lease.runtime_instance_id,
            allocation.lease.slot_id,
        );
        Ok(erased)
    }

    /// Removes cache metadata for an incomplete provider/tool sequence only
    /// while its fencing lease is still current. Callers that cannot confirm a
    /// physical erase must keep the lease quarantined until expiry.
    pub fn invalidate_allocation_residency_if_current(
        &self,
        allocation: &AdamEngineAllocation,
    ) -> bool {
        if !self.store.validate_slot_lease(
            &allocation.lease.lease_id,
            &allocation.lease.fencing_token,
            &allocation.lease.runtime_instance_id,
            crate::now_unix_ms(),
        ) {
            return false;
        }
        self.store.remove_live_residency(
            &allocation.lease.endpoint_id,
            &allocation.lease.runtime_instance_id,
            allocation.lease.slot_id,
        );
        self.cache_scheduler.remove_residency(
            &allocation.lease.endpoint_id,
            &allocation.lease.runtime_instance_id,
            allocation.lease.slot_id,
        );
        true
    }

    /// Brackets a destructive mutation with live process-identity probes.
    /// llama.cpp does not currently expose a conditional-generation argument
    /// on slot actions, so any unverified or changed generation fails closed
    /// instead of claiming a successful erase; callers retain or quarantine
    /// their metadata according to the operation that requested the erase.
    async fn erase_physical_slot_for_runtime(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
        expected_runtime_instance_id: &str,
    ) -> anyhow::Result<i64> {
        let before = self.operations.probe_endpoint(endpoint).await?;
        if before.runtime_instance_id != expected_runtime_instance_id {
            anyhow::bail!("cannot erase slot after llama runtime identity changed");
        }
        let erased = self.operations.erase_slot(endpoint, slot_id).await?;
        let after = self.operations.probe_endpoint(endpoint).await?;
        if after.runtime_instance_id != expected_runtime_instance_id {
            anyhow::bail!("cannot confirm slot erase after llama runtime identity changed");
        }
        Ok(erased)
    }

    pub async fn snapshot_before_eviction(
        &self,
        allocation: &AdamEngineAllocation,
    ) -> anyhow::Result<AdamKvSnapshotRecord> {
        let snapshot = self.save_snapshot(allocation).await?;
        self.erase_slot(allocation).await?;
        Ok(snapshot)
    }

    async fn save_evicted_residency_snapshot(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        evicted: &AdamCacheResident,
        probe: &AdamLlamaEngineProbe,
    ) -> anyhow::Result<AdamKvSnapshotRecord> {
        let snapshot_id = format!(
            "adam-snapshot-{}",
            short_hash(&format!(
                "{}:{}:{}:{}",
                endpoint.model_id,
                evicted.affinity.exact_prefix_hash,
                evicted.affinity.context_compaction_epoch,
                now_unix_nanos()
            ))
        );
        let mut snapshot = AdamKvSnapshotRecord {
            snapshot_id: snapshot_id.clone(),
            blob_ref: format!("{snapshot_id}.bin"),
            model_id: endpoint.model_id.clone(),
            tokenizer_hash: endpoint.tokenizer_hash.clone(),
            context_params_hash: endpoint.context_params_hash.clone(),
            exact_prefix_hash: evicted.affinity.exact_prefix_hash.clone(),
            context_compaction_epoch: evicted.affinity.context_compaction_epoch,
            prefix_token_count: evicted.prefix_token_count,
            snapshot_checksum: String::new(),
            snapshot_bytes_written: 0,
            snapshot_format_version: "llama-slot-v1".to_string(),
            llama_build_fingerprint: probe.llama_build_fingerprint.clone(),
        };
        let saved = self
            .operations
            .save_snapshot(endpoint, evicted.slot_id, &snapshot)
            .await?;
        if saved.n_saved <= 0 || saved.n_written <= 0 {
            anyhow::bail!("Adam eviction snapshot save returned an empty physical receipt");
        }
        let post_save_probe = self.operations.probe_endpoint(endpoint).await?;
        if post_save_probe.runtime_instance_id != endpoint.runtime_instance_id {
            anyhow::bail!("cannot record eviction snapshot after llama restarted during save");
        }
        snapshot.prefix_token_count = saved.n_saved;
        snapshot.snapshot_bytes_written = saved.n_written;
        self.store.upsert_snapshot(snapshot.clone());
        Ok(snapshot)
    }

    /// Records evidence only after the provider has completed successfully,
    /// then publishes the observed live residency for future allocations.
    pub fn record_engine_evidence(
        &self,
        allocation: &AdamEngineAllocation,
        cache_hit_tokens: Option<i64>,
        cache_miss_tokens: Option<i64>,
    ) -> anyhow::Result<AdamEngineCompletionEvidence> {
        let created_at_unix_ms = crate::now_unix_ms();
        let evidence = AdamEngineCompletionEvidence {
            evidence_id: format!(
                "adam-engine-evidence-{}",
                short_hash(&format!(
                    "{}:{}:{}:{}:{}:{}:{}",
                    allocation.lease.endpoint_id,
                    allocation.lease.runtime_instance_id,
                    allocation.lease.slot_id,
                    allocation.intent.exact_prefix_hash,
                    allocation.lease.lease_id,
                    allocation.lease.lease_generation,
                    created_at_unix_ms
                ))
            ),
            endpoint_id: allocation.lease.endpoint_id.clone(),
            slot_id: allocation.lease.slot_id,
            runtime_instance_id: allocation.lease.runtime_instance_id.clone(),
            cache_plan: allocation.cache_plan.as_str().to_string(),
            allocation_reason: allocation.evidence.allocation_reason.clone(),
            exact_prefix_hash: allocation.intent.exact_prefix_hash.clone(),
            cache_hit_tokens,
            cache_miss_tokens,
            confirmed_provider_completion: true,
            created_at_unix_ms,
        };
        let residency = AdamLiveSlotResidencyRecord {
            inhouse_polar_id: allocation.intent.inhouse_polar_id.clone(),
            thread_id: allocation.intent.thread_id.clone(),
            projection_id: allocation.intent.projection_id.clone(),
            endpoint_id: allocation.lease.endpoint_id.clone(),
            slot_id: allocation.lease.slot_id,
            exact_prefix_hash: allocation.intent.exact_prefix_hash.clone(),
            runtime_instance_id: allocation.lease.runtime_instance_id.clone(),
            prefix_token_count: cache_hit_tokens
                .unwrap_or_default()
                .saturating_add(cache_miss_tokens.unwrap_or_default()),
            context_compaction_epoch: allocation.intent.context_compaction_epoch,
            last_activity_unix_ms: created_at_unix_ms,
        };
        if !self
            .store
            .commit_engine_completion_if_current(AdamEngineCompletionCommit {
                lease_id: allocation.lease.lease_id.clone(),
                fencing_token: allocation.lease.fencing_token.clone(),
                runtime_instance_id: allocation.lease.runtime_instance_id.clone(),
                now_unix_ms: created_at_unix_ms,
                evidence: evidence.clone(),
                residency,
            })
        {
            anyhow::bail!(
                "stale Adam provider result rejected for lease {} generation {}",
                allocation.lease.lease_id,
                allocation.lease.lease_generation
            );
        }
        self.cache_scheduler.observe_residency(AdamCacheResident {
            endpoint_id: allocation.lease.endpoint_id.clone(),
            runtime_instance_id: allocation.lease.runtime_instance_id.clone(),
            affinity: CacheAffinityKey {
                inhouse_polar_id: allocation.intent.inhouse_polar_id.clone(),
                thread_id: allocation.intent.thread_id.clone(),
                projection_id: allocation.intent.projection_id.clone(),
                exact_prefix_hash: allocation.intent.exact_prefix_hash.clone(),
                context_compaction_epoch: allocation.intent.context_compaction_epoch,
            },
            slot_id: allocation.lease.slot_id,
            last_activity_unix_ms: created_at_unix_ms,
            prefix_token_count: cache_hit_tokens
                .unwrap_or_default()
                .saturating_add(cache_miss_tokens.unwrap_or_default()),
            in_flight: false,
        });
        Ok(evidence)
    }

    pub fn record_completion_evidence(
        &self,
        allocation: &AdamEngineAllocation,
        cache_hit_tokens: Option<i64>,
        cache_miss_tokens: Option<i64>,
    ) -> anyhow::Result<AdamEngineCompletionEvidence> {
        let result = self.record_engine_evidence(allocation, cache_hit_tokens, cache_miss_tokens);
        self.release_allocation(allocation);
        result
    }

    /// Opportunistically prepares only a spare idle slot. An active residency
    /// is never removed to make room for background compaction.
    pub async fn prepare_compact(
        &self,
        intent: AdamPreparedCompactIntent,
    ) -> anyhow::Result<AdamPreparedCompactResult> {
        verify_exact_prefix_payload(&intent)?;
        let endpoint = self.compatible_prepared_endpoint(&intent)?;
        let before = self
            .probe_complete_endpoint_identity(&endpoint, "prepared compact warmup")
            .await?;
        if !probe_matches_endpoint_identity(&before, &endpoint) {
            self.persist_live_endpoint_identity(endpoint, &before);
            anyhow::bail!("Adam llama runtime identity changed before prepared compact warmup");
        }
        if let Some(existing) = self
            .store
            .prepared_compact_slot_for_session(&intent.session_id)
        {
            self.discard_prepared_compact(
                &existing.candidate_compaction_id,
                "superseded_prepared_compact_candidate",
            );
        }
        self.store.invalidate_live_residencies_for_runtime(
            &endpoint.endpoint_id,
            &endpoint.runtime_instance_id,
        );
        let active_slot_ids = self
            .store
            .active_slot_ids(&endpoint.endpoint_id, &endpoint.runtime_instance_id);
        let reservation_id = format!(
            "adam-prepare-{}",
            short_hash(&format!(
                "{}:{}:{}:{}",
                endpoint.endpoint_id,
                endpoint.runtime_instance_id,
                intent.candidate_compaction_id,
                now_unix_nanos()
            ))
        );
        let Some(slot_id) = self.store.reserve_prepared_slot(
            &reservation_id,
            &endpoint.endpoint_id,
            &endpoint.runtime_instance_id,
            endpoint.slot_offset,
            endpoint.slot_count,
        ) else {
            return Ok(AdamPreparedCompactResult::NoSpareIdleSlot { active_slot_ids });
        };
        if let Err(error) = self
            .operations
            .warm_prepared_compact(&endpoint, slot_id, &intent)
            .await
        {
            self.store.release_slot_reservation(&reservation_id);
            return Err(error);
        }
        let after = match self
            .probe_complete_endpoint_identity(&endpoint, "prepared compact warmup")
            .await
        {
            Ok(probe) => probe,
            Err(error) => {
                self.store.release_slot_reservation(&reservation_id);
                return Err(error);
            }
        };
        if !same_physical_identity(&before, &after)
            || !probe_matches_endpoint_identity(&after, &endpoint)
        {
            self.store.release_slot_reservation(&reservation_id);
            self.persist_live_endpoint_identity(endpoint, &after);
            anyhow::bail!("Adam llama runtime identity changed during prepared compact warmup");
        }
        let prepared = AdamPreparedCompactSlot {
            session_id: intent.session_id,
            thread_id: intent.thread_id,
            candidate_compaction_id: intent.candidate_compaction_id,
            source_digest_before: intent.source_digest_before,
            checkpoint_source_ref: intent.checkpoint_source_ref,
            exact_prefix_hash: intent.exact_prefix_hash,
            target_context_compaction_epoch: intent.target_context_compaction_epoch,
            model_id: intent.model_id,
            tokenizer_hash: intent.tokenizer_hash,
            context_params_hash: intent.context_params_hash,
            endpoint_id: endpoint.endpoint_id,
            runtime_instance_id: endpoint.runtime_instance_id,
            slot_id,
            promoted: false,
            committed_compaction_id: None,
            discard_reason: None,
        };
        self.store.upsert_prepared_compact_slot(prepared.clone());
        self.store.release_slot_reservation(&reservation_id);
        Ok(AdamPreparedCompactResult::Prepared(Box::new(prepared)))
    }

    pub async fn promote_prepared_compact(
        &self,
        approved: &AdamPreparedCompactIntent,
        commit: &polar_inhouse_contract::InhouseCompactionCommit,
    ) -> anyhow::Result<AdamPreparedCompactPromotion> {
        verify_exact_prefix_payload(approved)?;
        let Some(prepared) = self
            .store
            .prepared_compact_slot(&approved.candidate_compaction_id)
            .or_else(|| {
                self.store
                    .prepared_compact_slot_for_session(&approved.session_id)
            })
        else {
            return Ok(AdamPreparedCompactPromotion::Missing {
                candidate_compaction_id: approved.candidate_compaction_id.clone(),
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        };
        let commit_mismatch = [
            ("thread_id", commit.thread_id == approved.thread_id),
            (
                "candidate_compaction_id",
                commit.candidate_compaction_id == approved.candidate_compaction_id,
            ),
            (
                "source_digest",
                commit.source_digest == approved.source_digest_before,
            ),
            (
                "context_compaction_epoch",
                commit.context_compaction_epoch == approved.target_context_compaction_epoch,
            ),
            (
                "committed_compaction_id",
                !commit.committed_compaction_id.trim().is_empty(),
            ),
        ]
        .into_iter()
        .find_map(|(field, matches)| (!matches).then(|| format!("commit_mismatch:{field}")));
        if let Some(reason) = commit_mismatch {
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        }
        let Some(endpoint) = self.store.endpoints().into_iter().find(|endpoint| {
            endpoint.endpoint_id == prepared.endpoint_id
                && endpoint.adam_managed_only
                && !endpoint.context_params_hash.trim().is_empty()
        }) else {
            let reason = "prepared_mismatch:endpoint_id".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        };
        let mismatch = prepared_match_mismatch(&prepared, approved, &endpoint);
        if let Some(reason) = mismatch {
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        }
        if !self
            .endpoint_identity_is_current(&endpoint, "prepared compact promotion")
            .await?
        {
            let reason = "prepared_mismatch:runtime_identity_changed".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        }
        self.operations
            .promote_prepared_compact(&endpoint, &prepared)
            .await?;
        if !self
            .endpoint_identity_is_current(&endpoint, "prepared compact promotion")
            .await?
        {
            let reason = "prepared_mismatch:runtime_identity_changed".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: approved.exact_prefix_hash.clone(),
                },
            });
        }
        let promoted = self
            .store
            .promote_prepared_compact_slot(
                &approved.candidate_compaction_id,
                &commit.committed_compaction_id,
            )
            .ok_or_else(|| anyhow::anyhow!("prepared compact slot disappeared before promotion"))?;
        Ok(AdamPreparedCompactPromotion::Promoted(promoted))
    }

    /// Promotes disposable prepared state only when authorized by the exact
    /// canonical LGP commit identity.
    pub async fn promote_prepared_compact_commit(
        &self,
        commit: &polar_inhouse_contract::InhouseCompactionCommit,
    ) -> anyhow::Result<AdamPreparedCompactPromotion> {
        let Some(prepared) = self
            .store
            .prepared_compact_slot(&commit.candidate_compaction_id)
        else {
            return Ok(AdamPreparedCompactPromotion::Missing {
                candidate_compaction_id: commit.candidate_compaction_id.clone(),
                cache_plan: AdamCachePlan::ColdPrefill {
                    reason: "committed_compaction_was_not_prepared".to_string(),
                },
            });
        };
        let prepared_exact_prefix_hash = prepared.exact_prefix_hash.clone();
        let mismatch = [
            ("thread_id", prepared.thread_id == commit.thread_id),
            (
                "candidate_compaction_id",
                prepared.candidate_compaction_id == commit.candidate_compaction_id,
            ),
            (
                "source_digest",
                prepared.source_digest_before == commit.source_digest,
            ),
            (
                "context_compaction_epoch",
                prepared.target_context_compaction_epoch == commit.context_compaction_epoch,
            ),
            (
                "committed_compaction_id",
                !commit.committed_compaction_id.trim().is_empty(),
            ),
        ]
        .into_iter()
        .find_map(|(field, matches)| (!matches).then(|| format!("commit_mismatch:{field}")));
        if let Some(reason) = mismatch {
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::RewarmPrefix {
                    exact_prefix_hash: prepared_exact_prefix_hash,
                },
            });
        }
        let Some(endpoint) = self.store.endpoints().into_iter().find(|endpoint| {
            endpoint.endpoint_id == prepared.endpoint_id
                && endpoint.runtime_instance_id == prepared.runtime_instance_id
                && endpoint.model_id == prepared.model_id
                && endpoint.tokenizer_hash == prepared.tokenizer_hash
                && endpoint.context_params_hash == prepared.context_params_hash
                && endpoint.adam_managed_only
        }) else {
            let reason = "prepared_mismatch:endpoint_identity".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::ColdPrefill {
                    reason: "prepared_endpoint_identity_changed".to_string(),
                },
            });
        };
        if !self
            .endpoint_identity_is_current(&endpoint, "prepared compact commit promotion")
            .await?
        {
            let reason = "prepared_mismatch:runtime_identity_changed".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::ColdPrefill {
                    reason: "prepared_endpoint_identity_changed".to_string(),
                },
            });
        }
        self.operations
            .promote_prepared_compact(&endpoint, &prepared)
            .await?;
        if !self
            .endpoint_identity_is_current(&endpoint, "prepared compact commit promotion")
            .await?
        {
            let reason = "prepared_mismatch:runtime_identity_changed".to_string();
            let discarded = self
                .discard_prepared_compact(&prepared.candidate_compaction_id, &reason)
                .unwrap_or(prepared);
            return Ok(AdamPreparedCompactPromotion::Discarded {
                prepared: Box::new(discarded),
                reason,
                cache_plan: AdamCachePlan::ColdPrefill {
                    reason: "prepared_endpoint_identity_changed".to_string(),
                },
            });
        }
        let promoted = self
            .store
            .promote_prepared_compact_slot(
                &commit.candidate_compaction_id,
                &commit.committed_compaction_id,
            )
            .ok_or_else(|| anyhow::anyhow!("prepared compact slot disappeared before commit"))?;
        Ok(AdamPreparedCompactPromotion::Promoted(promoted))
    }

    pub fn invalidate_committed_compaction_epoch(
        &self,
        inhouse_polar_id: &str,
        thread_id: &str,
        session_id: &str,
        context_compaction_epoch: i64,
    ) -> usize {
        self.store.invalidate_live_residencies_for_context_epoch(
            inhouse_polar_id,
            thread_id,
            context_compaction_epoch,
        ) + self.invalidate_prepared_compact_slots_for_context_epoch(
            session_id,
            context_compaction_epoch,
        )
    }

    pub fn discard_prepared_compact(
        &self,
        candidate_compaction_id: &str,
        reason: &str,
    ) -> Option<AdamPreparedCompactSlot> {
        self.store
            .discard_prepared_compact_slot(candidate_compaction_id, reason)
    }

    pub fn invalidate_prepared_compact_slots_for_context_epoch(
        &self,
        session_id: &str,
        context_compaction_epoch: i64,
    ) -> usize {
        let stale = self
            .store
            .prepared_compact_slots_for_session(session_id)
            .into_iter()
            .filter(|prepared| {
                prepared.discard_reason.is_none()
                    && prepared.target_context_compaction_epoch != context_compaction_epoch
            })
            .map(|prepared| prepared.candidate_compaction_id)
            .collect::<Vec<_>>();
        for candidate_compaction_id in &stale {
            self.discard_prepared_compact(
                candidate_compaction_id,
                "context_compacted_epoch_mismatch",
            );
        }
        stale.len()
    }

    /// A promoted prepared prefix is reusable only for its exact wire prefix.
    /// Once the next user message changes that prefix, release the physical
    /// protection so foreground scheduling cannot deadlock behind an
    /// optimization artifact.
    pub fn discard_nonmatching_promoted_prefixes(
        &self,
        session_id: &str,
        exact_prefix_hash: &str,
    ) -> usize {
        let stale = self
            .store
            .prepared_compact_slots_for_session(session_id)
            .into_iter()
            .filter(|prepared| {
                prepared.promoted
                    && prepared.discard_reason.is_none()
                    && prepared.exact_prefix_hash != exact_prefix_hash
            })
            .map(|prepared| prepared.candidate_compaction_id)
            .collect::<Vec<_>>();
        for candidate_compaction_id in &stale {
            self.discard_prepared_compact(
                candidate_compaction_id,
                "promoted_prefix_changed_before_foreground_turn",
            );
        }
        stale.len()
    }

    fn compatible_endpoint(
        &self,
        intent: &AdamEngineControlIntent,
    ) -> anyhow::Result<AdamManagedLlamaEndpoint> {
        if intent.managed_endpoint_id.trim().is_empty() {
            anyhow::bail!("Adam slot control requires an opaque managed_endpoint_id binding");
        }
        if intent.transport != "llamacpp_slots" {
            anyhow::bail!(
                "Adam slot control requires native llamacpp_slots transport, got {}",
                intent.transport
            );
        }
        let connector_base_url = normalize_connector_url(&intent.connector_base_url);
        if connector_base_url.is_empty() {
            anyhow::bail!("Adam slot control requires a connector base URL identity");
        }
        // Runner compatibility hashes are cached observations. Select only the
        // exact host-owned resource here; `allocate_internal` probes it before
        // using any live residency or snapshot and replaces both hashes with
        // the physical endpoint's current identity.
        self.store
            .endpoints()
            .into_iter()
            .find(|endpoint| {
                endpoint.adam_managed_only
                    && endpoint.allows_backend("adam")
                    && endpoint.endpoint_id == intent.managed_endpoint_id
                    && endpoint.transport == "llamacpp_slots"
                    && normalize_connector_url(&endpoint.connector_base_url) == connector_base_url
                    && endpoint.model_id == intent.model_id
                    && endpoint.slot_range().is_some()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no Adam-managed native endpoint for URL {} model {} tokenizer {}",
                    connector_base_url,
                    intent.model_id,
                    intent.tokenizer_hash
                )
            })
    }

    fn compatible_prepared_endpoint(
        &self,
        intent: &AdamPreparedCompactIntent,
    ) -> anyhow::Result<AdamManagedLlamaEndpoint> {
        if intent.managed_endpoint_id.trim().is_empty() {
            anyhow::bail!(
                "Adam prepared-prefix control requires an opaque managed_endpoint_id binding"
            );
        }
        if intent.context_params_hash.trim().is_empty() {
            anyhow::bail!("Adam prepared-prefix control requires a nonblank context_params_hash");
        }
        self.store
            .endpoints()
            .into_iter()
            .find(|endpoint| {
                endpoint.adam_managed_only
                    && endpoint.allows_backend("adam")
                    && endpoint.endpoint_id == intent.managed_endpoint_id
                    && endpoint.transport == "llamacpp_slots"
                    && !normalize_connector_url(&endpoint.connector_base_url).is_empty()
                    && endpoint.model_id == intent.model_id
                    && endpoint.tokenizer_hash == intent.tokenizer_hash
                    && !endpoint.context_params_hash.trim().is_empty()
                    && endpoint.context_params_hash == intent.context_params_hash
                    && endpoint.slot_range().is_some()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no Adam-managed native endpoint for prepared model {} tokenizer {}",
                    intent.model_id,
                    intent.tokenizer_hash
                )
            })
    }
}

pub fn normalize_connector_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

impl AdamCachePlan {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UseLiveSlot { .. } => "UseLiveSlot",
            Self::LoadSnapshot { .. } => "LoadSnapshot",
            Self::RewarmPrefix { .. } => "RewarmPrefix",
            Self::ColdPrefill { .. } => "ColdPrefill",
        }
    }
}

fn prepared_match_mismatch(
    prepared: &AdamPreparedCompactSlot,
    approved: &AdamPreparedCompactIntent,
    endpoint: &AdamManagedLlamaEndpoint,
) -> Option<String> {
    let checks = [
        (
            "candidate_compaction_id",
            prepared.candidate_compaction_id == approved.candidate_compaction_id,
        ),
        (
            "source_digest_before",
            prepared.source_digest_before == approved.source_digest_before,
        ),
        (
            "checkpoint_source_ref",
            prepared.checkpoint_source_ref == approved.checkpoint_source_ref,
        ),
        (
            "exact_prefix_hash",
            prepared.exact_prefix_hash == approved.exact_prefix_hash,
        ),
        (
            "target_context_compaction_epoch",
            prepared.target_context_compaction_epoch == approved.target_context_compaction_epoch,
        ),
        ("model_id", prepared.model_id == approved.model_id),
        (
            "tokenizer_hash",
            prepared.tokenizer_hash == approved.tokenizer_hash,
        ),
        (
            "context_params_hash",
            prepared.context_params_hash == approved.context_params_hash,
        ),
        (
            "runtime_instance_id",
            prepared.runtime_instance_id == endpoint.runtime_instance_id,
        ),
        ("endpoint_model_id", prepared.model_id == endpoint.model_id),
        (
            "endpoint_tokenizer_hash",
            prepared.tokenizer_hash == endpoint.tokenizer_hash,
        ),
        (
            "endpoint_context_params_hash",
            prepared.context_params_hash == endpoint.context_params_hash
                && approved.context_params_hash == endpoint.context_params_hash,
        ),
    ];
    checks
        .into_iter()
        .find_map(|(field, matches)| (!matches).then(|| format!("prepared_mismatch:{field}")))
}

fn same_physical_identity(left: &AdamLlamaEngineProbe, right: &AdamLlamaEngineProbe) -> bool {
    left.runtime_instance_id == right.runtime_instance_id
        && left.tokenizer_hash == right.tokenizer_hash
        && left.context_params_hash == right.context_params_hash
}

fn probe_matches_endpoint_identity(
    probe: &AdamLlamaEngineProbe,
    endpoint: &AdamManagedLlamaEndpoint,
) -> bool {
    probe.runtime_instance_id == endpoint.runtime_instance_id
        && probe.tokenizer_hash == endpoint.tokenizer_hash
        && probe.context_params_hash == endpoint.context_params_hash
}

fn verify_exact_prefix_payload(intent: &AdamPreparedCompactIntent) -> anyhow::Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(intent.exact_prefix_payload.as_bytes());
    let actual_hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    if actual_hash != intent.exact_prefix_hash {
        anyhow::bail!("Adam prepared-prefix exact_prefix_hash does not match exact_prefix_payload");
    }
    Ok(())
}

fn short_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
