//! Backend-neutral persistence contracts.
// storage-operation-catalog: storage.health.probe
//!
//! This crate owns stable operation, error, health and shutdown semantics. It
//! intentionally defines no SQL, rows, driver connections or transactions.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

mod metrics;

/// An explicit operation with a complete configurable object, never a field patch.
/// Omitted operations, deletion and unknown operation names are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RecordOperation<T> {
    Insert(T),
    Update(T),
    Upsert(T),
}

impl<T> RecordOperation<T> {
    pub fn into_value(self) -> T {
        match self {
            Self::Insert(value) | Self::Update(value) | Self::Upsert(value) => value,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> RecordOperation<U> {
        match self {
            Self::Insert(value) => RecordOperation::Insert(f(value)),
            Self::Update(value) => RecordOperation::Update(f(value)),
            Self::Upsert(value) => RecordOperation::Upsert(f(value)),
        }
    }
    pub fn mode(&self) -> RecordWriteMode {
        match self {
            Self::Insert(_) => RecordWriteMode::Insert,
            Self::Update(_) => RecordWriteMode::Update,
            Self::Upsert(_) => RecordWriteMode::Upsert,
        }
    }

    pub fn value(&self) -> &T {
        match self {
            Self::Insert(value) | Self::Update(value) | Self::Upsert(value) => value,
        }
    }
}

impl<T> std::ops::Deref for RecordOperation<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value()
    }
}

impl<T> std::ops::DerefMut for RecordOperation<T> {
    fn deref_mut(&mut self) -> &mut T {
        match self {
            Self::Insert(value) | Self::Update(value) | Self::Upsert(value) => value,
        }
    }
}

/// Atomic object-write preconditions shared by domain store ports.
/// This is not an installation policy and has no deletion or merge operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordWriteMode {
    Insert,
    Update,
    Upsert,
}

/// A materialized record and the atomic write requested by its owning domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWrite<T> {
    pub mode: RecordWriteMode,
    pub record: T,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordWriteOutcome {
    Created,
    Replaced,
    Skipped,
}

pub use metrics::{
    StorageOperationMetricsSnapshot, StorageRuntimeMetricEvent, StorageRuntimeMetrics,
    StorageRuntimeMetricsSnapshot,
};

/// A validated, stable identifier used for operation metrics and policy.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct StorageOperationId(String);

impl StorageOperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidStorageIdentifier> {
        let value = value.into();
        if is_valid_identifier(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidStorageIdentifier::new("operation", value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StorageOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A validated canonical storage target identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct StorageTargetId(String);

impl StorageTargetId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidStorageIdentifier> {
        let value = value.into();
        if is_valid_identifier(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidStorageIdentifier::new("target", value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StorageTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The logical category of one declared persistence operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Read,
    Write,
    Transaction,
    Migration,
    Maintenance,
}

/// The observable consistency required by one operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyRequirement {
    Eventual,
    ReadYourWrites,
    Snapshot,
    Atomic,
}

/// Whether the caller may retry a fully bounded logical operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryDisposition {
    Never,
    SafeRead,
    IdempotentTransaction,
}

impl RetryDisposition {
    pub const fn permits_retry(self) -> bool {
        !matches!(self, Self::Never)
    }
}

/// Stable metadata for one project-owned storage operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageOperation {
    id: StorageOperationId,
    kind: OperationKind,
    consistency: ConsistencyRequirement,
    retry: RetryDisposition,
}

impl StorageOperation {
    pub fn new(
        id: StorageOperationId,
        kind: OperationKind,
        consistency: ConsistencyRequirement,
        retry: RetryDisposition,
    ) -> Self {
        Self {
            id,
            kind,
            consistency,
            retry,
        }
    }

    pub fn id(&self) -> &StorageOperationId {
        &self.id
    }

    pub const fn kind(&self) -> OperationKind {
        self.kind
    }

    pub const fn consistency(&self) -> ConsistencyRequirement {
        self.consistency
    }

    pub const fn retry(&self) -> RetryDisposition {
        self.retry
    }
}

/// Normalized error categories that adapters may expose to domain ports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageErrorKind {
    InvalidInput,
    Unavailable,
    Timeout,
    Conflict,
    Constraint,
    Corrupt,
    ShuttingDown,
    Internal,
}

/// Backend-neutral identity for diagnostics without exposing a driver type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackendKind {
    Sqlite,
    Postgres,
}

