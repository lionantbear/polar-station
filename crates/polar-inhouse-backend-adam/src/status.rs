//! Redacted operator status for the Adam-managed engine pool.

use crate::{
    AdamCacheScheduler, AdamEngineAllocation, AdamEngineOperations, AdamEngineStore,
    AdamWaitQueueStatus,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEffectiveEngineCapabilities {
    pub supports_llama_slot_control: bool,
    pub supports_kv_residency: bool,
    pub supports_kv_snapshot_save: bool,
    pub supports_kv_snapshot_load: bool,
    pub supports_slot_erase: bool,
    pub supports_cache_affinity: bool,
    pub supports_runtime_evidence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEndpointStatus {
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub model_id: String,
    pub slot_offset: i64,
    pub slot_count: i64,
    pub idle_slot_ids: Vec<i64>,
    pub busy_slot_ids: Vec<i64>,
    pub effective_capabilities: AdamEffectiveEngineCapabilities,
    pub probe_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamResidencyStatus {
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub projection_id: String,
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub slot_id: i64,
    pub context_compaction_epoch: i64,
    pub residency_source: String,
    pub allocation_reason: String,
    pub last_activity_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEngineStatusReport {
    pub generated_at_unix_ms: i64,
    pub endpoints: Vec<AdamEndpointStatus>,
    pub residencies: Vec<AdamResidencyStatus>,
    #[serde(default)]
    pub wait_queue: Vec<AdamWaitQueueStatus>,
    pub active_lease_count: usize,
    pub snapshot_count: usize,
    pub evidence_count: usize,
    #[serde(default)]
    pub recent_runs: Vec<AdamRunReport>,
}

/// Redacted per-turn evidence suitable for operator support workflows.
///
/// Message identifiers remain LGP-owned references. This report deliberately
/// excludes prompt text, connector URLs, credentials, snapshot paths, and
/// compatibility hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamRunReport {
    pub run_id: String,
    pub thread_id: String,
    pub request_message_id: String,
    pub reply_message_id: String,
    pub projection_id: String,
    pub cache_plan: String,
    pub allocation_reason: String,
    #[serde(default)]
    pub eviction_decision: String,
    #[serde(default)]
    pub protected_activity_ttl_ms: i64,
    #[serde(default)]
    pub scheduling_priority: String,
    #[serde(default)]
    pub scheduler_request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_wait_queue_position: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_wait_queue_depth: Option<usize>,
    #[serde(default)]
    pub wait_observation_count: u32,
    #[serde(default)]
    pub scheduler_waited_ms: i64,
    #[serde(default)]
    pub scheduler_wait_limit_ms: i64,
    #[serde(default)]
    pub admitted_after_wait: bool,
    #[serde(default)]
    pub evicted_active_residency: bool,
    #[serde(default)]
    pub snapshot_before_eviction: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_runtime_instance_id: Option<String>,
    #[serde(default)]
    pub runtime_generation_changed: bool,
    #[serde(default)]
    pub old_runtime_invalidated: bool,
    #[serde(default)]
    pub cache_epoch_transition: String,
    pub lease_id: String,
    pub endpoint_id: String,
    pub slot_id: i64,
    pub runtime_instance_id: String,
    #[serde(default)]
    pub reported_at_unix_ms: i64,
    #[serde(default)]
    pub lease_generation: i64,
    #[serde(default)]
    pub lease_expires_at_unix_ms: i64,
    #[serde(default)]
    pub prompt_source_ref_count: usize,
    #[serde(default)]
    pub context_compaction_epoch: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_call_ids: Vec<String>,
    #[serde(default)]
    pub tool_result_count: usize,
    #[serde(default)]
    pub provider_cache_read_tokens: i64,
    #[serde(default)]
    pub provider_cache_write_tokens: i64,
    #[serde(default)]
    pub provider_input_uncached_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cache_hit_rate_basis_points: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cache_hit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub snapshot_restored: bool,
    #[serde(default)]
    pub final_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdamRunCompletionFacts {
    pub prompt_source_ref_count: usize,
    pub context_compaction_epoch: i64,
    pub compaction_id: Option<String>,
    pub tool_call_ids: Vec<String>,
    pub tool_result_count: usize,
    pub provider_cache_read_tokens: i64,
    pub provider_cache_write_tokens: i64,
    pub provider_input_uncached_tokens: i64,
    pub provider_cache_hit_rate_basis_points: Option<u32>,
    pub provider_cache_hit: Option<bool>,
    pub final_status: String,
    pub tool_error_kind: Option<String>,
    pub provider_error_kind: Option<String>,
    pub provider_response_id: Option<String>,
}

impl AdamRunReport {
    pub fn from_allocation(
        run_id: impl Into<String>,
        request_message_id: impl Into<String>,
        reply_message_id: impl Into<String>,
        allocation: &AdamEngineAllocation,
    ) -> Self {
        Self::from_completed_turn(
            run_id,
            request_message_id,
            reply_message_id,
            allocation,
            AdamRunCompletionFacts {
                final_status: "allocated".to_string(),
                ..AdamRunCompletionFacts::default()
            },
        )
    }

    pub fn from_completed_turn(
        run_id: impl Into<String>,
        request_message_id: impl Into<String>,
        reply_message_id: impl Into<String>,
        allocation: &AdamEngineAllocation,
        facts: AdamRunCompletionFacts,
    ) -> Self {
        let (snapshot_id, snapshot_restored) = match &allocation.cache_plan {
            crate::AdamCachePlan::LoadSnapshot { snapshot } => {
                (Some(snapshot.snapshot_id.clone()), true)
            }
            _ => (None, false),
        };
        Self {
            run_id: run_id.into(),
            thread_id: allocation.intent.thread_id.clone(),
            request_message_id: request_message_id.into(),
            reply_message_id: reply_message_id.into(),
            projection_id: allocation.intent.projection_id.clone(),
            cache_plan: allocation.cache_plan.as_str().to_string(),
            allocation_reason: allocation.evidence.allocation_reason.clone(),
            eviction_decision: allocation.evidence.eviction_decision.clone(),
            protected_activity_ttl_ms: allocation.evidence.protected_activity_ttl_ms,
            scheduling_priority: allocation.evidence.scheduling_priority.clone(),
            scheduler_request_id: allocation.evidence.scheduler_request_id.clone(),
            initial_wait_queue_position: allocation.evidence.initial_wait_queue_position,
            initial_wait_queue_depth: allocation.evidence.initial_wait_queue_depth,
            wait_observation_count: allocation.evidence.wait_observation_count,
            scheduler_waited_ms: allocation.evidence.waited_ms,
            scheduler_wait_limit_ms: allocation.evidence.wait_limit_ms,
            admitted_after_wait: allocation.evidence.admitted_after_wait,
            evicted_active_residency: allocation.evidence.evicted_active_residency,
            snapshot_before_eviction: allocation.evidence.snapshot_before_eviction,
            previous_runtime_instance_id: allocation.evidence.previous_runtime_instance_id.clone(),
            runtime_generation_changed: allocation.evidence.runtime_generation_changed,
            old_runtime_invalidated: allocation.evidence.old_runtime_invalidated,
            cache_epoch_transition: allocation.evidence.cache_epoch_transition.clone(),
            lease_id: allocation.lease.lease_id.clone(),
            endpoint_id: allocation.lease.endpoint_id.clone(),
            slot_id: allocation.lease.slot_id,
            runtime_instance_id: allocation.lease.runtime_instance_id.clone(),
            reported_at_unix_ms: crate::now_unix_ms(),
            lease_generation: allocation.lease.lease_generation,
            lease_expires_at_unix_ms: allocation.lease.expires_at_unix_ms,
            prompt_source_ref_count: facts.prompt_source_ref_count,
            context_compaction_epoch: facts.context_compaction_epoch,
            compaction_id: facts.compaction_id,
            tool_call_ids: facts.tool_call_ids,
            tool_result_count: facts.tool_result_count,
            provider_cache_read_tokens: facts.provider_cache_read_tokens,
            provider_cache_write_tokens: facts.provider_cache_write_tokens,
            provider_input_uncached_tokens: facts.provider_input_uncached_tokens,
            provider_cache_hit_rate_basis_points: facts.provider_cache_hit_rate_basis_points,
            provider_cache_hit: facts.provider_cache_hit,
            snapshot_id,
            snapshot_restored,
            final_status: facts.final_status,
            tool_error_kind: facts.tool_error_kind,
            provider_error_kind: facts.provider_error_kind,
            provider_response_id: facts.provider_response_id,
        }
    }
}

pub async fn build_adam_engine_status(
    store: &dyn AdamEngineStore,
    operations: &dyn AdamEngineOperations,
) -> AdamEngineStatusReport {
    build_adam_engine_status_inner(store, operations, None).await
}

pub(crate) async fn build_adam_engine_status_with_scheduler(
    store: &dyn AdamEngineStore,
    operations: &dyn AdamEngineOperations,
    scheduler: &AdamCacheScheduler,
) -> AdamEngineStatusReport {
    build_adam_engine_status_inner(store, operations, Some(scheduler)).await
}

async fn build_adam_engine_status_inner(
    store: &dyn AdamEngineStore,
    operations: &dyn AdamEngineOperations,
    scheduler: Option<&AdamCacheScheduler>,
) -> AdamEngineStatusReport {
    let mut endpoint_statuses = Vec::new();
    let mut residency_statuses = Vec::new();
    let evidence = store.engine_evidence();
    for endpoint in store.endpoints() {
        let probe = operations.probe_endpoint(&endpoint).await;
        let (runtime_instance_id, idle_slot_ids, busy_slot_ids, capabilities, probe_error) =
            match probe {
                Ok(probe) => (
                    probe.runtime_instance_id,
                    probe.idle_slot_ids,
                    probe.busy_slot_ids,
                    AdamEffectiveEngineCapabilities {
                        supports_llama_slot_control: true,
                        supports_kv_residency: true,
                        supports_kv_snapshot_save: probe.supports_snapshot_save,
                        supports_kv_snapshot_load: probe.supports_snapshot_restore,
                        supports_slot_erase: probe.supports_slot_erase,
                        supports_cache_affinity: true,
                        supports_runtime_evidence: true,
                    },
                    None,
                ),
                Err(error) => (
                    endpoint.runtime_instance_id.clone(),
                    Vec::new(),
                    Vec::new(),
                    AdamEffectiveEngineCapabilities {
                        supports_llama_slot_control: false,
                        supports_kv_residency: false,
                        supports_kv_snapshot_save: false,
                        supports_kv_snapshot_load: false,
                        supports_slot_erase: false,
                        supports_cache_affinity: false,
                        supports_runtime_evidence: false,
                    },
                    Some(redacted_probe_error(&error.to_string())),
                ),
            };
        for residency in store.live_residencies(&endpoint.endpoint_id, &runtime_instance_id) {
            let allocation_reason = evidence
                .iter()
                .rev()
                .find(|item| {
                    item.endpoint_id == residency.endpoint_id && item.slot_id == residency.slot_id
                })
                .map(|item| item.allocation_reason.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            residency_statuses.push(AdamResidencyStatus {
                inhouse_polar_id: residency.inhouse_polar_id,
                thread_id: residency.thread_id,
                projection_id: residency.projection_id,
                endpoint_id: residency.endpoint_id,
                runtime_instance_id: runtime_instance_id.clone(),
                slot_id: residency.slot_id,
                context_compaction_epoch: residency.context_compaction_epoch,
                residency_source: "live_slot".to_string(),
                allocation_reason,
                last_activity_unix_ms: residency.last_activity_unix_ms,
            });
        }
        endpoint_statuses.push(AdamEndpointStatus {
            endpoint_id: endpoint.endpoint_id,
            runtime_instance_id,
            model_id: endpoint.model_id,
            slot_offset: endpoint.slot_offset,
            slot_count: endpoint.slot_count,
            idle_slot_ids,
            busy_slot_ids,
            effective_capabilities: capabilities,
            probe_error,
        });
    }
    let generated_at_unix_ms = crate::now_unix_ms();
    AdamEngineStatusReport {
        generated_at_unix_ms,
        endpoints: endpoint_statuses,
        residencies: residency_statuses,
        wait_queue: scheduler.map_or_else(Vec::new, AdamCacheScheduler::wait_queue_status),
        active_lease_count: store
            .slot_leases()
            .iter()
            .filter(|lease| {
                lease.released_at_unix_ms.is_none()
                    && lease.expires_at_unix_ms > generated_at_unix_ms
            })
            .count(),
        snapshot_count: store.snapshots().len(),
        evidence_count: evidence.len(),
        recent_runs: Vec::new(),
    }
}

fn redacted_probe_error(message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("timeout") || normalized.contains("timed out") {
        "endpoint_probe_timeout".to_string()
    } else if normalized.contains("connect") || normalized.contains("transport") {
        "endpoint_probe_unreachable".to_string()
    } else if normalized.contains("parse") || normalized.contains("decode") {
        "endpoint_probe_invalid_response".to_string()
    } else {
        "endpoint_probe_failed".to_string()
    }
}
