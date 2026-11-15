//! Adam-owned implementation of provider-neutral physical compaction.
//!
//! Every manager, store, projection, prepared-slot, and KV correlation remains
//! inside this backend. Shared runtime code can only invoke the neutral
//! `InhouseCompactionPhysicalPort` contract.

use crate::{
    llamacpp_wire_prompt_hash, project_adam_prepared_compact_request, AdamCacheEpoch,
    AdamCacheEpochState, AdamEngineStore, AdamInferenceEngineManager, AdamPreparedCompactIntent,
    AdamPreparedCompactPromotion, AdamPreparedCompactResult, AdamPreparedCompactSlotRecord,
    AdamRuntimeProjectionStore, LlamaWireProjectionRequest,
};
use anyhow::Result;
use async_trait::async_trait;
use polar_inhouse_contract::{
    InhouseCompactionAbort, InhouseCompactionCommit, InhouseCompactionPhysicalPort,
    InhouseCompactionPrepareDisposition, InhouseCompactionPrepareRequest,
    InhouseCompactionPreparedPromotion,
};
use std::sync::Arc;

pub(crate) struct AdamCompactionPhysicalPort {
    manager: Arc<AdamInferenceEngineManager>,
    engine_store: Arc<dyn AdamEngineStore>,
    projection_store: Arc<dyn AdamRuntimeProjectionStore>,
    inhouse_polar_id: String,
    session_id: String,
    managed_endpoint_id: Option<String>,
    enable_thinking: bool,
}

impl AdamCompactionPhysicalPort {
    pub(crate) fn new(
        manager: Arc<AdamInferenceEngineManager>,
        engine_store: Arc<dyn AdamEngineStore>,
        projection_store: Arc<dyn AdamRuntimeProjectionStore>,
        inhouse_polar_id: impl Into<String>,
        session_id: impl Into<String>,
        managed_endpoint_id: Option<String>,
        enable_thinking: bool,
    ) -> Self {
        Self {
            manager,
            engine_store,
            projection_store,
            inhouse_polar_id: inhouse_polar_id.into(),
            session_id: session_id.into(),
            managed_endpoint_id,
            enable_thinking,
        }
    }

    async fn exact_prepared_projection_epoch(
        &self,
        commit: &InhouseCompactionCommit,
    ) -> Result<Option<AdamCacheEpoch>> {
        Ok(self
            .projection_store
            .latest_prepared_compact_epoch(&self.session_id)
            .await?
            .filter(|epoch| {
                epoch.context_compaction_epoch == commit.context_compaction_epoch
                    && epoch.source_digest == commit.source_digest
                    && epoch
                        .prepared_compact_slot
                        .as_ref()
                        .is_some_and(|prepared| {
                            prepared.candidate_compaction_id == commit.candidate_compaction_id
                        })
            }))
    }
}

