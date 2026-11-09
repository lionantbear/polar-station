//! Bounded location claims exchanged by domains. A claim never grants I/O or
//! proves admission. Persisted scopes are opaque; the caller establishes access.
use crate::{
    canonical::canonical_json_bytes, source::SourceLocator, store_index::StoreRef, ContentRef,
    Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};

pub const MAX_SOURCE_HINTS: usize = 32;
pub const MAX_HINT_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintAuthentication {
    /// No material is needed, but Host network policy still applies.
    Anonymous,
    #[default]
    Bound,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceHint {
    Store {
        store: StoreRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locator: Option<SourceLocator>,
        #[serde(default)]
        authentication: HintAuthentication,
    },
    Url {
        url: String,
    },
}
impl SourceHint {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Store { store, locator, .. } => {
                store.validate()?;
                if let Some(l) = locator {
                    l.validate()?;
                    if l.key.starts_with('/')
                        || l.key.contains('\\')
                        || l.key.split('/').any(|s| matches!(s, "" | "." | ".."))
                    {
                        return Err(Error::Invalid("invalid relative hint key".into()));
                    }
                }
            }
            Self::Url { url } => {
                public_https_parts(url)?;
            }
        }
        if canonical_json_bytes(self)
            .map_err(|_| Error::Invalid("hint encoding".into()))?
            .len()
            > MAX_HINT_BYTES
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    pub fn id(&self) -> Result<ObjectId> {
        self.validate()?;
        Ok(ObjectId::for_bytes(
            &canonical_json_bytes(self).map_err(|_| Error::Invalid("hint encoding".into()))?,
        ))
    }
}

/// This v1 deliberately excludes URL userinfo, fragments and query strings
/// (including presigned credentials). Direct ordinary URLs need no fake Store.
/// The transport independently checks DNS/IP reachability and pins the origin.
pub fn public_https_parts(url: &str) -> Result<(String, String)> {
    if url.len() > 4096
        || url.bytes().any(|b| b <= 0x20 || b == 0x7f)
        || url.contains(['@', '#', '?', '\\'])
    {
        return Err(Error::Invalid("credential-free HTTPS URL required".into()));
    }
    let uri: http::Uri = url
        .parse()
        .map_err(|_| Error::Invalid("invalid HTTPS URL".into()))?;
    if uri.scheme_str() != Some("https") || uri.host().is_none() {
        return Err(Error::Invalid("HTTPS URL required".into()));
    }
    let authority = uri
        .authority()
        .ok_or_else(|| Error::Invalid("HTTPS origin missing".into()))?;
    Ok((
        format!("https://{authority}"),
        uri.path().trim_start_matches('/').to_string(),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintOutcome {
    Candidate,
    Verified,
    Missing,
    Denied,
    Failed,
    Integrity,
}
impl HintOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Verified => "verified",
            Self::Missing => "missing",
            Self::Denied => "denied",
            Self::Failed => "failed",
            Self::Integrity => "integrity",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RememberedHint {
    pub hint: SourceHint,
    pub last_outcome: String,
    pub last_attempt_ms: u64,
    pub last_verified_ms: Option<u64>,
}
/// Scope derivation takes unambiguous, structured domain components. It contains
/// no auth token and does not itself grant access. Never accept it from a message.
pub fn hint_scope(parts: &[&str]) -> Result<ObjectId> {
    if parts.is_empty() || parts.len() > 8 || parts.iter().any(|p| p.len() > 4096) {
        return Err(Error::Invalid("invalid source scope".into()));
    }
    Ok(ObjectId::for_bytes(
        &canonical_json_bytes(&("source-scope-v1", parts))
            .map_err(|_| Error::Invalid("scope encoding".into()))?,
    ))
}
pub fn validate_hint_request(
    scope: &ObjectId,
    content: &ContentRef,
    hints: &[SourceHint],
) -> Result<()> {
    scope.require_sha256()?;
    content.id.require_sha256()?;
    if content.size_bytes > i64::MAX as u64 || hints.len() > MAX_SOURCE_HINTS {
        return Err(Error::TooLarge);
    }
    for hint in hints {
        hint.validate()?;
    }
    Ok(())
}
