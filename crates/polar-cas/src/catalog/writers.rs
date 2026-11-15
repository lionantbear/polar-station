//! Explicit successful-save facts. Identity is bound by the owning runtime,
//! not inferred from a Store creator, filesystem owner, namespace or reader.
use crate::{source::LocationId, store_index::StoreRef, ContentRef, Error, ObjectId, Result};
use serde::{Deserialize, Serialize};

/// Stable canonical Polar Node ID text; no identity runtime enters generic CAS.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WriterId(String);
impl WriterId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("writer requires a canonical Node ID".into()));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for WriterId {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}
impl From<WriterId> for String {
    fn from(value: WriterId) -> Self {
        value.0
    }
}

/// Frozen at operation preparation, including the original visibility scope.
/// A configured legacy LocationId does not imply a fabricated Store definition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveAttribution {
    pub writer: WriterId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ObjectId>,
}
impl SaveAttribution {
    pub fn validate(&self) -> Result<()> {
        if let Some(store) = &self.store {
            store.validate()?;
        }
        if let Some(scope) = &self.scope {
            scope.require_sha256()?;
        }
        Ok(())
    }
}

/// Not an ownership grant, author claim, operation log or physical availability
/// proof. Multiple writers and destinations can refer to identical content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriterRecord {
    pub content: ContentRef,
    pub attribution: SaveAttribution,
    pub destination: LocationId,
    pub first_saved_at_ms: u64,
}
impl WriterRecord {
    pub fn validate(&self) -> Result<()> {
        self.attribution.validate()?;
        if self.content.size_bytes > i64::MAX as u64 || self.first_saved_at_ms > i64::MAX as u64 {
            return Err(Error::Invalid("writer fact exceeds catalog bounds".into()));
        }
        Ok(())
    }
    /// Association identity, independent of completion time and content bytes.
    /// When a real Store is bound, aliases of its runtime location deduplicate.
    pub fn association_key(&self) -> String {
        let target = self
            .attribution
            .store
            .as_ref()
            .map(|s| format!("store:{}", s.key()))
            .unwrap_or_else(|| format!("location:{}", self.destination.as_str()));
        format!(
            "{}/{}/{}",
            self.attribution.writer.as_str(),
            self.attribution
                .scope
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            target
        )
    }
    pub fn visible_in(&self, scope: Option<&ObjectId>) -> bool {
        self.attribution.scope.is_none() || self.attribution.scope.as_ref() == scope
    }
}