#[async_trait]
impl InhouseCompactionPhysicalPort for AdamCompactionPhysicalPort {
    async fn prepare_compact(
        &self,
        request: &InhouseCompactionPrepareRequest,
    ) -> Result<InhouseCompactionPrepareDisposition> {
        if request.thread_id != request.backend_turn_request.turn_ref.thread_id
            || request.backend_turn_request.turn_ref.inhouse_polar_id != self.inhouse_polar_id
        {
            anyhow::bail!("Adam prepare correlation does not match shared backend turn identity");
        }
        let Some(managed_endpoint_id) = self.managed_endpoint_id.as_deref() else {
            return Ok(InhouseCompactionPrepareDisposition::NotPrepared);
        };
        let projection = project_adam_prepared_compact_request(&request.backend_turn_request);
        if projection.has_tools {
            return Ok(InhouseCompactionPrepareDisposition::NotPrepared);
        }
        let Some(endpoint) = self
            .engine_store
            .endpoints()
            .into_iter()
            .find(|endpoint| endpoint.endpoint_id == managed_endpoint_id)
        else {
            return Ok(InhouseCompactionPrepareDisposition::NotPrepared);
        };
        let live_residencies = self
            .engine_store
            .live_residencies(managed_endpoint_id, &endpoint.runtime_instance_id);
        let Some(residency) = live_residencies
            .into_iter()
            .filter(|residency| {
                residency.inhouse_polar_id == self.inhouse_polar_id
                    && residency.thread_id == request.thread_id
                    && residency.endpoint_id == managed_endpoint_id
            })
            .max_by(|left, right| {
                left.last_activity_unix_ms
                    .cmp(&right.last_activity_unix_ms)
                    .then_with(|| left.projection_id.cmp(&right.projection_id))
                    .then_with(|| left.exact_prefix_hash.cmp(&right.exact_prefix_hash))
                    .then_with(|| left.slot_id.cmp(&right.slot_id))
            })
        else {
            return Ok(InhouseCompactionPrepareDisposition::NotPrepared);
        };
        let wire = self
            .manager
            .project_wire_prompt(
                managed_endpoint_id,
                &LlamaWireProjectionRequest {
                    stable_projection: projection.exact_prefix_payload.clone(),
                    full_projection: projection.full_prompt_payload.clone(),
                    enable_thinking: self.enable_thinking,
                },
            )
            .await?;
        if !self
            .engine_store
            .live_residencies(managed_endpoint_id, &endpoint.runtime_instance_id)
            .into_iter()
            .any(|current| current == residency)
        {
            return Ok(InhouseCompactionPrepareDisposition::NotPrepared);
        }
        let wire_prefix_hash = llamacpp_wire_prompt_hash(&wire.exact_prefix);
        let intent = AdamPreparedCompactIntent {
            managed_endpoint_id: endpoint.endpoint_id.clone(),
            session_id: self.session_id.clone(),
            thread_id: request.thread_id.clone(),
            candidate_compaction_id: request.candidate_compaction_id.clone(),
            source_digest_before: request.source_digest.clone(),
            checkpoint_source_ref: request.retained_tail_checkpoint_source_ref.clone(),
            exact_prefix_hash: wire_prefix_hash.clone(),
            exact_prefix_payload: wire.exact_prefix,
            target_context_compaction_epoch: request.target_context_compaction_epoch,
            model_id: endpoint.model_id.clone(),
            tokenizer_hash: endpoint.tokenizer_hash.clone(),
            context_params_hash: endpoint.context_params_hash.clone(),
        };
        let prepared = match self.manager.prepare_compact(intent).await? {
            AdamPreparedCompactResult::Prepared(prepared) => prepared,
            AdamPreparedCompactResult::NoSpareIdleSlot { .. } => {
                return Ok(InhouseCompactionPrepareDisposition::NotPrepared)
            }
        };
        let now = compaction_now_unix_ms();
        let cache_epoch = AdamCacheEpoch {
            cache_epoch_id: format!("adam-prepared:{}", request.candidate_compaction_id),
            session_id: self.session_id.clone(),
            render_projection_id: None,
            state: AdamCacheEpochState::Prepared,
            connection_mode: "llama".to_string(),
            transport: Some(endpoint.transport.clone()),
            provider_response_id: None,
            llama_slot_id: Some(prepared.slot_id),
            endpoint_fingerprint: Some(format!(
                "{}:{}",
                endpoint.endpoint_id, endpoint.runtime_instance_id
            )),
            model_label: Some(endpoint.model_id.clone()),
            warmed_prefix_hash: wire_prefix_hash.clone(),
            source_digest: request.source_digest.clone(),
            context_compaction_epoch: request.target_context_compaction_epoch,
            renderer_version: projection.hashes.renderer_version,
            model_id: endpoint.model_id,
            template_hash: projection.hashes.template_hash,
            tool_schema_hash: projection.hashes.tool_schema_hash,
            exact_prefix_hash: wire_prefix_hash,
            capability_profile_snapshot: None,
            prepared_compact_slot: Some(AdamPreparedCompactSlotRecord {
                candidate_compaction_id: request.candidate_compaction_id.clone(),
                target_context_epoch: request.target_context_compaction_epoch,
                compact_summary_source_ref: Some(format!(
                    "lgp:compaction:{}",
                    request.candidate_compaction_id
                )),
                retained_tail_checkpoint_source_ref: request
                    .retained_tail_checkpoint_source_ref
                    .clone(),
                prepared_prefix_hash: prepared.exact_prefix_hash.clone(),
                prepared_slot_id: prepared.slot_id,
                promoted_at_unix_ms: None,
                discard_reason: None,
            }),
            invalidation_reason: None,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        if let Err(error) = self.projection_store.record_cache_epoch(cache_epoch).await {
            self.manager.discard_prepared_compact(
                &request.candidate_compaction_id,
                "prepared_projection_evidence_write_failed",
            );
            return Err(error);
        }
        Ok(InhouseCompactionPrepareDisposition::Prepared)
    }

    async fn invalidate_compaction_cache_epoch(
        &self,
        commit: &InhouseCompactionCommit,
    ) -> Result<()> {
        let previous_epoch = self
            .projection_store
            .latest_context_compaction_epoch(&self.session_id)
            .await?;
        if previous_epoch.is_some_and(|epoch| epoch > commit.context_compaction_epoch) {
            anyhow::bail!(
                "Adam projection epoch regressed from {} to {}",
                previous_epoch.unwrap_or_default(),
                commit.context_compaction_epoch
            );
        }
        let mut prepared_target = self.exact_prepared_projection_epoch(commit).await?;
        let already_active =
            previous_epoch == Some(commit.context_compaction_epoch) && prepared_target.is_none();
        if !already_active {
            self.projection_store
                .invalidate_render_projections(&self.session_id, "context_compacted")
                .await?;
            self.projection_store
                .invalidate_cache_epochs(&self.session_id, "context_compacted")
                .await?;
            if let Some(epoch) = prepared_target.as_mut() {
                epoch.state = AdamCacheEpochState::Prepared;
                epoch.invalidation_reason = None;
                self.projection_store
                    .record_cache_epoch(epoch.clone())
                    .await?;
            }
        }
        self.manager.invalidate_committed_compaction_epoch(
            &self.inhouse_polar_id,
            &commit.thread_id,
            &self.session_id,
            commit.context_compaction_epoch,
        );
        Ok(())
    }

    async fn promote_prepared_compact(
        &self,
        commit: &InhouseCompactionCommit,
    ) -> Result<InhouseCompactionPreparedPromotion> {
        let projection_epoch = self.exact_prepared_projection_epoch(commit).await?;
        match self.manager.promote_prepared_compact_commit(commit).await? {
            AdamPreparedCompactPromotion::Promoted(_) => {
                if let Some(epoch) = projection_epoch {
                    self.projection_store
                        .promote_prepared_compact_epoch(&epoch.cache_epoch_id)
                        .await?;
                }
                Ok(InhouseCompactionPreparedPromotion::Promoted)
            }
            AdamPreparedCompactPromotion::Discarded { reason, .. } => {
                if let Some(epoch) = projection_epoch {
                    self.projection_store
                        .discard_prepared_compact_epoch(&epoch.cache_epoch_id, &reason)
                        .await?;
                }
                Ok(InhouseCompactionPreparedPromotion::NotPrepared)
            }
            AdamPreparedCompactPromotion::Missing { .. } => {
                if let Some(epoch) = projection_epoch {
                    self.projection_store
                        .discard_prepared_compact_epoch(
                            &epoch.cache_epoch_id,
                            "physical_prepared_compact_missing",
                        )
                        .await?;
                }
                Ok(InhouseCompactionPreparedPromotion::NotPrepared)
            }
        }
    }

    async fn discard_prepared_compact(&self, abort: &InhouseCompactionAbort) -> Result<()> {
        self.manager
            .discard_prepared_compact(&abort.candidate_compaction_id, &abort.reason);
        if let Some(epoch) = self
            .projection_store
            .latest_prepared_compact_epoch(&self.session_id)
            .await?
            .filter(|epoch| {
                epoch
                    .prepared_compact_slot
                    .as_ref()
                    .is_some_and(|prepared| {
                        prepared.candidate_compaction_id == abort.candidate_compaction_id
                    })
            })
        {
            self.projection_store
                .discard_prepared_compact_epoch(&epoch.cache_epoch_id, &abort.reason)
                .await?;
        }
        Ok(())
    }
}

fn compaction_now_unix_ms() -> i64 {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}


