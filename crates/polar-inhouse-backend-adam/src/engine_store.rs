//! Adam-exclusive engine metadata and evidence storage.
//!
//! Live KV remains owned by the llama process. Records in this module are
//! disposable observations and compatibility metadata; they are never the
//! canonical Polar/LGP transcript or compaction record.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamManagedLlamaEndpoint {
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    /// Normalized connector URL this control record is allowed to govern.
    #[serde(default)]
    pub connector_base_url: String,
    /// Provider transport governed by this record. Engine slot control is
    /// currently valid only for `llamacpp_slots`.
    #[serde(default)]
    pub transport: String,
    /// First provider slot ID governed by this endpoint record.
    #[serde(default)]
    pub slot_offset: i64,
    pub slot_count: i64,
    pub model_id: String,
    pub tokenizer_hash: String,
    /// Hash of the context-shaping provider parameters. Engine control is
    /// compatible only when this nonblank fingerprint exactly matches the
    /// turn or prepared-prefix intent.
    #[serde(default)]
    pub context_params_hash: String,
    /// Adam engine-control guarantees are valid only on isolated endpoints.
    #[serde(default)]
    pub adam_managed_only: bool,
}

impl AdamManagedLlamaEndpoint {
    pub fn allows_backend(&self, backend: &str) -> bool {
        !self.adam_managed_only || backend == "adam"
    }

    pub fn slot_range(&self) -> Option<Range<i64>> {
        checked_slot_range(self.slot_offset, self.slot_count)
    }
}

