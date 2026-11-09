//! Physical Adam-owned llama.cpp slot operations.
//!
//! This adapter is deliberately below the shared inhouse semantic boundary:
//! it can probe, warm, save, restore, and erase physical KV state, but it does
//! not decide what a thread or compaction means.

use crate::engine_manager::{
    AdamEngineOperations, AdamLlamaEngineProbe, AdamSnapshotRestoreResult, AdamSnapshotSaveResult,
};
use crate::engine_store::{
    AdamKvSnapshotRecord, AdamManagedLlamaEndpoint, AdamPreparedCompactSlot,
};
use crate::llama::{
    build_llamacpp_apply_template_body, derive_llamacpp_wire_projection,
    parse_llamacpp_props_response, parse_llamacpp_slots_response, LlamaWireProjection,
    LlamaWireProjectionRequest, LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_A,
    LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_B,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct LlamaAdamEngineOperations {
    http: reqwest::Client,
}

impl Default for LlamaAdamEngineOperations {
    fn default() -> Self {
        Self::new(reqwest::Client::new())
    }
}

impl LlamaAdamEngineOperations {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    async fn get_json(&self, url: String) -> anyhow::Result<Value> {
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("llama engine GET {url} failed with {status}: {body}");
        }
        serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("decode llama engine response from {url}: {error}"))
    }

    async fn post_json(&self, url: String, body: Option<Value>) -> anyhow::Result<Value> {
        let request = self.http.post(&url);
        let response = match body {
            Some(body) => request.json(&body).send().await?,
            None => request.send().await?,
        };
        let status = response.status();
        let response_body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("llama engine POST {url} failed with {status}: {response_body}");
        }
        serde_json::from_str(&response_body)
            .map_err(|error| anyhow::anyhow!("decode llama engine response from {url}: {error}"))
    }

    async fn apply_template_prompt(
        &self,
        base_url: &str,
        projection: &str,
        enable_thinking: bool,
    ) -> anyhow::Result<String> {
        let value = self
            .post_json(
                format!("{}/apply-template", base_url.trim_end_matches('/')),
                Some(build_llamacpp_apply_template_body(
                    projection,
                    enable_thinking,
                )),
            )
            .await?;
        value
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|prompt| !prompt.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("llama /apply-template response omitted a non-empty string prompt")
            })
    }

    async fn process_start_identity(&self, base_url: &str) -> anyhow::Result<String> {
        let metrics_url = format!("{}/metrics", base_url.trim_end_matches('/'));
        let response = self.http.get(&metrics_url).send().await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("llama runtime identity probe {metrics_url} failed with {status}");
        }
        let raw = response
            .headers()
            .get("process-start-time-unix")
            .ok_or_else(|| {
                anyhow::anyhow!("llama runtime identity probe omitted Process-Start-Time-Unix")
            })?
            .to_str()
            .map(str::trim)
            .map_err(|error| anyhow::anyhow!("invalid Process-Start-Time-Unix header: {error}"))?;
        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) || raw == "0" {
            anyhow::bail!("invalid Process-Start-Time-Unix header value");
        }
        Ok(raw.to_string())
    }

    /// Probes an action with a negative slot id. llama.cpp wraps positive
    /// out-of-range ids modulo its slot count, so an oversized positive probe
    /// could become destructive if its HTTP parser widens. `-1` remains outside
    /// every concrete slot while still reaching the action route.
    async fn slot_action_supported(&self, base_url: &str, action: &str) -> bool {
        let url = format!(
            "{}/slots/{}?action={action}",
            base_url.trim_end_matches('/'),
            -1
        );
        let request = self.http.post(url);
        let response = match action {
            "save" | "restore" => {
                request
                    .json(&json!({"filename": "polar-adam-capability-probe.invalid"}))
                    .send()
                    .await
            }
            _ => request.send().await,
        };
        let Ok(response) = response else {
            return false;
        };
        let status = response.status();
        if status.is_success() {
            return true;
        }
        if matches!(status.as_u16(), 404 | 405 | 501) {
            return false;
        }
        let body = response
            .text()
            .await
            .unwrap_or_default()
            .to_ascii_lowercase();
        if body.contains("not supported")
            || body.contains("disabled")
            || body.contains("slot-save-path")
            || body.contains("unknown endpoint")
        {
            return false;
        }
        body.contains("invalid slot") || body.contains("id_slot") || body.contains("out of range")
    }

    fn fingerprint(label: &str, bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        format!("sha256:{:x}", hasher.finalize())
    }

    fn require_slot(endpoint: &AdamManagedLlamaEndpoint, slot_id: i64) -> anyhow::Result<()> {
        let range = endpoint
            .slot_range()
            .ok_or_else(|| anyhow::anyhow!("invalid Adam-managed slot range"))?;
        if !range.contains(&slot_id) {
            anyhow::bail!(
                "slot {slot_id} is outside Adam-managed slot range {}..{}",
                range.start,
                range.end
            );
        }
        Ok(())
    }

    fn snapshot_filename(snapshot: &AdamKvSnapshotRecord) -> anyhow::Result<&str> {
        let filename = Path::new(&snapshot.blob_ref)
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty() && *value != "." && *value != "..")
            .ok_or_else(|| anyhow::anyhow!("snapshot blob_ref has no safe filename"))?;
        Ok(filename)
    }

    fn slot_action_url(endpoint: &AdamManagedLlamaEndpoint, slot_id: i64, action: &str) -> String {
        let query = match action {
            "save" => "action=save",
            "restore" => "action=restore",
            "erase" => "action=erase",
            other => return format!("invalid-slot-action:{other}"),
        };
        format!(
            "{}/slots/{slot_id}?{query}",
            endpoint.connector_base_url.trim_end_matches('/')
        )
    }

    fn response_slot(value: &Value) -> Option<i64> {
        value
            .get("id_slot")
            .or_else(|| value.get("slot_id"))
            .and_then(Value::as_i64)
    }

    fn max_context_tokens(value: &Value) -> Option<i64> {
        [
            "/default_generation_settings/n_ctx",
            "/default_generation_settings/n_ctx_train",
            "/model_meta/n_ctx_train",
            "/server/n_ctx",
            "/n_ctx",
        ]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_i64))
        .filter(|tokens| *tokens > 0)
    }

    fn require_response_slot(value: &Value, expected: i64) -> anyhow::Result<()> {
        let actual = Self::response_slot(value)
            .ok_or_else(|| anyhow::anyhow!("llama engine response omitted id_slot"))?;
        if actual != expected {
            anyhow::bail!("llama engine response slot mismatch: expected {expected}, got {actual}");
        }
        Ok(())
    }
}