/// Sanitized backend diagnostic data suitable for logs and error mapping.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageDiagnostic {
    pub backend: Option<StorageBackendKind>,
    pub code: Option<String>,
    pub constraint: Option<String>,
}

/// A typed persistence failure tied to its target operation and retry policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageError {
    kind: StorageErrorKind,
    message: String,
    operation: Option<StorageOperationId>,
    target: Option<StorageTargetId>,
    retry: RetryDisposition,
    diagnostic: StorageDiagnostic,
}

impl StorageError {
    pub fn new(
        kind: StorageErrorKind,
        message: impl Into<String>,
        operation: Option<StorageOperationId>,
        target: Option<StorageTargetId>,
        retry: RetryDisposition,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            operation,
            target,
            retry,
            diagnostic: StorageDiagnostic::default(),
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: StorageDiagnostic) -> Self {
        self.diagnostic = diagnostic;
        self
    }

    pub fn with_target(mut self, target: StorageTargetId) -> Self {
        self.target = Some(target);
        self
    }

    pub const fn kind(&self) -> StorageErrorKind {
        self.kind
    }

    pub fn operation(&self) -> Option<&StorageOperationId> {
        self.operation.as_ref()
    }

    pub fn target(&self) -> Option<&StorageTargetId> {
        self.target.as_ref()
    }

    pub const fn retry(&self) -> RetryDisposition {
        self.retry
    }

    pub fn diagnostic(&self) -> &StorageDiagnostic {
        &self.diagnostic
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "storage {:?}: {}", self.kind, self.message)
    }
}

impl Error for StorageError {}

/// Normalizes an internal implementation error at a typed storage boundary.
/// Backend runtimes may provide more specific conversions before this fallback
/// is used by an adapter.
impl From<anyhow::Error> for StorageError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(
            StorageErrorKind::Internal,
            error.to_string(),
            None,
            None,
            RetryDisposition::Never,
        )
    }
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::new(
            StorageErrorKind::Internal,
            error.to_string(),
            None,
            None,
            RetryDisposition::Never,
        )
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Health is a target-level projection; it never contains a backend handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageHealth {
    target: StorageTargetId,
    status: StorageHealthStatus,
    detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    ShuttingDown,
}

impl StorageHealth {
    pub fn healthy(target: StorageTargetId) -> Self {
        Self {
            target,
            status: StorageHealthStatus::Healthy,
            detail: None,
        }
    }

    pub fn status(&self) -> StorageHealthStatus {
        self.status
    }

    pub fn target(&self) -> &StorageTargetId {
        &self.target
    }

    pub const fn is_healthy(&self) -> bool {
        matches!(self.status, StorageHealthStatus::Healthy)
    }
}

/// Accounting returned by an explicit, awaited storage shutdown.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageShutdownReport {
    target: StorageTargetId,
    accepted_operations: u64,
    completed_operations: u64,
    rejected_operations: u64,
}

impl StorageShutdownReport {
    pub fn drained(
        target: StorageTargetId,
        accepted_operations: u64,
        completed_operations: u64,
    ) -> Self {
        Self {
            target,
            accepted_operations,
            completed_operations,
            rejected_operations: accepted_operations.saturating_sub(completed_operations),
        }
    }

    pub fn target(&self) -> &StorageTargetId {
        &self.target
    }

    pub const fn accepted_operations(&self) -> u64 {
        self.accepted_operations
    }

    pub const fn completed_operations(&self) -> u64 {
        self.completed_operations
    }

    pub const fn rejected_operations(&self) -> u64 {
        self.rejected_operations
    }
}

/// A backend-neutral semantic case for later multi-backend contract tests.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageContractScenario {
    AtomicCommitOrRollback,
    IdempotentAppend,
    ReadYourWrites,
    ShutdownRejectsNewAdmission,
}

/// A failed attempt to construct a stable operation or target identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidStorageIdentifier {
    subject: &'static str,
    value: String,
}

impl InvalidStorageIdentifier {
    fn new(subject: &'static str, value: String) -> Self {
        Self { subject, value }
    }
}

impl fmt::Display for InvalidStorageIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid storage {} identifier: {}",
            self.subject, self.value
        )
    }
}

impl Error for InvalidStorageIdentifier {}

fn is_valid_identifier(value: &str) -> bool {
    let mut characters = value.bytes();
    matches!(characters.next(), Some(b'a'..=b'z'))
        && characters.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
}