fn checked_slot_range(slot_offset: i64, slot_count: i64) -> Option<Range<i64>> {
    if slot_offset < 0 || slot_count <= 0 {
        return None;
    }
    let slot_end = slot_offset.checked_add(slot_count)?;
    Some(slot_offset..slot_end)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamLiveSlotResidencyRecord {
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub projection_id: String,
    pub endpoint_id: String,
    pub slot_id: i64,
    pub exact_prefix_hash: String,
    pub runtime_instance_id: String,
    pub prefix_token_count: i64,
    #[serde(default)]
    pub context_compaction_epoch: i64,
    /// Durable scheduler activity used for restart-safe eviction decisions.
    #[serde(default)]
    pub last_activity_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamKvSnapshotRecord {
    pub snapshot_id: String,
    pub blob_ref: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub tokenizer_hash: String,
    #[serde(default)]
    pub context_params_hash: String,
    pub exact_prefix_hash: String,
    #[serde(default)]
    pub context_compaction_epoch: i64,
    pub prefix_token_count: i64,
    /// Legacy external byte-digest field. It stays blank unless an adapter can
    /// actually hash the saved file bytes; llama HTTP counters alone are not a
    /// checksum.
    #[serde(default)]
    pub snapshot_checksum: String,
    /// Physical byte receipt returned by llama's save operation. Restore is
    /// accepted only when `n_read` exactly matches this value.
    #[serde(default)]
    pub snapshot_bytes_written: i64,
    /// Stable format discriminator for future compatibility migrations.
    #[serde(default)]
    pub snapshot_format_version: String,
    /// Fingerprint of the llama build/runtime that produced the snapshot.
    #[serde(default)]
    pub llama_build_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamSnapshotCompatibility<'a> {
    pub model_id: &'a str,
    pub tokenizer_hash: &'a str,
    pub context_params_hash: &'a str,
    pub exact_prefix_hash: &'a str,
    pub context_compaction_epoch: i64,
    pub llama_build_fingerprint: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamPromotedPreparedLookup<'a> {
    pub endpoint_id: &'a str,
    pub runtime_instance_id: &'a str,
    pub model_id: &'a str,
    pub tokenizer_hash: &'a str,
    pub context_params_hash: &'a str,
    pub exact_prefix_hash: &'a str,
    pub context_compaction_epoch: i64,
}

impl AdamKvSnapshotRecord {
    pub fn is_compatible_with(&self, required: &AdamSnapshotCompatibility<'_>) -> bool {
        self.model_id == required.model_id
            && self.tokenizer_hash == required.tokenizer_hash
            && self.context_params_hash == required.context_params_hash
            && self.exact_prefix_hash == required.exact_prefix_hash
            && self.context_compaction_epoch == required.context_compaction_epoch
            && self.prefix_token_count > 0
            && self.snapshot_bytes_written > 0
            && !self.snapshot_format_version.trim().is_empty()
            && self.llama_build_fingerprint == required.llama_build_fingerprint
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamEngineCompletionEvidence {
    pub evidence_id: String,
    pub endpoint_id: String,
    pub slot_id: i64,
    pub runtime_instance_id: String,
    pub cache_plan: String,
    #[serde(default)]
    pub allocation_reason: String,
    pub exact_prefix_hash: String,
    pub cache_hit_tokens: Option<i64>,
    pub cache_miss_tokens: Option<i64>,
    pub confirmed_provider_completion: bool,
    pub created_at_unix_ms: i64,
}

/// One fenced provider-completion transaction.
///
/// The store must compare the lease token/runtime/expiry and publish the
/// evidence plus residency atomically. Callers must not split this operation
/// into a validation followed by independent writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamEngineCompletionCommit {
    pub lease_id: String,
    pub fencing_token: String,
    pub runtime_instance_id: String,
    pub now_unix_ms: i64,
    pub evidence: AdamEngineCompletionEvidence,
    pub residency: AdamLiveSlotResidencyRecord,
}

/// Physical prepared-slot evidence. Semantic compaction content remains in LGP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamPreparedCompactSlot {
    pub session_id: String,
    pub thread_id: String,
    pub candidate_compaction_id: String,
    pub source_digest_before: String,
    pub checkpoint_source_ref: String,
    pub exact_prefix_hash: String,
    pub target_context_compaction_epoch: i64,
    pub model_id: String,
    pub tokenizer_hash: String,
    pub context_params_hash: String,
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub slot_id: i64,
    pub promoted: bool,
    #[serde(default)]
    pub committed_compaction_id: Option<String>,
    pub discard_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamTurnSlotReservation {
    pub lease_id: String,
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub slot_id: i64,
    pub used_preferred_slot: bool,
    pub reserved_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamTurnSlotReservationRequest<'a> {
    pub lease_id: &'a str,
    pub endpoint_id: &'a str,
    pub runtime_instance_id: &'a str,
    pub slot_offset: i64,
    pub slot_count: i64,
    pub preferred_slot_id: Option<i64>,
    pub allowed_slot_ids: &'a [i64],
    pub reserved_at_unix_ms: i64,
    pub ttl_ms: i64,
}

/// Durable lease record used for compare-and-swap fencing.
///
/// The slot generation is monotonic for one endpoint/runtime/slot identity.
/// A provider result is admissible only while this exact lease and fencing
/// token remain current and unexpired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamPersistentSlotLease {
    pub lease_id: String,
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub slot_id: i64,
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub projection_id: String,
    pub lease_generation: i64,
    pub fencing_token: String,
    pub acquired_at_unix_ms: i64,
    pub heartbeat_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub released_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamSlotLeaseCasRequest<'a> {
    pub lease_id: &'a str,
    pub endpoint_id: &'a str,
    pub runtime_instance_id: &'a str,
    pub slot_id: i64,
    pub inhouse_polar_id: &'a str,
    pub thread_id: &'a str,
    pub projection_id: &'a str,
    pub acquired_at_unix_ms: i64,
    pub ttl_ms: i64,
}

pub trait AdamEngineStore: Send + Sync {
    fn endpoints(&self) -> Vec<AdamManagedLlamaEndpoint>;

    /// Publishes a physically probed endpoint identity. Runtime/profile
    /// adapters may register an unprobed URL/model/slot resource first, but
    /// only the engine manager may replace it with observed runtime facts.
    fn upsert_managed_endpoint(&self, endpoint: AdamManagedLlamaEndpoint);

    /// Removes a host-revoked endpoint and all endpoint-scoped runtime state.
    /// Host declarations are authoritative across process restarts.
    fn remove_managed_endpoint(&self, endpoint_id: &str) -> bool;

    fn live_residency(
        &self,
        inhouse_polar_id: &str,
        thread_id: &str,
        projection_id: &str,
        exact_prefix_hash: &str,
    ) -> Option<AdamLiveSlotResidencyRecord>;

    fn active_slot_ids(&self, endpoint_id: &str, runtime_instance_id: &str) -> Vec<i64>;

    /// Atomically reserves a slot for a foreground turn. A matching live slot
    /// is preferred, then an unused slot, then a completed live residency may
    /// be evicted. Prepared and in-flight slots are never evicted.
    fn reserve_turn_slot(
        &self,
        request: &AdamTurnSlotReservationRequest<'_>,
    ) -> Option<AdamTurnSlotReservation>;

    /// Atomically reserves only a genuinely idle slot for background prefix
    /// preparation. It never evicts a completed residency.
    fn reserve_prepared_slot(
        &self,
        reservation_id: &str,
        endpoint_id: &str,
        runtime_instance_id: &str,
        slot_offset: i64,
        slot_count: i64,
    ) -> Option<i64>;

    fn release_slot_reservation(&self, reservation_id: &str) -> bool;

    fn slot_reservations(&self) -> Vec<AdamTurnSlotReservation>;

    /// Removes expired reservations and, during runtime open, reservations
    /// whose matching live lease no longer exists.
    fn reconcile_slot_reservations(&self, now_unix_ms: i64, remove_unleased: bool) -> usize;

    /// Atomically acquires the selected slot and advances its durable fencing
    /// generation. An unexpired lease held by another request wins the CAS.
    fn compare_and_swap_slot_lease(
        &self,
        request: &AdamSlotLeaseCasRequest<'_>,
    ) -> Option<AdamPersistentSlotLease>;

    fn heartbeat_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        now_unix_ms: i64,
        ttl_ms: i64,
    ) -> bool;

    fn validate_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        runtime_instance_id: &str,
        now_unix_ms: i64,
    ) -> bool;

    fn release_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        released_at_unix_ms: i64,
    ) -> bool;

    fn slot_leases(&self) -> Vec<AdamPersistentSlotLease>;

    fn compatible_snapshot(
        &self,
        compatibility: &AdamSnapshotCompatibility<'_>,
    ) -> Option<AdamKvSnapshotRecord>;

    fn upsert_snapshot(&self, snapshot: AdamKvSnapshotRecord);

    /// Quarantines an unverifiable physical snapshot by removing its eligible
    /// metadata record. The provider-owned file may remain, but Adam will not
    /// attempt to restore it again.
    fn remove_snapshot(&self, snapshot_id: &str) -> bool;

    fn snapshots(&self) -> Vec<AdamKvSnapshotRecord>;

    fn live_residencies(
        &self,
        endpoint_id: &str,
        runtime_instance_id: &str,
    ) -> Vec<AdamLiveSlotResidencyRecord>;

    fn engine_evidence(&self) -> Vec<AdamEngineCompletionEvidence>;

    /// Atomically commits completion evidence and the corresponding live
    /// residency only while the exact fencing generation remains current.
    fn commit_engine_completion_if_current(&self, commit: AdamEngineCompletionCommit) -> bool;

    fn record_engine_evidence(&self, evidence: AdamEngineCompletionEvidence);

    fn upsert_live_residency(&self, residency: AdamLiveSlotResidencyRecord);

    fn remove_live_residency(
        &self,
        endpoint_id: &str,
        runtime_instance_id: &str,
        slot_id: i64,
    ) -> bool;

    fn invalidate_live_residencies_for_runtime(
        &self,
        endpoint_id: &str,
        current_runtime_instance_id: &str,
    ) -> usize;

    fn invalidate_live_residencies_for_context_epoch(
        &self,
        inhouse_polar_id: &str,
        thread_id: &str,
        current_context_compaction_epoch: i64,
    ) -> usize;

    fn upsert_prepared_compact_slot(&self, prepared: AdamPreparedCompactSlot);

    fn prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
    ) -> Option<AdamPreparedCompactSlot>;

    fn prepared_compact_slot_for_session(
        &self,
        session_id: &str,
    ) -> Option<AdamPreparedCompactSlot>;

    fn prepared_compact_slots_for_session(&self, session_id: &str) -> Vec<AdamPreparedCompactSlot>;

    fn prepared_slot_ids(&self, endpoint_id: &str, runtime_instance_id: &str) -> Vec<i64>;

    fn promoted_prepared_compact_slot(
        &self,
        lookup: &AdamPromotedPreparedLookup<'_>,
    ) -> Option<AdamPreparedCompactSlot>;

    fn promote_prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
        committed_compaction_id: &str,
    ) -> Option<AdamPreparedCompactSlot>;

    fn discard_prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
        reason: &str,
    ) -> Option<AdamPreparedCompactSlot>;
}

