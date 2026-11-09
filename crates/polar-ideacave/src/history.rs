//! Explicit immutable lineage. State equality does not collapse occurrences.
//! This module performs no I/O and does not grant content admission or head authority.
use crate::{invalid, itree::strict_value, reference::valid_key, Limits, NodeRef, Result};
use polar_cas::{canonical::canonical_json_bytes, ContentRef, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

pub const REVISION_SCHEMA: &str = "polar.ideacave.revision.v1";
pub const MAX_PARENTS: usize = 16;

/// A scope/branch selection, not a guess based on timestamp or digest order.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CurrentVersion {
    UnknownObject,
    Unversioned,
    Selected { version: crate::search::HistoryEntry },
    Conflicted { versions: Vec<crate::search::HistoryEntry> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionReceipt {
    pub subject: Uuid,
    pub branch: String,
    pub revision: ContentRef,
    pub previous: Option<ContentRef>,
    pub state: RevisionState,
    pub height: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RevisionState {
    Content { content: ContentRef },
    Entity { entity: NodeRef },
}
impl RevisionState {
    pub fn content(&self) -> &ContentRef {
        match self {
            Self::Content { content } => content,
            Self::Entity { entity } => &entity.document,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.content().id.require_sha256()?;
        if self.content().size_bytes > i64::MAX as u64 {
            return Err(invalid("revision.state", "content size"));
        }
        if let Self::Entity { entity } = self {
            if !valid_key(&entity.node) {
                return Err(invalid("revision.state", "entity selector"));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionDraft {
    pub subject: Uuid,
    pub state: RevisionState,
    /// Primary parent first. No timestamp-derived edges and no implicit latest.
    pub parents: Vec<ContentRef>,
    /// Owner operation identity; deliberately not the state hash.
    pub occurrence: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    schema: String,
    draft: RevisionDraft,
    height: u64,
}
#[derive(Clone, Debug)]
pub struct VerifiedRevision {
    content: ContentRef,
    revision: Revision,
}
impl VerifiedRevision {
    pub fn content(&self) -> &ContentRef {
        &self.content
    }
    pub fn revision(&self) -> &Revision {
        &self.revision
    }
}
impl Revision {
    /// Parents must have been decoded from their exact bytes. The runtime also
    /// checks their CAS admission and that their ancestor chain was accepted.
    pub fn new(draft: RevisionDraft, parents: &[VerifiedRevision]) -> Result<Self> {
        if draft.parents.len() != parents.len() {
            return Err(invalid("revision", "missing parents"));
        }
        let mut height = 0;
        for (expected, actual) in draft.parents.iter().zip(parents) {
            if expected != actual.content() || draft.subject != actual.revision().subject() {
                return Err(invalid("revision", "parent identity/subject mismatch"));
            }
            height = height.max(
                actual
                    .revision
                    .height
                    .checked_add(1)
                    .ok_or_else(|| invalid("revision", "history height overflow"))?,
            );
        }
        let result = Self {
            schema: REVISION_SCHEMA.into(),
            draft,
            height,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn subject(&self) -> Uuid {
        self.draft.subject
    }
    pub fn state(&self) -> &RevisionState {
        &self.draft.state
    }
    pub fn parents(&self) -> &[ContentRef] {
        &self.draft.parents
    }
    pub fn occurrence(&self) -> &str {
        &self.draft.occurrence
    }
    pub fn height(&self) -> u64 {
        self.height
    }
    pub fn validate_parents(&self, parents: &[VerifiedRevision]) -> Result<()> {
        let checked = Self::new(self.draft.clone(), parents)?;
        if checked != *self {
            return Err(invalid("revision", "invalid height/parents"));
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        if self.schema != REVISION_SCHEMA
            || self.draft.subject.is_nil()
            || self.draft.parents.len() > MAX_PARENTS
            || self.height > i64::MAX as u64
            || self.draft.parents.is_empty() != (self.height == 0)
            || self.draft.occurrence.is_empty()
            || self.draft.occurrence.len() > 160
            || !self
                .draft
                .occurrence
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            return Err(invalid("revision", "invalid revision envelope"));
        }
        self.draft.state.validate()?;
        let mut unique = BTreeSet::new();
        for p in &self.draft.parents {
            p.id.require_sha256()?;
            if p.size_bytes == 0
                || p.size_bytes > i64::MAX as u64
                || !unique.insert(p.id.to_string())
            {
                return Err(invalid("revision", "invalid/duplicate parent"));
            }
        }
        Ok(())
    }
    pub fn canonical_bytes(&self, limits: &Limits) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = canonical_json_bytes(self)?;
        if bytes.len() > limits.max_bytes {
            return Err(crate::Error::Limit("revision bytes"));
        }
        Ok(bytes)
    }
    pub fn decode_cas(
        bytes: &[u8],
        expected: &ContentRef,
        limits: &Limits,
    ) -> Result<VerifiedRevision> {
        if bytes.len() as u64 != expected.size_bytes || ObjectId::for_bytes(bytes) != expected.id {
            return Err(crate::Error::IdentityMismatch);
        }
        let revision: Self = serde_json::from_value(strict_value(bytes, limits)?)?;
        if revision.canonical_bytes(limits)? != bytes {
            return Err(crate::Error::NonCanonical);
        }
        Ok(VerifiedRevision {
            content: expected.clone(),
            revision,
        })
    }
}
