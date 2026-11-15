//! Immutable, credential-free descriptions. These are claims, not verification.
use crate::{ContentClaim, ContentRef, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > 255
                    || !value.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')
                    })
                {
                    return Err(Error::Invalid(
                        concat!("invalid ", stringify!($name)).into(),
                    ));
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self> {
                Self::new(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
identifier!(SourceId);
identifier!(LocationId);
identifier!(OpaqueAccessRef);
identifier!(OperationId);
identifier!(RequestKey);
identifier!(ReplicaId);

/// Adapter-owned public configuration only. Credentials belong to injected I/O
/// bindings; callers must not put credentials or signed URLs in this map.
pub type PublicBinding = BTreeMap<String, String>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationSpec {
    pub id: LocationId,
    pub adapter: String,
    pub storage_identity: String,
    pub binding: PublicBinding,
    pub credential_ref: Option<OpaqueAccessRef>,
}

/// Key relative to an injected storage binding. No headers, URL credentials,
/// query string, or transient access capability belongs here. Revision is an
/// optional immutable backend version, never a CAS source version history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocator {
    pub key: String,
    pub revision: Option<String>,
}

impl SourceLocator {
    pub fn validate(&self) -> Result<()> {
        if self.key.is_empty()
            || self.key.len() > 8192
            || self.key.contains('\0')
            || self.key.contains('?')
            || self.key.contains('#')
            || self.key.contains("://")
            || self
                .revision
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 1024 || v.contains('\0'))
        {
            return Err(Error::Invalid(
                "invalid public source locator; inject access material separately".into(),
            ));
        }
        Ok(())
    }
}

impl LocationSpec {
    pub fn validate(&self) -> Result<()> {
        if self.storage_identity.is_empty() || self.storage_identity.len() > 1024 {
            return Err(Error::Invalid("invalid location description".into()));
        }
        validate_public_storage(&self.adapter, &self.binding)
    }
}

pub(crate) fn validate_public_storage(adapter: &str, binding: &PublicBinding) -> Result<()> {
    if adapter.is_empty()
        || adapter.len() > 64
        || !adapter
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        || binding.len() > 32
    {
        return Err(Error::Invalid("invalid public storage description".into()));
    }
    // Metadata is public by contract. Reject common accidental capability
    // injection without claiming a generic secret detector or ACL system.
    for (key, value) in binding {
        let lower = key.to_ascii_lowercase();
        if key.len() > 128
            || value.len() > 8192
            || [
                "secret",
                "token",
                "password",
                "authorization",
                "credential",
                "header",
            ]
            .iter()
            .any(|part| lower.contains(part))
            || value.contains('?')
            || value.contains('#')
            || value.contains('\0')
            || value.contains('@')
        {
            return Err(Error::Invalid(
                "location binding must contain only public configuration".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub source_id: SourceId,
    pub location: LocationId,
    pub locator: SourceLocator,
    pub expected: ContentClaim,
    pub access_ref: Option<OpaqueAccessRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Registered,
    Verified,
    Missing,
    Unavailable,
    Invalid,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecord {
    pub spec: SourceSpec,
    pub bound: Option<ContentRef>,
    pub state: SourceState,
}