#[derive(Default)]
pub struct InMemoryAdamEngineStore {
    endpoints: Mutex<HashMap<String, AdamManagedLlamaEndpoint>>,
    live_residencies: Mutex<Vec<AdamLiveSlotResidencyRecord>>,
    snapshots: Mutex<Vec<AdamKvSnapshotRecord>>,
    engine_evidence: Mutex<Vec<AdamEngineCompletionEvidence>>,
    prepared_compact_slots: Mutex<HashMap<String, AdamPreparedCompactSlot>>,
    slot_reservations: Mutex<HashMap<String, AdamTurnSlotReservation>>,
    slot_leases: Mutex<HashMap<String, AdamPersistentSlotLease>>,
    slot_lease_generations: Mutex<HashMap<(String, String, i64), i64>>,
}

impl InMemoryAdamEngineStore {
    pub fn upsert_endpoint(&self, endpoint: AdamManagedLlamaEndpoint) {
        let endpoint_id = endpoint.endpoint_id.clone();
        let runtime_instance_id = endpoint.runtime_instance_id.clone();
        self.endpoints
            .lock()
            .expect("Adam engine endpoint store lock")
            .insert(endpoint_id.clone(), endpoint);
        self.invalidate_live_residencies_for_runtime(&endpoint_id, &runtime_instance_id);
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .retain(|_, prepared| {
                prepared.endpoint_id != endpoint_id
                    || prepared.runtime_instance_id == runtime_instance_id
            });
        self.slot_leases
            .lock()
            .expect("Adam persistent slot lease lock")
            .retain(|_, lease| {
                lease.endpoint_id != endpoint_id || lease.runtime_instance_id == runtime_instance_id
            });
        self.slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .retain(|_, reservation| {
                reservation.endpoint_id != endpoint_id
                    || reservation.runtime_instance_id == runtime_instance_id
            });
    }