#[async_trait]
impl AdamEngineOperations for LlamaAdamEngineOperations {
    async fn project_wire_prompt(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        request: &LlamaWireProjectionRequest,
    ) -> anyhow::Result<LlamaWireProjection> {
        let base = endpoint.connector_base_url.trim_end_matches('/');
        let rendered_stable_a = self
            .apply_template_prompt(
                base,
                &format!(
                    "{}{}",
                    request.stable_projection, LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_A
                ),
                request.enable_thinking,
            )
            .await?;
        let rendered_stable_b = self
            .apply_template_prompt(
                base,
                &format!(
                    "{}{}",
                    request.stable_projection, LLAMACPP_WIRE_PREFIX_PROBE_SUFFIX_B
                ),
                request.enable_thinking,
            )
            .await?;
        let rendered_full = self
            .apply_template_prompt(base, &request.full_projection, request.enable_thinking)
            .await?;
        derive_llamacpp_wire_projection(
            request,
            &rendered_stable_a,
            &rendered_stable_b,
            &rendered_full,
        )
        .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    async fn probe_endpoint(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
    ) -> anyhow::Result<AdamLlamaEngineProbe> {
        let base = endpoint.connector_base_url.trim_end_matches('/');
        let process_start_before = self.process_start_identity(base).await?;
        let slots_value = self.get_json(format!("{base}/slots")).await?;
        let props_value = self.get_json(format!("{base}/props")).await?;
        let slots = parse_llamacpp_slots_response(&slots_value)
            .ok_or_else(|| anyhow::anyhow!("llama /slots response did not expose slot state"))?;
        let props = parse_llamacpp_props_response(&props_value)
            .ok_or_else(|| anyhow::anyhow!("llama /props response did not expose slot count"))?;
        let managed_range = endpoint
            .slot_range()
            .ok_or_else(|| anyhow::anyhow!("invalid Adam-managed slot range"))?;
        let physical_slot_count = i64::from(slots.total_slots);
        if slots.total_slots != props.total_slots || managed_range.end > physical_slot_count {
            anyhow::bail!(
                "llama slot probe mismatch: slots={} props={} managed={}..{}",
                slots.total_slots,
                props.total_slots,
                managed_range.start,
                managed_range.end
            );
        }
        let reported_slot_ids = slots
            .idle_slots
            .iter()
            .chain(&slots.busy_slots)
            .copied()
            .collect::<std::collections::HashSet<_>>();
        if managed_range
            .clone()
            .any(|slot_id| !reported_slot_ids.contains(&slot_id))
        {
            anyhow::bail!(
                "llama slot probe did not report every managed slot in {}..{}",
                managed_range.start,
                managed_range.end
            );
        }
        let serialized = serde_json::to_vec(&props_value)?;
        let supports_snapshot_save = self.slot_action_supported(base, "save").await;
        let supports_snapshot_restore = self.slot_action_supported(base, "restore").await;
        let supports_slot_erase = self.slot_action_supported(base, "erase").await;
        let process_start_after = self.process_start_identity(base).await?;
        if process_start_before != process_start_after {
            anyhow::bail!(
                "llama process changed during endpoint probe; refusing mixed runtime facts"
            );
        }
        let runtime_instance_id = Self::fingerprint(
            "llama-runtime-instance-v1",
            format!(
                "{}\0{}",
                endpoint.connector_base_url.trim_end_matches('/'),
                process_start_after
            )
            .as_bytes(),
        );
        let llama_build_fingerprint = props_value
            .get("build_info")
            .or_else(|| props_value.get("build"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| runtime_instance_id.clone());
        let tokenizer_identity = props_value
            .get("model_path")
            .or_else(|| props_value.get("model"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(AdamLlamaEngineProbe {
            runtime_instance_id,
            llama_build_fingerprint: llama_build_fingerprint.clone(),
            tokenizer_hash: Self::fingerprint(
                "llama-tokenizer-v1",
                format!("{tokenizer_identity}\0{llama_build_fingerprint}").as_bytes(),
            ),
            context_params_hash: Self::fingerprint("llama-context-params-v1", &serialized),
            max_context_tokens: Self::max_context_tokens(&props_value).ok_or_else(|| {
                anyhow::anyhow!("llama /props response omitted verified max_context_tokens")
            })?,
            total_slots: slots.total_slots,
            idle_slot_ids: slots.idle_slots,
            busy_slot_ids: slots.busy_slots,
            supports_snapshot_save,
            supports_snapshot_restore,
            supports_slot_erase,
        })
    }

    async fn count_wire_prompt_tokens(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        prompt: &str,
    ) -> anyhow::Result<i64> {
        let value = self
            .post_json(
                format!(
                    "{}/tokenize",
                    endpoint.connector_base_url.trim_end_matches('/')
                ),
                Some(json!({"content": prompt, "add_special": false, "with_pieces": false})),
            )
            .await?;
        let count = value
            .get("tokens")
            .and_then(Value::as_array)
            .map(|tokens| tokens.len())
            .or_else(|| {
                value
                    .get("count")
                    .and_then(Value::as_u64)
                    .and_then(|count| usize::try_from(count).ok())
            })
            .ok_or_else(|| anyhow::anyhow!("llama /tokenize response omitted exact tokens"))?;
        i64::try_from(count).map_err(|_| anyhow::anyhow!("llama /tokenize count exceeds i64"))
    }

    async fn load_snapshot(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
        snapshot: &AdamKvSnapshotRecord,
    ) -> anyhow::Result<AdamSnapshotRestoreResult> {
        Self::require_slot(endpoint, slot_id)?;
        let filename = Self::snapshot_filename(snapshot)?;
        let value = self
            .post_json(
                Self::slot_action_url(endpoint, slot_id, "restore"),
                Some(json!({"filename": filename})),
            )
            .await?;
        Self::require_response_slot(&value, slot_id)?;
        let n_restored = value
            .get("n_restored")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("llama snapshot restore omitted n_restored"))?;
        let n_read = value
            .get("n_read")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("llama snapshot restore omitted n_read"))?;
        Ok(AdamSnapshotRestoreResult { n_restored, n_read })
    }

    async fn warm_prepared_compact(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
        intent: &crate::engine_manager::AdamPreparedCompactIntent,
    ) -> anyhow::Result<()> {
        Self::require_slot(endpoint, slot_id)?;
        let url = format!(
            "{}/completion",
            endpoint.connector_base_url.trim_end_matches('/')
        );
        let value = self
            .post_json(
                url,
                Some(json!({
                    "prompt": intent.exact_prefix_payload,
                    "id_slot": slot_id,
                    "cache_prompt": true,
                    "n_predict": 0,
                    "stream": false
                })),
            )
            .await?;
        Self::require_response_slot(&value, slot_id)
    }

    async fn promote_prepared_compact(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        prepared: &AdamPreparedCompactSlot,
    ) -> anyhow::Result<()> {
        Self::require_slot(endpoint, prepared.slot_id)?;
        let probe = self.probe_endpoint(endpoint).await?;
        if !probe.idle_slot_ids.contains(&prepared.slot_id) {
            anyhow::bail!(
                "prepared slot {} is not idle at promotion",
                prepared.slot_id
            );
        }
        Ok(())
    }

    async fn save_snapshot(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
        snapshot: &AdamKvSnapshotRecord,
    ) -> anyhow::Result<AdamSnapshotSaveResult> {
        Self::require_slot(endpoint, slot_id)?;
        let filename = Self::snapshot_filename(snapshot)?;
        let value = self
            .post_json(
                Self::slot_action_url(endpoint, slot_id, "save"),
                Some(json!({"filename": filename})),
            )
            .await?;
        Self::require_response_slot(&value, slot_id)?;
        let n_saved = value
            .get("n_saved")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("llama snapshot save omitted n_saved"))?;
        let n_written = value
            .get("n_written")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if n_saved < 0 || n_written <= 0 {
            anyhow::bail!("llama snapshot save produced no durable bytes");
        }
        Ok(AdamSnapshotSaveResult { n_saved, n_written })
    }

    async fn erase_slot(
        &self,
        endpoint: &AdamManagedLlamaEndpoint,
        slot_id: i64,
    ) -> anyhow::Result<i64> {
        Self::require_slot(endpoint, slot_id)?;
        let value = self
            .post_json(Self::slot_action_url(endpoint, slot_id, "erase"), None)
            .await?;
        Self::require_response_slot(&value, slot_id)?;
        value
            .get("n_erased")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("llama slot erase omitted n_erased"))
    }
}
