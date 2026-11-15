//! Stable Store routing links. These are identities, never endpoints or grants.
//! The wire carries the canonical Polar NodeId text; this backend-neutral crate
//! does not derive identities or depend on a Polar runtime/credential provider.
use crate::{Error, ObjectId, ObjectRef, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use uuid::Uuid;

pub const STORE_LINK_BYTES: usize = 109;
pub const CAS_LINK_BYTES: usize = 179;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StoreLink {
    creator_node_id: String,
    uuid: Uuid,
}

impl StoreLink {
    pub fn new(creator_node_id: &str, uuid: Uuid) -> Result<Self> {
        validate_creator_node_id(creator_node_id)?;
        if uuid.is_nil() {
            return Err(Error::Invalid("Store UUID must not be nil".into()));
        }
        Ok(Self {
            creator_node_id: creator_node_id.into(),
            uuid,
        })
    }
    pub fn creator_node_id(&self) -> &str {
        &self.creator_node_id
    }
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }
    /// Canonical root shared by the creator and every authorized consumer.
    pub fn new_store_root(&self) -> String {
        format!("polar/{}/{}/", self.creator_node_id, self.uuid)
    }
    pub fn content_link(&self, content: &ObjectId) -> Result<CasLink> {
        content.require_sha256()?;
        Ok(CasLink {
            store: self.clone(),
            content: content.clone(),
        })
    }
}
pub(crate) fn validate_creator_node_id(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid(
            "creator NodeId requires 64 lowercase hex characters".into(),
        ));
    }
    Ok(())
}
impl fmt::Display for StoreLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "store://{}/{}", self.creator_node_id, self.uuid)
    }
}
impl FromStr for StoreLink {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() > STORE_LINK_BYTES {
            return Err(Error::TooLarge);
        }
        let (node, uuid) = value
            .strip_prefix("store://")
            .and_then(|v| v.split_once('/'))
            .ok_or_else(|| Error::Invalid("expected Store link".into()))?;
        let uuid =
            Uuid::parse_str(uuid).map_err(|_| Error::Invalid("invalid Store UUID".into()))?;
        let link = Self::new(node, uuid)?;
        if link.to_string() != value {
            return Err(Error::Invalid("noncanonical Store link".into()));
        }
        Ok(link)
    }
}
impl TryFrom<String> for StoreLink {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}
impl From<StoreLink> for String {
    fn from(value: StoreLink) -> String {
        value.to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CasLink {
    store: StoreLink,
    content: ObjectId,
}
impl CasLink {
    pub fn store(&self) -> &StoreLink {
        &self.store
    }
    pub fn content(&self) -> &ObjectId {
        &self.content
    }
}
impl fmt::Display for CasLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cas://{}/{}/sha256/{}",
            self.store.creator_node_id,
            self.store.uuid,
            self.content.hex()
        )
    }
}
impl FromStr for CasLink {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() > CAS_LINK_BYTES {
            return Err(Error::TooLarge);
        }
        let (store, hash) = value
            .strip_prefix("cas://")
            .and_then(|v| v.split_once("/sha256/"))
            .ok_or_else(|| Error::Invalid("expected CAS Store link".into()))?;
        let store: StoreLink = format!("store://{store}").parse()?;
        let content = ObjectId::try_from(format!("sha256:{hash}"))?;
        let link = store.content_link(&content)?;
        if link.to_string() != value {
            return Err(Error::Invalid("noncanonical CAS Store link".into()));
        }
        Ok(link)
    }
}
impl TryFrom<String> for CasLink {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}
impl From<CasLink> for String {
    fn from(value: CasLink) -> String {
        value.to_string()
    }
}

/// Explicit grammar selection keeps namespace ObjectRefs separate from Store links.
/// A namespace ObjectRef is not an incomplete Store definition or creator fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CasLocator {
    Store(CasLink),
    Namespace(ObjectRef),
}
impl FromStr for CasLocator {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let path = value
            .strip_prefix("cas://")
            .ok_or_else(|| Error::Invalid("expected CAS link".into()))?;
        if path.split('/').nth(1) == Some("sha256") {
            ObjectRef::from_locator(value).map(Self::Namespace)
        } else {
            value.parse().map(Self::Store)
        }
    }
}