    pub fn put_live_residency(&self, residency: AdamLiveSlotResidencyRecord) {
        self.upsert_live_residency(residency);
    }

    pub fn put_snapshot(&self, snapshot: AdamKvSnapshotRecord) {
        self.upsert_snapshot(snapshot);
    }

    pub fn engine_evidence_snapshot(&self) -> Vec<AdamEngineCompletionEvidence> {
        self.engine_evidence
            .lock()
            .expect("Adam engine evidence store lock")
            .clone()
    }

    pub fn live_residencies_snapshot(&self) -> Vec<AdamLiveSlotResidencyRecord> {
        self.live_residencies
            .lock()
            .expect("Adam live residency store lock")
            .clone()
    }

    pub fn prepared_compact_slots_snapshot(&self) -> Vec<AdamPreparedCompactSlot> {
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .values()
            .cloned()
            .collect()
    }

    pub fn slot_reservations_snapshot(&self) -> Vec<AdamTurnSlotReservation> {
        self.slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .values()
            .cloned()
            .collect()
    }
}

impl AdamEngineStore for InMemoryAdamEngineStore {
    fn endpoints(&self) -> Vec<AdamManagedLlamaEndpoint> {
        self.endpoints
            .lock()
            .expect("Adam engine endpoint store lock")
            .values()
            .cloned()
            .collect()
    }

    fn upsert_managed_endpoint(&self, endpoint: AdamManagedLlamaEndpoint) {
        self.upsert_endpoint(endpoint);
    }

    fn remove_managed_endpoint(&self, endpoint_id: &str) -> bool {
        let removed = self
            .endpoints
            .lock()
            .expect("Adam engine endpoint store lock")
            .remove(endpoint_id)
            .is_some();
        self.live_residencies
            .lock()
            .expect("Adam live residency store lock")
            .retain(|record| record.endpoint_id != endpoint_id);
        self.engine_evidence
            .lock()
            .expect("Adam engine evidence store lock")
            .retain(|record| record.endpoint_id != endpoint_id);
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .retain(|_, record| record.endpoint_id != endpoint_id);
        self.slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .retain(|_, record| record.endpoint_id != endpoint_id);
        self.slot_leases
            .lock()
            .expect("Adam persistent slot lease lock")
            .retain(|_, record| record.endpoint_id != endpoint_id);
        self.slot_lease_generations
            .lock()
            .expect("Adam slot lease generation lock")
            .retain(|(record_endpoint_id, _, _), _| record_endpoint_id != endpoint_id);
        removed
    }

    fn live_residency(
        &self,
        inhouse_polar_id: &str,
        thread_id: &str,
        projection_id: &str,
        exact_prefix_hash: &str,
    ) -> Option<AdamLiveSlotResidencyRecord> {
        self.live_residencies
            .lock()
            .expect("Adam live residency store lock")
            .iter()
            .find(|record| {
                record.inhouse_polar_id == inhouse_polar_id
                    && record.thread_id == thread_id
                    && record.projection_id == projection_id
                    && record.exact_prefix_hash == exact_prefix_hash
            })
            .cloned()
    }

