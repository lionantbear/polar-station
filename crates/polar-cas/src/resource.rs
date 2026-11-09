//! Shared synchronous source ports migrated from Polar Network Audio.
//! Call blocking adapters on a worker, never on an async executor or UI thread.
//! These ports provide bytes and observations, not CAS admission proofs.
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use thiserror::Error;

const DEFAULT_MAX_READ_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_LIST_ENTRIES: usize = 500;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourceLocator {
    pub uri: String,
}

impl ResourceLocator {
    pub fn local_path(path: impl AsRef<Path>) -> Self {
        Self {
            uri: format!("file://{}", path.as_ref().to_string_lossy()),
        }
    }

    pub fn scheme(&self) -> Option<&str> {
        self.uri.split_once(":").map(|(scheme, _)| scheme)
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourceVersion {
    pub size: Option<u64>,
    pub validator: Option<String>,
    #[serde(default, with = "nanos_serde::optional")]
    pub modified_ns: Option<u128>,
}

/// JSON timestamp adaptation for serde's internally tagged ContentDeserializer,
/// which supports u64 but not u128. Keep the original in-memory timestamp type.
pub mod nanos_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        let value = u64::try_from(*value).map_err(serde::ser::Error::custom)?;
        serializer.serialize_u64(value)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        u64::deserialize(deserializer).map(u128::from)
    }
    pub mod optional {
        use super::*;
        use serde::Serialize;
        pub fn serialize<S: Serializer>(
            value: &Option<u128>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            value
                .map(u64::try_from)
                .transpose()
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer)
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<u128>, D::Error> {
            Option::<u64>::deserialize(deserializer).map(|value| value.map(u128::from))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourceCaps {
    pub sequential_read: bool,
    pub read_at: bool,
    pub known_size: bool,
    pub conditional_read: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourceInfo {
    pub locator: ResourceLocator,
    pub version: ResourceVersion,
    pub caps: ResourceCaps,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ListRequest {
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourceEntry {
    pub name: String,
    pub locator: ResourceLocator,
    pub is_file: bool,
    pub size: Option<u64>,
    pub version: ResourceVersion,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct ResourcePage {
    pub entries: Vec<ResourceEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IoContext {
    pub max_read_bytes: usize,
    pub deadline: Option<Instant>,
    cancelled: Arc<AtomicBool>,
    reserved: Arc<AtomicU64>,
    total_limit: Option<u64>,
}

impl Default for IoContext {
    fn default() -> Self {
        Self {
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            deadline: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            reserved: Arc::new(AtomicU64::new(0)),
            total_limit: None,
        }
    }
}

impl IoContext {
    pub fn with_budget(bytes: u64) -> Self {
        Self {
            total_limit: Some(bytes),
            ..Self::default()
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn reserved_bytes(&self) -> u64 {
        self.reserved.load(Ordering::Acquire)
    }
    pub fn reserve(&self, bytes: usize) -> Result<(), ResourceError> {
        self.check(bytes)?;
        self.reserved
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(bytes as u64)
                    .filter(|next| self.total_limit.is_none_or(|limit| *next <= limit))
            })
            .map_err(|_| ResourceError::BudgetExceeded)?;
        Ok(())
    }

    pub fn check(&self, requested: usize) -> Result<(), ResourceError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(ResourceError::Cancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ResourceError::DeadlineExceeded);
        }
        if requested > self.max_read_bytes {
            return Err(ResourceError::ResourceLimit {
                requested,
                maximum: self.max_read_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("resource operation cancelled")]
    Cancelled,
    #[error("resource cumulative read budget exhausted")]
    BudgetExceeded,
    #[error("invalid resource locator")]
    InvalidLocator,
    #[error("resource scheme is not supported: {0}")]
    UnsupportedScheme(String),
    #[error("resource is not a regular file")]
    NotFile,
    #[error("resource was not found")]
    NotFound,
    #[error("resource access requires authorization")]
    AuthRequired,
    #[error("resource permission was denied")]
    PermissionDenied,
    #[error("resource is unavailable")]
    Unavailable,
    #[error("resource version changed")]
    VersionChanged,
    #[error("resource version cannot be verified")]
    VersionUnverifiable,
    #[error("resource does not support the requested capability")]
    CapabilityMissing,
    #[error("resource response was malformed: {0}")]
    MalformedResponse(&'static str),
    #[error("resource operation exceeded its deadline")]
    DeadlineExceeded,
    #[error("resource read exceeds the configured limit: {requested} > {maximum}")]
    ResourceLimit { requested: usize, maximum: usize },
    #[error("resource I/O error: {0}")]
    Io(#[from] io::Error),
}

pub trait ResourceProvider: Send + Sync {
    fn list(
        &self,
        locator: &ResourceLocator,
        request: &ListRequest,
        ctx: &IoContext,
    ) -> Result<ResourcePage, ResourceError>;

    /// Metadata only: must never read payload or fall back to a GET request.
    fn probe(
        &self,
        locator: &ResourceLocator,
        ctx: &IoContext,
    ) -> Result<ResourceInfo, ResourceError>;

    fn open(
        &self,
        locator: &ResourceLocator,
        expected: Option<&ResourceVersion>,
        ctx: &IoContext,
    ) -> Result<Box<dyn ResourceReader>, ResourceError>;
}

pub trait ResourceReader: Send {
    fn info(&self) -> &ResourceInfo;
    fn read_next(&mut self, dst: &mut [u8], ctx: &IoContext) -> Result<usize, ResourceError>;
    fn read_at(
        &mut self,
        offset: u64,
        dst: &mut [u8],
        ctx: &IoContext,
    ) -> Result<usize, ResourceError>;
    fn close(&mut self, _ctx: &IoContext) -> Result<(), ResourceError>;
}

pub fn verify_expected_version(
    actual: &ResourceVersion,
    expected: Option<&ResourceVersion>,
) -> Result<(), ResourceError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected.size.is_some() && actual.size != expected.size {
        return Err(ResourceError::VersionChanged);
    }
    if let Some(expected_validator) = expected.validator.as_deref() {
        match actual.validator.as_deref() {
            Some(actual_validator) if actual_validator == expected_validator => {}
            Some(_) => return Err(ResourceError::VersionChanged),
            None => return Err(ResourceError::VersionUnverifiable),
        }
    }
    if expected.modified_ns.is_some() && actual.modified_ns != expected.modified_ns {
        return Err(ResourceError::VersionChanged);
    }
    Ok(())
}
