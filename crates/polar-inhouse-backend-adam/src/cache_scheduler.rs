//! Adam-owned active-working-set scheduler for managed llama slots.

use crate::engine_manager::AdamEngineTurnPriority;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheAffinityKey {
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub projection_id: String,
    pub exact_prefix_hash: String,
    pub context_compaction_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamCacheResident {
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub affinity: CacheAffinityKey,
    pub slot_id: i64,
    pub last_activity_unix_ms: i64,
    pub prefix_token_count: i64,
    pub in_flight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamCacheScheduleRequest {
    pub request_id: String,
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub affinity: CacheAffinityKey,
    pub priority: AdamEngineTurnPriority,
    pub requested_at_unix_ms: i64,
}

/// Redacted, transient operator projection of one scheduler waiter.
///
/// The projection intentionally omits prompt, projection, prefix-hash, and
/// connector details. Queue positions and depths are recomputed from the live
/// queues whenever status is requested rather than persisted as durable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdamWaitQueueStatus {
    pub endpoint_id: String,
    pub runtime_instance_id: String,
    pub scheduler_request_id: String,
    pub inhouse_polar_id: String,
    pub thread_id: String,
    pub scheduling_priority: String,
    pub position: usize,
    pub depth: usize,
    pub requested_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdamCacheAllocationReason {
    CacheAffinityHit,
    ThreadAffinityReuse,
    IdleSlot,
    FairnessEviction,
    WaitQueue,
}

impl AdamCacheAllocationReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CacheAffinityHit => "cache_affinity_hit",
            Self::ThreadAffinityReuse => "thread_affinity_reuse",
            Self::IdleSlot => "idle_slot",
            Self::FairnessEviction => "fairness_eviction",
            Self::WaitQueue => "wait_queue",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdamCacheScheduleDecision {
    pub slot_id: Option<i64>,
    pub allocation_reason: AdamCacheAllocationReason,
    pub snapshot_before_eviction: bool,
    pub evicted: Option<AdamCacheResident>,
    pub wait_queue_position: Option<usize>,
    pub wait_queue_depth: Option<usize>,
    pub evicted_active_residency: bool,
}

#[derive(Debug, Clone)]
pub struct AdamCacheSchedulerConfig {
    pub activity_ttl_ms: i64,
    pub per_inhouse_polar_quota: usize,
    pub max_wait_queue_depth: usize,
}

impl Default for AdamCacheSchedulerConfig {
    fn default() -> Self {
        Self {
            activity_ttl_ms: 120_000,
            per_inhouse_polar_quota: 2,
            max_wait_queue_depth: 1_024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AdamCacheEndpointKey {
    endpoint_id: String,
    runtime_instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AdamCacheSlotKey {
    endpoint: AdamCacheEndpointKey,
    slot_id: i64,
}

#[derive(Debug, Clone)]
struct AdamCacheWaiter {
    request_id: String,
    inhouse_polar_id: String,
    thread_id: String,
    priority: AdamEngineTurnPriority,
    requested_at_unix_ms: i64,
}

#[derive(Debug, Default)]
struct AdamCacheSchedulerState {
    residents: HashMap<AdamCacheSlotKey, AdamCacheResident>,
    wait_queues: HashMap<AdamCacheEndpointKey, VecDeque<AdamCacheWaiter>>,
}

/// Process-wide scheduler owned by `AdamBackendRuntime` through the engine
/// manager. It protects recent user work, preserves cache affinity, and makes
/// background work yield through a bounded fairness queue.
#[derive(Debug)]
pub struct AdamCacheScheduler {
    config: AdamCacheSchedulerConfig,
    state: Mutex<AdamCacheSchedulerState>,
}

impl AdamCacheScheduler {
    pub fn new(config: AdamCacheSchedulerConfig) -> Self {
        Self {
            config,
            state: Mutex::new(AdamCacheSchedulerState::default()),
        }
    }

    pub fn protected_activity_ttl_ms(&self) -> i64 {
        self.config.activity_ttl_ms
    }

    pub fn observe_residency(&self, resident: AdamCacheResident) {
        let mut state = self.state.lock().expect("Adam cache scheduler lock");
        let slot_key = resident.slot_key();
        if let Some(existing) = state.residents.get_mut(&slot_key) {
            if existing.affinity == resident.affinity {
                existing.last_activity_unix_ms = existing
                    .last_activity_unix_ms
                    .max(resident.last_activity_unix_ms);
                existing.prefix_token_count =
                    existing.prefix_token_count.max(resident.prefix_token_count);
                existing.in_flight = resident.in_flight;
                return;
            }
        }
        state.residents.insert(slot_key, resident);
    }

    pub fn remove_residency(&self, endpoint_id: &str, runtime_instance_id: &str, slot_id: i64) {
        self.state
            .lock()
            .expect("Adam cache scheduler lock")
            .residents
            .remove(&AdamCacheSlotKey {
                endpoint: AdamCacheEndpointKey {
                    endpoint_id: endpoint_id.to_string(),
                    runtime_instance_id: runtime_instance_id.to_string(),
                },
                slot_id,
            });
    }

    pub fn wait_queue_depth(&self, endpoint_id: &str, runtime_instance_id: &str) -> usize {
        self.state
            .lock()
            .expect("Adam cache scheduler lock")
            .wait_queues
            .get(&AdamCacheEndpointKey {
                endpoint_id: endpoint_id.to_string(),
                runtime_instance_id: runtime_instance_id.to_string(),
            })
            .map_or(0, VecDeque::len)
    }

    /// Returns the current redacted queue view in stable endpoint/runtime and
    /// queue-position order. Entries disappear as soon as they are admitted or
    /// cancelled because the projection is derived directly from live state.
    pub fn wait_queue_status(&self) -> Vec<AdamWaitQueueStatus> {
        let state = self.state.lock().expect("Adam cache scheduler lock");
        let mut status = state
            .wait_queues
            .iter()
            .flat_map(|(endpoint, queue)| {
                let depth = queue.len();
                queue
                    .iter()
                    .enumerate()
                    .map(move |(index, waiter)| AdamWaitQueueStatus {
                        endpoint_id: endpoint.endpoint_id.clone(),
                        runtime_instance_id: endpoint.runtime_instance_id.clone(),
                        scheduler_request_id: waiter.request_id.clone(),
                        inhouse_polar_id: waiter.inhouse_polar_id.clone(),
                        thread_id: waiter.thread_id.clone(),
                        scheduling_priority: match waiter.priority {
                            AdamEngineTurnPriority::ActiveThread => "active_thread",
                            AdamEngineTurnPriority::Background => "background",
                        }
                        .to_string(),
                        position: index + 1,
                        depth,
                        requested_at_unix_ms: waiter.requested_at_unix_ms,
                    })
            })
            .collect::<Vec<_>>();
        status.sort_by(|left, right| {
            (
                &left.endpoint_id,
                &left.runtime_instance_id,
                left.position,
                &left.scheduler_request_id,
            )
                .cmp(&(
                    &right.endpoint_id,
                    &right.runtime_instance_id,
                    right.position,
                    &right.scheduler_request_id,
                ))
        });
        status
    }

    pub fn cancel_waiting(
        &self,
        endpoint_id: &str,
        runtime_instance_id: &str,
        request_id: &str,
    ) -> bool {
        let mut state = self.state.lock().expect("Adam cache scheduler lock");
        let endpoint = AdamCacheEndpointKey {
            endpoint_id: endpoint_id.to_string(),
            runtime_instance_id: runtime_instance_id.to_string(),
        };
        let Some(queue) = state.wait_queues.get_mut(&endpoint) else {
            return false;
        };
        let before = queue.len();
        queue.retain(|waiter| waiter.request_id != request_id);
        let removed = before != queue.len();
        if queue.is_empty() {
            state.wait_queues.remove(&endpoint);
        }
        removed
    }

    pub fn schedule(
        &self,
        request: &AdamCacheScheduleRequest,
        usable_slot_ids: &[i64],
    ) -> AdamCacheScheduleDecision {
        self.schedule_at(request, usable_slot_ids, request.requested_at_unix_ms)
    }

    /// Evaluates a stable FIFO request at the current time. The original
    /// `requested_at_unix_ms` remains unchanged for queue ordering while the
    /// evaluation clock advances so activity TTLs can actually expire.
    pub fn schedule_at(
        &self,
        request: &AdamCacheScheduleRequest,
        usable_slot_ids: &[i64],
        evaluated_at_unix_ms: i64,
    ) -> AdamCacheScheduleDecision {
        let mut state = self.state.lock().expect("Adam cache scheduler lock");
        let endpoint = request.endpoint_key();
        if let Some(queue) = state.wait_queues.get_mut(&endpoint) {
            // Manager-owned timeout/cancellation normally removes waiters.
            // This longer fail-safe only bounds entries orphaned by an
            // abruptly dropped task; it must not expire a valid default wait
            // before the protected activity TTL can elapse.
            let oldest_allowed =
                evaluated_at_unix_ms.saturating_sub(self.config.activity_ttl_ms.max(300_000));
            queue.retain(|waiter| waiter.requested_at_unix_ms >= oldest_allowed);
        }
        let must_wait_behind_existing = if let Some(queue) = state.wait_queues.get_mut(&endpoint) {
            let request_position = queue
                .iter()
                .position(|waiter| waiter.request_id == request.request_id);
            if queue
                .front()
                .is_some_and(|front| front.request_id != request.request_id)
            {
                if request_position.is_none() {
                    enqueue_waiter(queue, request, self.config.max_wait_queue_depth);
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if must_wait_behind_existing {
            let position = state
                .wait_queues
                .get(&endpoint)
                .and_then(|queue| {
                    queue
                        .iter()
                        .position(|waiter| waiter.request_id == request.request_id)
                })
                .map(|index| index + 1);
            let depth = state.wait_queues.get(&endpoint).map(VecDeque::len);
            return waiting_decision(position, depth);
        }
        if let Some(slot_id) = state
            .residents
            .values()
            .find(|resident| {
                resident.belongs_to(request)
                    && resident.affinity == request.affinity
                    && !resident.in_flight
                    && usable_slot_ids.contains(&resident.slot_id)
            })
            .map(|resident| resident.slot_id)
        {
            admit_waiter(&mut state, &endpoint, &request.request_id);
            return AdamCacheScheduleDecision {
                slot_id: Some(slot_id),
                allocation_reason: AdamCacheAllocationReason::CacheAffinityHit,
                snapshot_before_eviction: false,
                evicted: None,
                wait_queue_position: None,
                wait_queue_depth: None,
                evicted_active_residency: false,
            };
        }

        // A same-thread follow-up necessarily has a new projection and often
        // a new exact prefix hash. It may reuse that thread's physical slot so
        // a one-slot endpoint keeps making progress, but it is not an exact
        // cache hit. The manager consequently selects an honest RewarmPrefix
        // plan unless an exact compatible snapshot exists.
        if let Some(slot_id) = state
            .residents
            .values()
            .find(|resident| {
                resident.belongs_to(request)
                    && resident.affinity.inhouse_polar_id == request.affinity.inhouse_polar_id
                    && resident.affinity.thread_id == request.affinity.thread_id
                    && resident.affinity.context_compaction_epoch
                        == request.affinity.context_compaction_epoch
                    && !resident.in_flight
                    && usable_slot_ids.contains(&resident.slot_id)
            })
            .map(|resident| resident.slot_id)
        {
            admit_waiter(&mut state, &endpoint, &request.request_id);
            return AdamCacheScheduleDecision {
                slot_id: Some(slot_id),
                allocation_reason: AdamCacheAllocationReason::ThreadAffinityReuse,
                snapshot_before_eviction: false,
                evicted: None,
                wait_queue_position: None,
                wait_queue_depth: None,
                evicted_active_residency: false,
            };
        }

        if let Some(slot_id) = usable_slot_ids.iter().copied().find(|slot_id| {
            !state.residents.contains_key(&AdamCacheSlotKey {
                endpoint: endpoint.clone(),
                slot_id: *slot_id,
            })
        }) {
            admit_waiter(&mut state, &endpoint, &request.request_id);
            return AdamCacheScheduleDecision {
                slot_id: Some(slot_id),
                allocation_reason: AdamCacheAllocationReason::IdleSlot,
                snapshot_before_eviction: false,
                evicted: None,
                wait_queue_position: None,
                wait_queue_depth: None,
                evicted_active_residency: false,
            };
        }

        let active_cutoff = evaluated_at_unix_ms.saturating_sub(self.config.activity_ttl_ms);
        let mut candidates = state
            .residents
            .values()
            .filter(|resident| {
                resident.belongs_to(request)
                    && usable_slot_ids.contains(&resident.slot_id)
                    && !resident.in_flight
                    && resident.last_activity_unix_ms < active_cutoff
            })
            .cloned()
            .collect::<Vec<_>>();
        let owner_counts = state
            .residents
            .values()
            .filter(|resident| resident.belongs_to(request))
            .fold(HashMap::<String, usize>::new(), |mut counts, resident| {
                *counts
                    .entry(resident.affinity.inhouse_polar_id.clone())
                    .or_default() += 1;
                counts
            });
        candidates.sort_by_key(|resident| {
            let above_quota = owner_counts
                .get(&resident.affinity.inhouse_polar_id)
                .copied()
                .unwrap_or_default()
                > self.config.per_inhouse_polar_quota;
            (
                !above_quota,
                resident.last_activity_unix_ms,
                resident.slot_id,
            )
        });
        if let Some(evicted) = candidates.into_iter().next() {
            admit_waiter(&mut state, &endpoint, &request.request_id);
            return AdamCacheScheduleDecision {
                slot_id: Some(evicted.slot_id),
                allocation_reason: AdamCacheAllocationReason::FairnessEviction,
                snapshot_before_eviction: evicted.prefix_token_count > 0,
                evicted_active_residency: evicted.last_activity_unix_ms >= active_cutoff,
                evicted: Some(evicted),
                wait_queue_position: None,
                wait_queue_depth: None,
            };
        }

        let queue = state.wait_queues.entry(endpoint).or_default();
        enqueue_waiter(queue, request, self.config.max_wait_queue_depth);
        let wait_queue_position = queue
            .iter()
            .position(|waiter| waiter.request_id == request.request_id)
            .map(|index| index + 1);
        let wait_queue_depth = Some(queue.len());
        waiting_decision(wait_queue_position, wait_queue_depth)
    }
}

impl AdamCacheResident {
    fn slot_key(&self) -> AdamCacheSlotKey {
        AdamCacheSlotKey {
            endpoint: AdamCacheEndpointKey {
                endpoint_id: self.endpoint_id.clone(),
                runtime_instance_id: self.runtime_instance_id.clone(),
            },
            slot_id: self.slot_id,
        }
    }

    fn belongs_to(&self, request: &AdamCacheScheduleRequest) -> bool {
        self.endpoint_id == request.endpoint_id
            && self.runtime_instance_id == request.runtime_instance_id
    }
}

impl AdamCacheScheduleRequest {
    fn endpoint_key(&self) -> AdamCacheEndpointKey {
        AdamCacheEndpointKey {
            endpoint_id: self.endpoint_id.clone(),
            runtime_instance_id: self.runtime_instance_id.clone(),
        }
    }
}

fn enqueue_waiter(
    queue: &mut VecDeque<AdamCacheWaiter>,
    request: &AdamCacheScheduleRequest,
    max_depth: usize,
) {
    if queue
        .iter()
        .any(|waiter| waiter.request_id == request.request_id)
        || queue.len() >= max_depth
    {
        return;
    }
    let waiter = AdamCacheWaiter {
        request_id: request.request_id.clone(),
        inhouse_polar_id: request.affinity.inhouse_polar_id.clone(),
        thread_id: request.affinity.thread_id.clone(),
        priority: request.priority.clone(),
        requested_at_unix_ms: request.requested_at_unix_ms,
    };
    match request.priority {
        AdamEngineTurnPriority::ActiveThread => {
            let index = queue
                .iter()
                .position(|queued| matches!(queued.priority, AdamEngineTurnPriority::Background))
                .unwrap_or(queue.len());
            queue.insert(index, waiter);
        }
        AdamEngineTurnPriority::Background => queue.push_back(waiter),
    }
}

fn admit_waiter(
    state: &mut AdamCacheSchedulerState,
    endpoint: &AdamCacheEndpointKey,
    request_id: &str,
) {
    let Some(queue) = state.wait_queues.get_mut(endpoint) else {
        return;
    };
    queue.retain(|waiter| waiter.request_id != request_id);
    if queue.is_empty() {
        state.wait_queues.remove(endpoint);
    }
}

fn waiting_decision(
    wait_queue_position: Option<usize>,
    wait_queue_depth: Option<usize>,
) -> AdamCacheScheduleDecision {
    AdamCacheScheduleDecision {
        slot_id: None,
        allocation_reason: AdamCacheAllocationReason::WaitQueue,
        snapshot_before_eviction: false,
        evicted: None,
        wait_queue_position,
        wait_queue_depth,
        evicted_active_residency: false,
    }
}

impl Default for AdamCacheScheduler {
    fn default() -> Self {
        Self::new(AdamCacheSchedulerConfig::default())
    }
}