    fn active_slot_ids(&self, endpoint_id: &str, runtime_instance_id: &str) -> Vec<i64> {
        let mut slots = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock")
            .iter()
            .filter(|record| {
                record.endpoint_id == endpoint_id
                    && record.runtime_instance_id == runtime_instance_id
            })
            .map(|record| record.slot_id)
            .collect::<Vec<_>>();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    fn reserve_turn_slot(
        &self,
        request: &AdamTurnSlotReservationRequest<'_>,
    ) -> Option<AdamTurnSlotReservation> {
        if request.reserved_at_unix_ms < 0 || request.ttl_ms <= 0 {
            return None;
        }
        let expires_at_unix_ms = request.reserved_at_unix_ms.checked_add(request.ttl_ms)?;
        self.reconcile_slot_reservations(request.reserved_at_unix_ms, false);
        let mut slot_range = checked_slot_range(request.slot_offset, request.slot_count)?;
        let mut reservations = self
            .slot_reservations
            .lock()
            .expect("Adam slot reservation store lock");
        let mut prepared = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock");
        let mut live = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");

        let in_flight = reservations
            .values()
            .filter(|reservation| {
                reservation.endpoint_id == request.endpoint_id
                    && reservation.runtime_instance_id == request.runtime_instance_id
            })
            .map(|reservation| reservation.slot_id)
            .collect::<Vec<_>>();
        let protected_prepared = prepared
            .values()
            .filter(|record| {
                record.endpoint_id == request.endpoint_id
                    && record.runtime_instance_id == request.runtime_instance_id
                    && record.discard_reason.is_none()
                    && Some(record.slot_id) != request.preferred_slot_id
            })
            .map(|record| record.slot_id)
            .collect::<Vec<_>>();
        let unavailable = in_flight
            .into_iter()
            .chain(protected_prepared)
            .collect::<std::collections::BTreeSet<_>>();

        let valid_preferred = request.preferred_slot_id.filter(|slot_id| {
            slot_range.contains(slot_id)
                && !unavailable.contains(slot_id)
                && (request.allowed_slot_ids.is_empty()
                    || request.allowed_slot_ids.contains(slot_id))
        });
        let live_slot_ids = live
            .iter()
            .filter(|record| {
                record.endpoint_id == request.endpoint_id
                    && record.runtime_instance_id == request.runtime_instance_id
            })
            .map(|record| record.slot_id)
            .collect::<std::collections::BTreeSet<_>>();
        let slot_id = valid_preferred
            .or_else(|| {
                slot_range.clone().find(|slot_id| {
                    !unavailable.contains(slot_id)
                        && !live_slot_ids.contains(slot_id)
                        && (request.allowed_slot_ids.is_empty()
                            || request.allowed_slot_ids.contains(slot_id))
                })
            })
            .or_else(|| {
                slot_range.find(|slot_id| {
                    !unavailable.contains(slot_id)
                        && (request.allowed_slot_ids.is_empty()
                            || request.allowed_slot_ids.contains(slot_id))
                })
            })?;
        let used_preferred_slot = valid_preferred == Some(slot_id);

        if !used_preferred_slot {
            // The selected residency is completed (not in-flight) and may be
            // replaced by foreground work. This prevents stale live metadata
            // from permanently exhausting the endpoint.
            live.retain(|record| {
                !(record.endpoint_id == request.endpoint_id
                    && record.runtime_instance_id == request.runtime_instance_id
                    && record.slot_id == slot_id)
            });
            for record in prepared.values_mut().filter(|record| {
                record.endpoint_id == request.endpoint_id
                    && record.runtime_instance_id == request.runtime_instance_id
                    && record.slot_id == slot_id
                    && record.promoted
                    && record.discard_reason.is_none()
            }) {
                record.promoted = false;
                record.discard_reason = Some("slot_reused_by_foreground_turn".to_string());
            }
        }
        let reservation = AdamTurnSlotReservation {
            lease_id: request.lease_id.to_string(),
            endpoint_id: request.endpoint_id.to_string(),
            runtime_instance_id: request.runtime_instance_id.to_string(),
            slot_id,
            used_preferred_slot,
            reserved_at_unix_ms: request.reserved_at_unix_ms,
            expires_at_unix_ms,
        };
        reservations.insert(request.lease_id.to_string(), reservation.clone());
        Some(reservation)
    }

    fn reserve_prepared_slot(
        &self,
        reservation_id: &str,
        endpoint_id: &str,
        runtime_instance_id: &str,
        slot_offset: i64,
        slot_count: i64,
    ) -> Option<i64> {
        let reserved_at_unix_ms = crate::now_unix_ms();
        let expires_at_unix_ms = reserved_at_unix_ms.checked_add(30_000)?;
        self.reconcile_slot_reservations(reserved_at_unix_ms, false);
        let slot_range = checked_slot_range(slot_offset, slot_count)?;
        let mut reservations = self
            .slot_reservations
            .lock()
            .expect("Adam slot reservation store lock");
        let prepared = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock");
        let live = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        let unavailable = reservations
            .values()
            .filter(|reservation| {
                reservation.endpoint_id == endpoint_id
                    && reservation.runtime_instance_id == runtime_instance_id
            })
            .map(|reservation| reservation.slot_id)
            .chain(prepared.values().filter_map(|record| {
                (record.endpoint_id == endpoint_id
                    && record.runtime_instance_id == runtime_instance_id
                    && record.discard_reason.is_none())
                .then_some(record.slot_id)
            }))
            .chain(live.iter().filter_map(|record| {
                (record.endpoint_id == endpoint_id
                    && record.runtime_instance_id == runtime_instance_id)
                    .then_some(record.slot_id)
            }))
            .collect::<std::collections::BTreeSet<_>>();
        let slot_id = slot_range
            .rev()
            .find(|slot_id| !unavailable.contains(slot_id))?;
        reservations.insert(
            reservation_id.to_string(),
            AdamTurnSlotReservation {
                lease_id: reservation_id.to_string(),
                endpoint_id: endpoint_id.to_string(),
                runtime_instance_id: runtime_instance_id.to_string(),
                slot_id,
                used_preferred_slot: false,
                reserved_at_unix_ms,
                expires_at_unix_ms,
            },
        );
        Some(slot_id)
    }

    fn release_slot_reservation(&self, reservation_id: &str) -> bool {
        self.slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .remove(reservation_id)
            .is_some()
    }

    fn slot_reservations(&self) -> Vec<AdamTurnSlotReservation> {
        self.slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .values()
            .cloned()
            .collect()
    }

    fn reconcile_slot_reservations(&self, now_unix_ms: i64, remove_unleased: bool) -> usize {
        let leases = self
            .slot_leases
            .lock()
            .expect("Adam persistent slot lease lock");
        let mut reservations = self
            .slot_reservations
            .lock()
            .expect("Adam slot reservation store lock");
        let before = reservations.len();
        reservations.retain(|_, reservation| {
            if reservation.expires_at_unix_ms <= now_unix_ms {
                return false;
            }
            if !remove_unleased {
                return true;
            }
            leases.values().any(|lease| {
                lease.lease_id == reservation.lease_id
                    && lease.endpoint_id == reservation.endpoint_id
                    && lease.runtime_instance_id == reservation.runtime_instance_id
                    && lease.slot_id == reservation.slot_id
                    && lease.released_at_unix_ms.is_none()
                    && lease.expires_at_unix_ms > now_unix_ms
            })
        });
        before.saturating_sub(reservations.len())
    }

    fn compare_and_swap_slot_lease(
        &self,
        request: &AdamSlotLeaseCasRequest<'_>,
    ) -> Option<AdamPersistentSlotLease> {
        if request.ttl_ms <= 0 || request.acquired_at_unix_ms < 0 {
            return None;
        }
        let expires_at_unix_ms = request.acquired_at_unix_ms.checked_add(request.ttl_ms)?;
        let mut leases = self
            .slot_leases
            .lock()
            .expect("Adam persistent slot lease lock");
        if leases.values().any(|lease| {
            lease.endpoint_id == request.endpoint_id
                && lease.runtime_instance_id == request.runtime_instance_id
                && lease.slot_id == request.slot_id
                && lease.released_at_unix_ms.is_none()
                && lease.expires_at_unix_ms > request.acquired_at_unix_ms
                && lease.lease_id != request.lease_id
        }) {
            return None;
        }
        let key = (
            request.endpoint_id.to_string(),
            request.runtime_instance_id.to_string(),
            request.slot_id,
        );
        let lease_generation = {
            let mut generations = self
                .slot_lease_generations
                .lock()
                .expect("Adam slot lease generation lock");
            let generation = generations.entry(key).or_insert(0);
            *generation = generation.checked_add(1)?;
            *generation
        };
        let fencing_token = format!(
            "adam-fence:{}:{}:{}:{}",
            request.endpoint_id, request.runtime_instance_id, request.slot_id, lease_generation
        );
        let lease = AdamPersistentSlotLease {
            lease_id: request.lease_id.to_string(),
            endpoint_id: request.endpoint_id.to_string(),
            runtime_instance_id: request.runtime_instance_id.to_string(),
            slot_id: request.slot_id,
            inhouse_polar_id: request.inhouse_polar_id.to_string(),
            thread_id: request.thread_id.to_string(),
            projection_id: request.projection_id.to_string(),
            lease_generation,
            fencing_token,
            acquired_at_unix_ms: request.acquired_at_unix_ms,
            heartbeat_at_unix_ms: request.acquired_at_unix_ms,
            expires_at_unix_ms,
            released_at_unix_ms: None,
        };
        leases.insert(lease.lease_id.clone(), lease.clone());
        Some(lease)
    }

    fn heartbeat_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        now_unix_ms: i64,
        ttl_ms: i64,
    ) -> bool {
        let Some(expires_at_unix_ms) = now_unix_ms.checked_add(ttl_ms) else {
            return false;
        };
        let mut leases = self
            .slot_leases
            .lock()
            .expect("Adam persistent slot lease lock");
        let Some(lease) = leases.get_mut(lease_id) else {
            return false;
        };
        if lease.fencing_token != fencing_token
            || lease.released_at_unix_ms.is_some()
            || lease.expires_at_unix_ms <= now_unix_ms
            || ttl_ms <= 0
        {
            return false;
        }
        lease.heartbeat_at_unix_ms = now_unix_ms;
        lease.expires_at_unix_ms = expires_at_unix_ms;
        drop(leases);
        if let Some(reservation) = self
            .slot_reservations
            .lock()
            .expect("Adam slot reservation store lock")
            .get_mut(lease_id)
        {
            reservation.expires_at_unix_ms = expires_at_unix_ms;
        }
        true
    }

