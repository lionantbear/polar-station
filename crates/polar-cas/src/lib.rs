//! Content identity and backend-neutral object storage contracts.
pub mod resource;
pub mod archive;
pub mod backup;
pub mod backup_v2;
pub mod canonical;
pub mod catalog;
pub mod ingest;
pub mod iroh;
pub mod locked_directory;
pub mod manifest;
pub mod manifest_copy;
pub mod native_fs;
pub mod oci;
pub mod operation;
pub mod pack;
pub mod ports;
pub mod projection;
pub mod publish;
pub mod shared;
pub mod source;
pub mod store_index;
pub mod store_link;
pub mod verification;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
mod object;
pub use object::{ContentClaim, ContentRef, DigestAlgorithm, ObjectHasher, ObjectId};
use std::{path::Path, pin::Pin};
use tokio::io::AsyncRead;

pub type Reader = Pin<Box<dyn AsyncRead + Send + Unpin>>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid CAS input: {0}")]
    Invalid(String),
    #[error("CAS object not found")]
    NotFound,
    #[error("CAS object integrity mismatch")]
    Integrity,
    #[error("CAS input exceeds configured size limit")]
    TooLarge,
    #[error("CAS IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("CAS backend unavailable: {0}")]
    Backend(String),
    #[error("CAS immutable metadata conflict")]
    Conflict,
    #[error("CAS source violates its original content claim")]
    ClaimMismatch,
    #[error("CAS source violates its immutable content binding")]
    SourceContentMismatch,
    #[error("CAS source is invalid; explicit verification is required")]
    SourceInvalid,
    #[error("CAS operation is already in progress")]
    InProgress,
    #[error("CAS operation outcome is uncertain; explicit recovery is required")]
    OutcomeUncertain,
    #[error("CAS required digest has not been verified")]
    MissingDigest,
    #[error("CAS required local content is unavailable")]
    MissingLocal,
    #[error("CAS required capability is unavailable")]
    UnsupportedCapability,
    #[error("CAS input or access material is unavailable")]
    AccessMaterialUnavailable,
    #[error("CAS operation previously failed ({0:?}); retry requires a new request key")]
    OperationFailed(operation::FailureCode),
}

/// A storage isolation name; possession is not authorization.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CasNamespace(String);
impl CasNamespace {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::Invalid("invalid namespace".into()));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for CasNamespace {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}
impl From<CasNamespace> for String {
    fn from(value: CasNamespace) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub namespace: CasNamespace,
    #[serde(
        deserialize_with = "deserialize_legacy_id",
        serialize_with = "serialize_legacy_id"
    )]
    pub object_id: ObjectId,
    pub size_bytes: u64,
}
fn deserialize_legacy_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ObjectId, D::Error> {
    let id = ObjectId::deserialize(deserializer)?;
    id.require_sha256().map_err(serde::de::Error::custom)?;
    Ok(id)
}

fn serialize_legacy_id<S: serde::Serializer>(
    id: &ObjectId,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    id.require_sha256().map_err(serde::ser::Error::custom)?;
    id.serialize(serializer)
}

impl ObjectRef {
    pub fn locator(&self) -> String {
        format!(
            "cas://{}/{}/{}/{}",
            self.namespace.as_str(),
            self.object_id.algorithm().as_str(),
            self.object_id.hex(),
            self.size_bytes
        )
    }

    pub fn from_locator(locator: &str) -> Result<Self> {
        let parts = locator
            .strip_prefix("cas://")
            .ok_or_else(|| Error::Invalid("expected CAS reference".into()))?
            .splitn(5, '/')
            .collect::<Vec<_>>();
        if parts.len() != 4 || parts[1] != "sha256" {
            return Err(Error::Invalid("invalid CAS reference".into()));
        }
        let object = Self {
            namespace: CasNamespace::new(parts[0])?,
            object_id: ObjectId::try_from(format!("sha256:{}", parts[2]))?,
            size_bytes: parts[3]
                .parse()
                .map_err(|_| Error::Invalid("invalid CAS length".into()))?,
        };
        if object.locator() != locator {
            return Err(Error::Invalid("noncanonical CAS reference".into()));
        }
        Ok(object)
    }

    /// Namespace-scoped layout used by existing host bindings until shared
    /// space admission and reference authorization are connected.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace.as_str(), self.content_key())
    }
    /// Physical content key within an explicitly shared storage space.
    pub fn content_key(&self) -> String {
        self.object_id.content_key()
    }
}

/// Select once when binding a backend; never probe/fallback between layouts.
/// SharedContent is a physical optimization, not an authorization mechanism.
#[derive(Debug, Clone, Copy)]
pub enum ObjectKeyLayout {
    NamespaceScoped,
    SharedContent,
}
impl ObjectKeyLayout {
    pub fn key(self, object: &ObjectRef) -> String {
        match self {
            Self::NamespaceScoped => object.key(),
            Self::SharedContent => object.content_key(),
        }
    }
}

/// Lower-layer port. Only the CAS coordinator publishes verified, exclusively
/// owned staging files here; this is not an ingress for user-supplied paths.
/// publish must not overwrite an existing object and must satisfy its backend's
/// durable completion contract before returning.
#[async_trait]
pub trait CasBackend: Send + Sync {
    async fn publish(&self, object: &ObjectRef, staged_file: &Path) -> Result<()>;
    async fn open(&self, object: &ObjectRef) -> Result<Reader>;
    /// Existence and length, NOT a claim that content has been checksum-verified.
    async fn stat(&self, object: &ObjectRef) -> Result<Option<u64>>;
}

#[async_trait]
pub trait CasStore: Send + Sync {
    /// Explicit binding; needed to validate restore before any write.
    fn namespace(&self) -> &CasNamespace;
    async fn put(&self, input: Reader) -> Result<ObjectRef>;
    async fn put_expected(&self, expected: ObjectId, input: Reader) -> Result<ObjectRef>;
    /// Integrity verification completes only on a successful EOF.
    async fn open(&self, object: &ObjectRef) -> Result<Reader>;
    async fn stat(&self, object: &ObjectRef) -> Result<Option<u64>>;
}



pub mod hybrid_fs;

/// Host-local workspace backing identities; not Catalog business state.
pub mod workspace;

pub mod source_hint;
