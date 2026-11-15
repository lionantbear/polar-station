use crate::{causal_identity::AdamToolInvocationEvidence, now_unix_ms, status::AdamRunReport};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Canonical Polar thread binding for one Adam runtime session.
///
/// The key points back to Polar-owned thread/profile state. It does not contain
/// transcript text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdamSessionBindingKey {
    pub owner_id: String,
    pub thread_id: String,
    pub inhouse_polar_id: String,
    pub backend_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamSessionBinding {
    pub key: AdamSessionBindingKey,
    pub session_id: String,
    pub llm_connection_mode: String,
    pub llm_transport: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl AdamSessionBinding {
    pub fn new(
        key: AdamSessionBindingKey,
        session_id: impl Into<String>,
        llm_connection_mode: impl Into<String>,
        llm_transport: Option<String>,
    ) -> Self {
        let now = now_unix_ms();
        Self {
            key,
            session_id: session_id.into(),
            llm_connection_mode: llm_connection_mode.into(),
            llm_transport,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamProjectionLookup {
    pub session_id: String,
    pub source_digest: String,
    pub context_compaction_epoch: i64,
    pub renderer_version: String,
    pub model_id: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub exact_prefix_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamRenderProjection {
    pub projection_id: String,
    pub session_id: String,
    pub canonical_source_refs: Vec<String>,
    pub source_digest: String,
    pub context_compaction_epoch: i64,
    pub prompt_asset_revision: String,
    pub renderer_version: String,
    pub model_id: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub exact_prefix_hash: String,
    pub input_hash: String,
    pub token_count: Option<i64>,
    pub valid: bool,
    pub invalidation_reason: Option<String>,
    pub created_at_unix_ms: i64,
}

impl AdamRenderProjection {
    pub fn matches_lookup(&self, lookup: &AdamProjectionLookup) -> bool {
        self.valid
            && self.session_id == lookup.session_id
            && self.source_digest == lookup.source_digest
            && self.context_compaction_epoch == lookup.context_compaction_epoch
            && self.renderer_version == lookup.renderer_version
            && self.model_id == lookup.model_id
            && self.template_hash == lookup.template_hash
            && self.tool_schema_hash == lookup.tool_schema_hash
            && self.exact_prefix_hash == lookup.exact_prefix_hash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamTurnAttemptStatus {
    Started,
    Completed,
    BackendBusy,
    Failed,
    Interrupted,
}

impl AdamTurnAttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::BackendBusy => "backend_busy",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamTurnAttempt {
    pub attempt_id: String,
    pub session_id: String,
    pub request_message_id: String,
    pub render_projection_id: Option<String>,
    pub status: AdamTurnAttemptStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_attempt_ref: Option<String>,
    pub provider_response_id: Option<String>,
    pub error_kind: Option<String>,
    pub error_summary: Option<String>,
    pub busy_kind: Option<String>,
    pub started_at_unix_ms: i64,
    pub ended_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamLlamaCacheEvidence {
    pub evidence_id: String,
    pub session_id: String,
    pub render_projection_id: Option<String>,
    pub slot_id: i64,
    pub prompt_cache_key: Option<String>,
    pub cache_hit_tokens: Option<i64>,
    pub cache_miss_tokens: Option<i64>,
    pub warmed_prefix_hash: String,
    pub model_label: String,
    pub endpoint_fingerprint: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamCacheEpochState {
    Prepared,
    Active,
    Invalidated,
}

impl AdamCacheEpochState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Active => "active",
            Self::Invalidated => "invalidated",
        }
    }

    /// Parses the stable persisted representation used by storage adapters.
    pub fn parse_persisted(value: &str) -> anyhow::Result<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "active" => Ok(Self::Active),
            "invalidated" => Ok(Self::Invalidated),
            other => anyhow::bail!("unknown Adam cache epoch state: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamProviderUsageCounters {
    pub input_uncached: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub output: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamUsageAccountingSource {
    ActualProvider,
    ProviderReported,
    TokenizerEstimate,
    CharEstimate,
}

impl AdamUsageAccountingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActualProvider => "actual_provider",
            Self::ProviderReported => "provider_reported",
            Self::TokenizerEstimate => "tokenizer_estimate",
            Self::CharEstimate => "char_estimate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamCapabilityProfileSnapshot {
    pub backend_profile_id: String,
    pub provider: String,
    pub streaming_answer: bool,
    pub streaming_thinking: bool,
    pub provider_native_tools: bool,
    pub strict_json_tool_envelope_fallback: bool,
    pub json_schema_support: bool,
    pub cache_telemetry: bool,
    pub cache_hinting: bool,
    pub runtime_control: bool,
    pub context_window_probe: bool,
    pub context_window_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamPreparedCompactSlotRecord {
    pub candidate_compaction_id: String,
    pub target_context_epoch: i64,
    pub compact_summary_source_ref: Option<String>,
    pub retained_tail_checkpoint_source_ref: String,
    pub prepared_prefix_hash: String,
    pub prepared_slot_id: i64,
    pub promoted_at_unix_ms: Option<i64>,
    pub discard_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamCacheEpoch {
    pub cache_epoch_id: String,
    pub session_id: String,
    pub render_projection_id: Option<String>,
    pub state: AdamCacheEpochState,
    pub connection_mode: String,
    pub transport: Option<String>,
    pub provider_response_id: Option<String>,
    pub llama_slot_id: Option<i64>,
    pub endpoint_fingerprint: Option<String>,
    pub model_label: Option<String>,
    pub warmed_prefix_hash: String,
    pub source_digest: String,
    pub context_compaction_epoch: i64,
    pub renderer_version: String,
    pub model_id: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub exact_prefix_hash: String,
    pub capability_profile_snapshot: Option<AdamCapabilityProfileSnapshot>,
    pub prepared_compact_slot: Option<AdamPreparedCompactSlotRecord>,
    pub invalidation_reason: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl AdamCacheEpoch {
    fn matches_lookup(&self, lookup: &AdamCacheEpochLookup) -> bool {
        self.state == AdamCacheEpochState::Active
            && self.session_id == lookup.session_id
            && self.source_digest == lookup.source_digest
            && self.context_compaction_epoch == lookup.context_compaction_epoch
            && self.renderer_version == lookup.renderer_version
            && self.model_id == lookup.model_id
            && self.template_hash == lookup.template_hash
            && self.tool_schema_hash == lookup.tool_schema_hash
            && self.exact_prefix_hash == lookup.exact_prefix_hash
            && self.connection_mode == lookup.connection_mode
            && self.transport == lookup.transport
            && self.endpoint_fingerprint == lookup.endpoint_fingerprint
            && self.model_label == lookup.model_label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamCacheEpochLookup {
    pub session_id: String,
    pub source_digest: String,
    pub context_compaction_epoch: i64,
    pub renderer_version: String,
    pub model_id: String,
    pub template_hash: String,
    pub tool_schema_hash: String,
    pub exact_prefix_hash: String,
    pub connection_mode: String,
    pub transport: Option<String>,
    pub endpoint_fingerprint: Option<String>,
    pub model_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamProviderUsageTelemetry {
    pub telemetry_id: String,
    pub session_id: String,
    pub cache_epoch_id: Option<String>,
    pub turn_attempt_id: Option<String>,
    pub provider: String,
    pub model_id: String,
    pub counters: AdamProviderUsageCounters,
    pub accounting_source: AdamUsageAccountingSource,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdamCompactionAdmissionDecision {
    pub decision_id: String,
    pub session_id: String,
    pub candidate_compaction_id: Option<String>,
    pub admitted: bool,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub evidence_json: serde_json::Value,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamToolCallCorrelation {
    pub correlation_id: String,
    pub session_id: String,
    pub provider_call_id: String,
    pub polar_tool_call_ref: Option<String>,
    pub polar_tool_result_ref: Option<String>,
    pub status: String,
    pub error_summary: Option<String>,
    pub created_at_unix_ms: i64,
}

#[async_trait]
pub trait AdamRuntimeProjectionStore: Send + Sync {
    /// Backend-native, model-free projection for one Adam session.
    async fn inspect_session_projection(
        &self,
        session_id: &str,
    ) -> anyhow::Result<serde_json::Value>;

    async fn upsert_session_binding(&self, binding: AdamSessionBinding) -> anyhow::Result<()>;

    async fn session_binding(
        &self,
        key: &AdamSessionBindingKey,
    ) -> anyhow::Result<Option<AdamSessionBinding>>;

    async fn put_render_projection(&self, projection: AdamRenderProjection) -> anyhow::Result<()>;

    async fn latest_valid_render_projection(
        &self,
        lookup: AdamProjectionLookup,
    ) -> anyhow::Result<Option<AdamRenderProjection>>;

    async fn invalidate_render_projections(
        &self,
        session_id: &str,
        reason: &str,
    ) -> anyhow::Result<usize>;

    async fn latest_context_compaction_epoch(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<i64>>;

    async fn record_turn_attempt(&self, attempt: AdamTurnAttempt) -> anyhow::Result<()>;

    async fn record_llama_cache_evidence(
        &self,
        evidence: AdamLlamaCacheEvidence,
    ) -> anyhow::Result<()>;

    async fn record_cache_epoch(&self, epoch: AdamCacheEpoch) -> anyhow::Result<()>;

    async fn latest_cache_epoch(
        &self,
        lookup: AdamCacheEpochLookup,
    ) -> anyhow::Result<Option<AdamCacheEpoch>>;

    async fn latest_prepared_compact_epoch(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>>;

    async fn invalidate_cache_epochs(
        &self,
        session_id: &str,
        reason: &str,
    ) -> anyhow::Result<usize>;

    async fn promote_prepared_compact_epoch(
        &self,
        cache_epoch_id: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>>;

    async fn discard_prepared_compact_epoch(
        &self,
        cache_epoch_id: &str,
        reason: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>>;

    async fn record_usage_telemetry(
        &self,
        telemetry: AdamProviderUsageTelemetry,
    ) -> anyhow::Result<()>;

    async fn record_compaction_admission_decision(
        &self,
        decision: AdamCompactionAdmissionDecision,
    ) -> anyhow::Result<()>;

    async fn record_tool_call_correlation(
        &self,
        correlation: AdamToolCallCorrelation,
    ) -> anyhow::Result<()>;

    async fn record_tool_invocation_evidence(
        &self,
        evidence: AdamToolInvocationEvidence,
    ) -> anyhow::Result<()>;

    async fn record_run_report(&self, report: AdamRunReport) -> anyhow::Result<()>;

    async fn recent_run_reports(&self, limit: usize) -> anyhow::Result<Vec<AdamRunReport>>;
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryAdamRuntimeProjectionStore {
    inner: Arc<Mutex<InMemoryProjectionState>>,
}

#[derive(Debug, Default)]
struct InMemoryProjectionState {
    bindings: HashMap<AdamSessionBindingKey, AdamSessionBinding>,
    projections: HashMap<String, AdamRenderProjection>,
    attempts: HashMap<String, AdamTurnAttempt>,
    llama_evidence: HashMap<String, AdamLlamaCacheEvidence>,
    cache_epochs: HashMap<String, AdamCacheEpoch>,
    usage_telemetry: HashMap<String, AdamProviderUsageTelemetry>,
    compaction_admission_decisions: HashMap<String, AdamCompactionAdmissionDecision>,
    tool_correlations: HashMap<String, AdamToolCallCorrelation>,
    tool_invocations: HashMap<String, AdamToolInvocationEvidence>,
    tool_invocation_writes: Vec<AdamToolInvocationEvidence>,
    run_reports: HashMap<String, AdamRunReport>,
}

impl InMemoryAdamRuntimeProjectionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool_call_correlations(&self) -> Vec<AdamToolCallCorrelation> {
        let mut correlations = self
            .inner
            .lock()
            .expect("projection store lock")
            .tool_correlations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        correlations.sort_by(|left, right| left.correlation_id.cmp(&right.correlation_id));
        correlations
    }

    pub fn attempts_snapshot(&self) -> Vec<AdamTurnAttempt> {
        self.inner
            .lock()
            .expect("projection store lock")
            .attempts
            .values()
            .cloned()
            .collect()
    }

    pub fn tool_invocations_snapshot(&self) -> Vec<AdamToolInvocationEvidence> {
        let mut invocations = self
            .inner
            .lock()
            .expect("projection store lock")
            .tool_invocations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        invocations.sort_by(|left, right| left.invocation_id.cmp(&right.invocation_id));
        invocations
    }

    pub fn tool_invocation_writes(&self) -> Vec<AdamToolInvocationEvidence> {
        self.inner
            .lock()
            .expect("projection store lock")
            .tool_invocation_writes
            .clone()
    }


    pub fn llama_evidence_snapshot(&self) -> Vec<AdamLlamaCacheEvidence> {
        self.inner
            .lock()
            .expect("projection store lock")
            .llama_evidence
            .values()
            .cloned()
            .collect()
    }

    pub fn render_projections_snapshot(&self) -> Vec<AdamRenderProjection> {
        self.inner
            .lock()
            .expect("projection store lock")
            .projections
            .values()
            .cloned()
            .collect()
    }

    pub fn cache_epochs_snapshot(&self) -> Vec<AdamCacheEpoch> {
        self.inner
            .lock()
            .expect("projection store lock")
            .cache_epochs
            .values()
            .cloned()
            .collect()
    }

    pub fn run_reports_snapshot(&self) -> Vec<AdamRunReport> {
        self.inner
            .lock()
            .expect("projection store lock")
            .run_reports
            .values()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl AdamRuntimeProjectionStore for InMemoryAdamRuntimeProjectionStore {
    async fn inspect_session_projection(
        &self,
        session_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let inner = self.inner.lock().expect("projection store lock");
        Ok(serde_json::json!({
            "schema": "polar.adam-native-session-projection.v1",
            "session_id": session_id,
            "render_projections": inner.projections.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "turn_attempts": inner.attempts.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "cache_evidence": inner.llama_evidence.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "cache_epochs": inner.cache_epochs.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "usage_telemetry": inner.usage_telemetry.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "compaction_admission": inner.compaction_admission_decisions.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "tool_correlations": inner.tool_correlations.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "tool_invocations": inner.tool_invocations.values().filter(|row| row.session_id == session_id).cloned().collect::<Vec<_>>(),
            "run_reports": inner.run_reports.values().filter(|row| serde_json::to_value(row).ok().and_then(|value| value.get("session_id").and_then(serde_json::Value::as_str).map(|value| value == session_id)).unwrap_or(false)).cloned().collect::<Vec<_>>(),
        }))
    }

    async fn upsert_session_binding(&self, mut binding: AdamSessionBinding) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().expect("projection store lock");
        if let Some(existing) = inner.bindings.get(&binding.key) {
            binding.created_at_unix_ms = existing.created_at_unix_ms;
        }
        binding.updated_at_unix_ms = now_unix_ms();
        inner.bindings.insert(binding.key.clone(), binding);
        Ok(())
    }

    async fn session_binding(
        &self,
        key: &AdamSessionBindingKey,
    ) -> anyhow::Result<Option<AdamSessionBinding>> {
        Ok(self
            .inner
            .lock()
            .expect("projection store lock")
            .bindings
            .get(key)
            .cloned())
    }

    async fn put_render_projection(&self, projection: AdamRenderProjection) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .projections
            .insert(projection.projection_id.clone(), projection);
        Ok(())
    }

    async fn latest_valid_render_projection(
        &self,
        lookup: AdamProjectionLookup,
    ) -> anyhow::Result<Option<AdamRenderProjection>> {
        let inner = self.inner.lock().expect("projection store lock");
        Ok(inner
            .projections
            .values()
            .filter(|projection| projection.matches_lookup(&lookup))
            .max_by_key(|projection| projection.created_at_unix_ms)
            .cloned())
    }

    async fn invalidate_render_projections(
        &self,
        session_id: &str,
        reason: &str,
    ) -> anyhow::Result<usize> {
        let mut count = 0;
        let mut inner = self.inner.lock().expect("projection store lock");
        for projection in inner.projections.values_mut() {
            if projection.session_id == session_id && projection.valid {
                projection.valid = false;
                projection.invalidation_reason = Some(reason.to_string());
                count += 1;
            }
        }
        Ok(count)
    }

    async fn latest_context_compaction_epoch(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<i64>> {
        let inner = self.inner.lock().expect("projection store lock");
        Ok(inner
            .projections
            .values()
            .filter(|projection| projection.session_id == session_id)
            .map(|projection| projection.context_compaction_epoch)
            .chain(
                inner
                    .cache_epochs
                    .values()
                    .filter(|epoch| epoch.session_id == session_id)
                    .map(|epoch| epoch.context_compaction_epoch),
            )
            .max())
    }

    async fn record_turn_attempt(&self, attempt: AdamTurnAttempt) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .attempts
            .insert(attempt.attempt_id.clone(), attempt);
        Ok(())
    }

    async fn record_llama_cache_evidence(
        &self,
        evidence: AdamLlamaCacheEvidence,
    ) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .llama_evidence
            .insert(evidence.evidence_id.clone(), evidence);
        Ok(())
    }

    async fn record_cache_epoch(&self, mut epoch: AdamCacheEpoch) -> anyhow::Result<()> {
        let now = now_unix_ms();
        if epoch.created_at_unix_ms == 0 {
            epoch.created_at_unix_ms = now;
        }
        epoch.updated_at_unix_ms = now;
        self.inner
            .lock()
            .expect("projection store lock")
            .cache_epochs
            .insert(epoch.cache_epoch_id.clone(), epoch);
        Ok(())
    }

    async fn latest_cache_epoch(
        &self,
        lookup: AdamCacheEpochLookup,
    ) -> anyhow::Result<Option<AdamCacheEpoch>> {
        let inner = self.inner.lock().expect("projection store lock");
        Ok(inner
            .cache_epochs
            .values()
            .filter(|epoch| epoch.matches_lookup(&lookup))
            .max_by_key(|epoch| epoch.updated_at_unix_ms)
            .cloned())
    }

    async fn latest_prepared_compact_epoch(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>> {
        let inner = self.inner.lock().expect("projection store lock");
        Ok(inner
            .cache_epochs
            .values()
            .filter(|epoch| {
                epoch.session_id == session_id
                    && epoch.state == AdamCacheEpochState::Prepared
                    && epoch.prepared_compact_slot.is_some()
            })
            .max_by_key(|epoch| epoch.updated_at_unix_ms)
            .cloned())
    }

    async fn invalidate_cache_epochs(
        &self,
        session_id: &str,
        reason: &str,
    ) -> anyhow::Result<usize> {
        let now = now_unix_ms();
        let mut count = 0;
        let mut inner = self.inner.lock().expect("projection store lock");
        for epoch in inner.cache_epochs.values_mut() {
            if epoch.session_id == session_id && epoch.state != AdamCacheEpochState::Invalidated {
                epoch.state = AdamCacheEpochState::Invalidated;
                epoch.invalidation_reason = Some(reason.to_string());
                epoch.updated_at_unix_ms = now;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn promote_prepared_compact_epoch(
        &self,
        cache_epoch_id: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>> {
        let now = now_unix_ms();
        let mut inner = self.inner.lock().expect("projection store lock");
        let Some(epoch) = inner.cache_epochs.get_mut(cache_epoch_id) else {
            return Ok(None);
        };
        if epoch.state != AdamCacheEpochState::Prepared {
            return Ok(Some(epoch.clone()));
        }
        epoch.state = AdamCacheEpochState::Active;
        epoch.updated_at_unix_ms = now;
        if let Some(prepared) = &mut epoch.prepared_compact_slot {
            prepared.promoted_at_unix_ms = Some(now);
            prepared.discard_reason = None;
        }
        Ok(Some(epoch.clone()))
    }

    async fn discard_prepared_compact_epoch(
        &self,
        cache_epoch_id: &str,
        reason: &str,
    ) -> anyhow::Result<Option<AdamCacheEpoch>> {
        let now = now_unix_ms();
        let mut inner = self.inner.lock().expect("projection store lock");
        let Some(epoch) = inner.cache_epochs.get_mut(cache_epoch_id) else {
            return Ok(None);
        };
        epoch.state = AdamCacheEpochState::Invalidated;
        epoch.invalidation_reason = Some(reason.to_string());
        epoch.updated_at_unix_ms = now;
        if let Some(prepared) = &mut epoch.prepared_compact_slot {
            prepared.discard_reason = Some(reason.to_string());
        }
        Ok(Some(epoch.clone()))
    }

    async fn record_usage_telemetry(
        &self,
        telemetry: AdamProviderUsageTelemetry,
    ) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .usage_telemetry
            .insert(telemetry.telemetry_id.clone(), telemetry);
        Ok(())
    }

    async fn record_compaction_admission_decision(
        &self,
        decision: AdamCompactionAdmissionDecision,
    ) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .compaction_admission_decisions
            .insert(decision.decision_id.clone(), decision);
        Ok(())
    }

    async fn record_tool_call_correlation(
        &self,
        correlation: AdamToolCallCorrelation,
    ) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .tool_correlations
            .insert(correlation.correlation_id.clone(), correlation);
        Ok(())
    }

    async fn record_tool_invocation_evidence(
        &self,
        evidence: AdamToolInvocationEvidence,
    ) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().expect("projection store lock");
        inner.tool_invocation_writes.push(evidence.clone());
        inner
            .tool_invocations
            .insert(evidence.invocation_id.clone(), evidence);
        Ok(())
    }

    async fn record_run_report(&self, report: AdamRunReport) -> anyhow::Result<()> {
        self.inner
            .lock()
            .expect("projection store lock")
            .run_reports
            .insert(report.run_id.clone(), report);
        Ok(())
    }

    async fn recent_run_reports(&self, limit: usize) -> anyhow::Result<Vec<AdamRunReport>> {
        let mut reports = self
            .inner
            .lock()
            .expect("projection store lock")
            .run_reports
            .values()
            .cloned()
            .collect::<Vec<_>>();
        reports.sort_by(|left, right| right.run_id.cmp(&left.run_id));
        reports.truncate(limit);
        Ok(reports)
    }
}