    fn validate_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        runtime_instance_id: &str,
        now_unix_ms: i64,
    ) -> bool {
        self.slot_leases
            .lock()
            .expect("Adam persistent slot lease lock")
            .get(lease_id)
            .is_some_and(|lease| {
                lease.fencing_token == fencing_token
                    && lease.runtime_instance_id == runtime_instance_id
                    && lease.released_at_unix_ms.is_none()
                    && lease.expires_at_unix_ms > now_unix_ms
            })
    }

    fn release_slot_lease(
        &self,
        lease_id: &str,
        fencing_token: &str,
        released_at_unix_ms: i64,
    ) -> bool {
        let mut leases = self
            .slot_leases
            .lock()
            .expect("Adam persistent slot lease lock");
        let Some(lease) = leases.get_mut(lease_id) else {
            return false;
        };
        if lease.fencing_token != fencing_token || lease.released_at_unix_ms.is_some() {
            return false;
        }
        lease.released_at_unix_ms = Some(released_at_unix_ms);
        true
    }

    fn slot_leases(&self) -> Vec<AdamPersistentSlotLease> {
        self.slot_leases
            .lock()
            .expect("Adam persistent slot lease lock")
            .values()
            .cloned()
            .collect()
    }

    fn compatible_snapshot(
        &self,
        compatibility: &AdamSnapshotCompatibility<'_>,
    ) -> Option<AdamKvSnapshotRecord> {
        self.snapshots
            .lock()
            .expect("Adam snapshot store lock")
            .iter()
            .find(|snapshot| snapshot.is_compatible_with(compatibility))
            .cloned()
    }

    fn upsert_snapshot(&self, snapshot: AdamKvSnapshotRecord) {
        let mut snapshots = self.snapshots.lock().expect("Adam snapshot store lock");
        snapshots.retain(|current| current.snapshot_id != snapshot.snapshot_id);
        snapshots.push(snapshot);
    }

    fn remove_snapshot(&self, snapshot_id: &str) -> bool {
        let mut snapshots = self.snapshots.lock().expect("Adam snapshot store lock");
        let before = snapshots.len();
        snapshots.retain(|snapshot| snapshot.snapshot_id != snapshot_id);
        before != snapshots.len()
    }

    fn snapshots(&self) -> Vec<AdamKvSnapshotRecord> {
        self.snapshots
            .lock()
            .expect("Adam snapshot store lock")
            .clone()
    }

    fn live_residencies(
        &self,
        endpoint_id: &str,
        runtime_instance_id: &str,
    ) -> Vec<AdamLiveSlotResidencyRecord> {
        self.live_residencies
            .lock()
            .expect("Adam live residency store lock")
            .iter()
            .filter(|record| {
                record.endpoint_id == endpoint_id
                    && record.runtime_instance_id == runtime_instance_id
            })
            .cloned()
            .collect()
    }

    fn engine_evidence(&self) -> Vec<AdamEngineCompletionEvidence> {
        self.engine_evidence
            .lock()
            .expect("Adam engine evidence store lock")
            .clone()
    }

    fn commit_engine_completion_if_current(&self, commit: AdamEngineCompletionCommit) -> bool {
        let leases = self
            .slot_leases
            .lock()
            .expect("Adam persistent slot lease lock");
        let current = leases.get(&commit.lease_id).is_some_and(|lease| {
            lease.fencing_token == commit.fencing_token
                && lease.runtime_instance_id == commit.runtime_instance_id
                && lease.endpoint_id == commit.evidence.endpoint_id
                && lease.slot_id == commit.evidence.slot_id
                && lease.released_at_unix_ms.is_none()
                && lease.expires_at_unix_ms > commit.now_unix_ms
                && commit.evidence.runtime_instance_id == lease.runtime_instance_id
                && commit.residency.endpoint_id == lease.endpoint_id
                && commit.residency.runtime_instance_id == lease.runtime_instance_id
                && commit.residency.slot_id == lease.slot_id
        });
        if !current {
            return false;
        }
        let mut evidence = self
            .engine_evidence
            .lock()
            .expect("Adam engine evidence store lock");
        let mut residencies = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        evidence.retain(|current| current.evidence_id != commit.evidence.evidence_id);
        evidence.push(commit.evidence);
        residencies.retain(|current| {
            !((current.endpoint_id == commit.residency.endpoint_id
                && current.runtime_instance_id == commit.residency.runtime_instance_id
                && current.slot_id == commit.residency.slot_id)
                || (current.inhouse_polar_id == commit.residency.inhouse_polar_id
                    && current.thread_id == commit.residency.thread_id
                    && current.projection_id == commit.residency.projection_id))
        });
        residencies.push(commit.residency);
        true
    }

    fn record_engine_evidence(&self, evidence: AdamEngineCompletionEvidence) {
        self.engine_evidence
            .lock()
            .expect("Adam engine evidence store lock")
            .push(evidence);
    }

    fn upsert_live_residency(&self, residency: AdamLiveSlotResidencyRecord) {
        let mut records = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        records.retain(|current| {
            !(current.endpoint_id == residency.endpoint_id
                && current.runtime_instance_id == residency.runtime_instance_id
                && current.slot_id == residency.slot_id
                || current.inhouse_polar_id == residency.inhouse_polar_id
                    && current.thread_id == residency.thread_id
                    && current.projection_id == residency.projection_id)
        });
        records.push(residency);
    }

    fn remove_live_residency(
        &self,
        endpoint_id: &str,
        runtime_instance_id: &str,
        slot_id: i64,
    ) -> bool {
        let mut records = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        let before = records.len();
        records.retain(|record| {
            record.endpoint_id != endpoint_id
                || record.runtime_instance_id != runtime_instance_id
                || record.slot_id != slot_id
        });
        before != records.len()
    }

    fn invalidate_live_residencies_for_runtime(
        &self,
        endpoint_id: &str,
        current_runtime_instance_id: &str,
    ) -> usize {
        let mut records = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        let before = records.len();
        records.retain(|record| {
            record.endpoint_id != endpoint_id
                || record.runtime_instance_id == current_runtime_instance_id
        });
        before.saturating_sub(records.len())
    }

    fn invalidate_live_residencies_for_context_epoch(
        &self,
        inhouse_polar_id: &str,
        thread_id: &str,
        current_context_compaction_epoch: i64,
    ) -> usize {
        let mut records = self
            .live_residencies
            .lock()
            .expect("Adam live residency store lock");
        let before = records.len();
        records.retain(|record| {
            record.inhouse_polar_id != inhouse_polar_id
                || record.thread_id != thread_id
                || record.context_compaction_epoch == current_context_compaction_epoch
        });
        before.saturating_sub(records.len())
    }

    fn upsert_prepared_compact_slot(&self, prepared: AdamPreparedCompactSlot) {
        let mut records = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock");
        for current in records.values_mut().filter(|current| {
            current.candidate_compaction_id != prepared.candidate_compaction_id
                && current.endpoint_id == prepared.endpoint_id
                && current.runtime_instance_id == prepared.runtime_instance_id
                && current.slot_id == prepared.slot_id
                && current.discard_reason.is_none()
        }) {
            current.promoted = false;
            current.discard_reason = Some("slot_reused_by_prepared_compact".to_string());
        }
        records.insert(prepared.candidate_compaction_id.clone(), prepared);
    }

    fn prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
    ) -> Option<AdamPreparedCompactSlot> {
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .get(candidate_compaction_id)
            .cloned()
    }

    fn prepared_compact_slot_for_session(
        &self,
        session_id: &str,
    ) -> Option<AdamPreparedCompactSlot> {
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .values()
            .find(|prepared| {
                prepared.session_id == session_id
                    && !prepared.promoted
                    && prepared.discard_reason.is_none()
            })
            .cloned()
    }

    fn prepared_compact_slots_for_session(&self, session_id: &str) -> Vec<AdamPreparedCompactSlot> {
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .values()
            .filter(|prepared| prepared.session_id == session_id)
            .cloned()
            .collect()
    }

    fn prepared_slot_ids(&self, endpoint_id: &str, runtime_instance_id: &str) -> Vec<i64> {
        let mut slots = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .values()
            .filter(|prepared| {
                prepared.endpoint_id == endpoint_id
                    && prepared.runtime_instance_id == runtime_instance_id
                    && !prepared.promoted
                    && prepared.discard_reason.is_none()
            })
            .map(|prepared| prepared.slot_id)
            .collect::<Vec<_>>();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    fn promoted_prepared_compact_slot(
        &self,
        lookup: &AdamPromotedPreparedLookup<'_>,
    ) -> Option<AdamPreparedCompactSlot> {
        self.prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock")
            .values()
            .find(|prepared| {
                prepared.endpoint_id == lookup.endpoint_id
                    && prepared.runtime_instance_id == lookup.runtime_instance_id
                    && prepared.model_id == lookup.model_id
                    && prepared.tokenizer_hash == lookup.tokenizer_hash
                    && prepared.context_params_hash == lookup.context_params_hash
                    && prepared.exact_prefix_hash == lookup.exact_prefix_hash
                    && prepared.target_context_compaction_epoch == lookup.context_compaction_epoch
                    && prepared.promoted
                    && prepared.committed_compaction_id.is_some()
                    && prepared.discard_reason.is_none()
            })
            .cloned()
    }

    fn promote_prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
        committed_compaction_id: &str,
    ) -> Option<AdamPreparedCompactSlot> {
        let mut prepared = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock");
        let record = prepared.get_mut(candidate_compaction_id)?;
        record.promoted = true;
        record.committed_compaction_id = Some(committed_compaction_id.to_string());
        record.discard_reason = None;
        Some(record.clone())
    }

    fn discard_prepared_compact_slot(
        &self,
        candidate_compaction_id: &str,
        reason: &str,
    ) -> Option<AdamPreparedCompactSlot> {
        let mut prepared = self
            .prepared_compact_slots
            .lock()
            .expect("Adam prepared compact slot store lock");
        let record = prepared.get_mut(candidate_compaction_id)?;
        record.promoted = false;
        record.committed_compaction_id = None;
        record.discard_reason = Some(reason.to_string());
        Some(record.clone())
    }
}
