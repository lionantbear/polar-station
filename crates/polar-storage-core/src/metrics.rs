//! Backend-neutral runtime metric counters.
//!
//! The counters deliberately use stable operation IDs and durations rather
//! than SQL text, connection handles, or payload-derived labels.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use crate::StorageOperationId;

#[derive(Clone, Debug)]
pub struct StorageRuntimeMetricEvent {
    pub operation_id: StorageOperationId,
    pub queue_depth: u64,
    pub admission_wait: Duration,
    pub transaction_duration: Option<Duration>,
    pub pool_acquire: Option<Duration>,
    pub copy_bytes: u64,
    pub runtime_dispatches: u64,
    pub statements: u64,
    pub transactions: u64,
    pub rows_read: u64,
    pub rows_written: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StorageOperationMetricsSnapshot {
    pub attempted: u64,
    pub completed: u64,
    pub rejected: u64,
    pub runtime_dispatches: u64,
    pub statements: u64,
    pub transactions: u64,
    pub rows_read: u64,
    pub rows_written: u64,
    pub bytes_written: u64,
    pub admission_wait: Duration,
    pub execution: Duration,
    pub transaction_duration: Duration,
    pub pool_acquire: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageRuntimeMetricsSnapshot {
    pub accepted_operations: u64,
    pub completed_operations: u64,
    pub rejected_operations: u64,
    pub queue_depth: u64,
    pub admission_wait: Duration,
    pub transaction_duration: Duration,
    pub pool_acquire: Duration,
    pub copy_bytes: u64,
    pub per_operation: BTreeMap<StorageOperationId, StorageOperationMetricsSnapshot>,
}

#[derive(Default)]
struct StorageOperationMetrics {
    attempted: AtomicU64,
    completed: AtomicU64,
    rejected: AtomicU64,
    runtime_dispatches: AtomicU64,
    statements: AtomicU64,
    transactions: AtomicU64,
    rows_read: AtomicU64,
    rows_written: AtomicU64,
    bytes_written: AtomicU64,
    admission_wait_nanos: AtomicU64,
    execution_nanos: AtomicU64,
    transaction_duration_nanos: AtomicU64,
    pool_acquire_nanos: AtomicU64,
}

#[derive(Default)]
struct StorageRuntimeMetricsInner {
    accepted_operations: AtomicU64,
    completed_operations: AtomicU64,
    rejected_operations: AtomicU64,
    queue_depth: AtomicU64,
    admission_wait_nanos: AtomicU64,
    transaction_duration_nanos: AtomicU64,
    pool_acquire_nanos: AtomicU64,
    copy_bytes: AtomicU64,
    per_operation: Mutex<BTreeMap<StorageOperationId, Arc<StorageOperationMetrics>>>,
}

/// Shared counters owned by a storage runtime instance.
///
/// A runtime records backend-local details here and exports the snapshot to
/// the host metrics registry. The operation ID attached to each event stays
/// low-cardinality and never contains SQL text.
#[derive(Clone, Default)]
pub struct StorageRuntimeMetrics {
    inner: Arc<StorageRuntimeMetricsInner>,
}

impl StorageRuntimeMetrics {
    pub fn observe(&self, event: StorageRuntimeMetricEvent) {
        let operation = self.operation(&event.operation_id);
        self.inner
            .accepted_operations
            .fetch_add(1, Ordering::Relaxed);
        operation.attempted.fetch_add(1, Ordering::Relaxed);
        self.inner
            .queue_depth
            .store(event.queue_depth, Ordering::Relaxed);
        add_duration(&self.inner.admission_wait_nanos, event.admission_wait);
        add_duration(&operation.admission_wait_nanos, event.admission_wait);
        operation
            .runtime_dispatches
            .fetch_add(event.runtime_dispatches, Ordering::Relaxed);
        operation
            .statements
            .fetch_add(event.statements, Ordering::Relaxed);
        operation
            .transactions
            .fetch_add(event.transactions, Ordering::Relaxed);
        operation
            .rows_read
            .fetch_add(event.rows_read, Ordering::Relaxed);
        operation
            .rows_written
            .fetch_add(event.rows_written, Ordering::Relaxed);
        operation
            .bytes_written
            .fetch_add(event.copy_bytes, Ordering::Relaxed);
        if let Some(transaction_duration) = event.transaction_duration {
            add_duration(&self.inner.transaction_duration_nanos, transaction_duration);
            add_duration(&operation.transaction_duration_nanos, transaction_duration);
        }
        if let Some(pool_acquire) = event.pool_acquire {
            add_duration(&self.inner.pool_acquire_nanos, pool_acquire);
            add_duration(&operation.pool_acquire_nanos, pool_acquire);
        }
        self.inner
            .copy_bytes
            .fetch_add(event.copy_bytes, Ordering::Relaxed);
    }

    pub fn complete(&self, operation_id: &StorageOperationId) {
        self.inner
            .completed_operations
            .fetch_add(1, Ordering::Relaxed);
        self.operation(operation_id)
            .completed
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn reject(&self, operation_id: &StorageOperationId) {
        self.inner
            .rejected_operations
            .fetch_add(1, Ordering::Relaxed);
        self.operation(operation_id)
            .rejected
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_transaction_duration(
        &self,
        operation_id: &StorageOperationId,
        transaction_duration: Duration,
    ) {
        self.operation(operation_id)
            .transactions
            .fetch_add(1, Ordering::Relaxed);
        add_duration(&self.inner.transaction_duration_nanos, transaction_duration);
        add_duration(
            &self.operation(operation_id).transaction_duration_nanos,
            transaction_duration,
        );
    }

    pub fn record_pool_acquire(&self, pool_acquire: Duration) {
        add_duration(&self.inner.pool_acquire_nanos, pool_acquire);
    }

    pub fn record_copy_bytes(&self, copy_bytes: u64) {
        self.inner
            .copy_bytes
            .fetch_add(copy_bytes, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StorageRuntimeMetricsSnapshot {
        let per_operation = self
            .inner
            .per_operation
            .lock()
            .expect("per-operation metrics lock")
            .iter()
            .map(|(operation_id, metrics)| {
                (
                    operation_id.clone(),
                    StorageOperationMetricsSnapshot {
                        attempted: metrics.attempted.load(Ordering::Relaxed),
                        completed: metrics.completed.load(Ordering::Relaxed),
                        rejected: metrics.rejected.load(Ordering::Relaxed),
                        runtime_dispatches: metrics.runtime_dispatches.load(Ordering::Relaxed),
                        statements: metrics.statements.load(Ordering::Relaxed),
                        transactions: metrics.transactions.load(Ordering::Relaxed),
                        rows_read: metrics.rows_read.load(Ordering::Relaxed),
                        rows_written: metrics.rows_written.load(Ordering::Relaxed),
                        bytes_written: metrics.bytes_written.load(Ordering::Relaxed),
                        admission_wait: duration(
                            metrics.admission_wait_nanos.load(Ordering::Relaxed),
                        ),
                        execution: duration(metrics.execution_nanos.load(Ordering::Relaxed)),
                        transaction_duration: duration(
                            metrics.transaction_duration_nanos.load(Ordering::Relaxed),
                        ),
                        pool_acquire: duration(metrics.pool_acquire_nanos.load(Ordering::Relaxed)),
                    },
                )
            })
            .collect();
        StorageRuntimeMetricsSnapshot {
            accepted_operations: self.inner.accepted_operations.load(Ordering::Relaxed),
            completed_operations: self.inner.completed_operations.load(Ordering::Relaxed),
            rejected_operations: self.inner.rejected_operations.load(Ordering::Relaxed),
            queue_depth: self.inner.queue_depth.load(Ordering::Relaxed),
            admission_wait: duration(self.inner.admission_wait_nanos.load(Ordering::Relaxed)),
            transaction_duration: duration(
                self.inner
                    .transaction_duration_nanos
                    .load(Ordering::Relaxed),
            ),
            pool_acquire: duration(self.inner.pool_acquire_nanos.load(Ordering::Relaxed)),
            copy_bytes: self.inner.copy_bytes.load(Ordering::Relaxed),
            per_operation,
        }
    }

    fn operation(&self, operation_id: &StorageOperationId) -> Arc<StorageOperationMetrics> {
        let mut metrics = self
            .inner
            .per_operation
            .lock()
            .expect("per-operation metrics lock");
        metrics
            .entry(operation_id.clone())
            .or_insert_with(|| Arc::new(StorageOperationMetrics::default()))
            .clone()
    }
}

fn add_duration(counter: &AtomicU64, value: Duration) {
    counter.fetch_add(
        u64::try_from(value.as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

fn duration(nanos: u64) -> Duration {
    Duration::from_nanos(nanos)
}
