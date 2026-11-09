//! Portable physical-location facts. An index is neither admission nor access authority.
//! Existing PCASIDX/PtrHash stores remain unchanged; this envelope carries facts
//! between heterogeneous stores and is reconstructed locally before installation.
use crate::{canonical::canonical_json_bytes, source::SourceLocator, ContentRef, Error, ObjectId, Result};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct StoreIndexCheckpoint {
    pub root: ContentRef,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreRef {
    pub uuid: Uuid,
    pub definition: ContentRef,
    /// Persistent identity of the Polar that created this Store. This is origin
    /// metadata, never a current writer, index-maintainer identity or access grant.
    #[serde(deserialize_with = "deserialize_creator_node_id")]
    pub creator_node_id: String,
}
fn deserialize_creator_node_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    let node = String::deserialize(deserializer)?;
    crate::store_link::validate_creator_node_id(&node).map_err(serde::de::Error::custom)?;
    Ok(node)
}
impl StoreRef {
    pub fn key(&self) -> String {
        format!("{}/{}@{}", self.creator_node_id, self.uuid, self.definition.id)
    }
    pub fn link(&self) -> Result<crate::store_link::StoreLink> {
        self.validate()?;
        crate::store_link::StoreLink::new(&self.creator_node_id, self.uuid)
    }
    pub fn validate(&self) -> Result<()> {
        if self.uuid.is_nil() || self.definition.size_bytes == 0 { return Err(Error::Invalid("invalid StoreRef".into())); }
        crate::store_link::validate_creator_node_id(&self.creator_node_id)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Representation {
    Blob,
    /// locator names the complete immutable image, NOT a guessed byte offset.
    /// A trusted native reader must interpret and verify member bytes.
    FilesystemErofsMember { image: ContentRef, tree: ContentRef, path: String },
    /// Existing digest-path CAS pack. Lookup uses the existing pack reader.
    DigestPackMember { image: ContentRef },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreEntry {
    pub content: ContentRef,
    pub locator: SourceLocator,
    pub representation: Representation,
}
impl StoreEntry {
    pub fn blob(content: ContentRef, locator: SourceLocator) -> Result<Self> {
        let value = Self { content, locator, representation: Representation::Blob };
        value.validate()?;
        Ok(value)
    }
    /// A physical slot, independent of the claim about its content. Two different
    /// content IDs may NOT quietly occupy the same blob key or pack member.
    pub fn slot(&self) -> Result<Vec<u8>> {
        let member = match &self.representation {
            Representation::Blob => ("blob", String::new()),
            Representation::FilesystemErofsMember { path, .. } => ("fs-erofs", path.clone()),
            Representation::DigestPackMember { .. } => ("digest-pack", self.content.id.to_string()),
        };
        canonical_json_bytes(&(&self.locator, member))
            .map_err(|_| Error::Invalid("index slot encoding".into()))
    }
    pub fn validate(&self) -> Result<()> {
        self.locator.validate()?;
        if self.locator.key.starts_with('/') || self.locator.key.contains('\\')
            || self.locator.key.split('/').any(|p| p.is_empty() || matches!(p, "." | ".."))
            || self.content.size_bytes > i64::MAX as u64 {
            return Err(Error::Invalid("invalid physical index entry".into()));
        }
        match &self.representation {
            Representation::Blob => {},
            Representation::DigestPackMember { image } => {
                image.id.require_sha256()?;
                if image.size_bytes < 4096 || image.size_bytes % 4096 != 0 { return Err(Error::Integrity); }
            }
            Representation::FilesystemErofsMember { image, tree, path } => {
                image.id.require_sha256()?;
                if image.size_bytes < 4096 || image.size_bytes % 4096 != 0 || tree.size_bytes == 0
                    || path.is_empty() || path.len() > 4096 || path.starts_with('/') || path.contains(['\\', '\0'])
                    || path.split('/').any(|p| p.is_empty() || matches!(p, "." | "..")) {
                    return Err(Error::Invalid("invalid packed member".into()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IndexLimits {
    pub max_segment_bytes: usize,
    pub max_total_bytes: usize,
    pub max_entries: usize,
    pub max_deltas: usize,
    pub max_retries: usize,
}
impl Default for IndexLimits {
    fn default() -> Self { Self {
        max_segment_bytes: 32 << 20, max_total_bytes: 128 << 20,
        max_entries: 100_000, max_deltas: 32, max_retries: 8,
    } }
}
impl IndexLimits {
    pub fn validate(&self) -> Result<()> {
        if self.max_segment_bytes == 0 || self.max_segment_bytes > 128 << 20
            || self.max_total_bytes > 512 << 20 || self.max_total_bytes < self.max_segment_bytes
            || self.max_entries > 1_000_000 || self.max_entries == 0 || self.max_deltas == 0 || self.max_deltas > 256
            || self.max_retries == 0 || self.max_retries > 64 {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexBase {
    pub schema: String,
    pub store: StoreRef,
    pub generation: u64,
    pub entries: Vec<StoreEntry>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndexChange {
    Add { entry: StoreEntry },
    /// Exact old fact, not a hash-wide deletion. Concurrent replacements conflict.
    Remove { entry: StoreEntry },
}
impl IndexChange {
    pub fn entry(&self) -> &StoreEntry { match self { Self::Add { entry } | Self::Remove { entry } => entry } }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDelta {
    pub schema: String,
    pub store: StoreRef,
    pub from: u64,
    pub to: u64,
    pub changes: Vec<IndexChange>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRoot {
    pub schema: String,
    pub store: StoreRef,
    pub generation: u64,
    pub base: ContentRef,
    pub deltas: Vec<ContentRef>,
}
impl IndexRoot {
    pub fn validate(&self, expected: &StoreRef, limits: IndexLimits) -> Result<()> {
        limits.validate()?;
        self.store.validate()?;
        if self.schema != "polar.cas.store-index-root.v1" || &self.store != expected
            || self.deltas.len() > limits.max_deltas { return Err(Error::Invalid("index root binding/format".into())); }
        let mut seen = BTreeSet::new();
        let mut total = 0usize;
        for c in std::iter::once(&self.base).chain(&self.deltas) {
            if c.size_bytes == 0 || c.size_bytes > limits.max_segment_bytes as u64 || !seen.insert(c.id.to_string()) {
                return Err(Error::TooLarge);
            }
            total = total.checked_add(c.size_bytes as usize).ok_or(Error::TooLarge)?;
        }
        if total > limits.max_total_bytes { return Err(Error::TooLarge); }
        Ok(())
    }
}

/// A checked in-memory snapshot. Still only location claims, never a Ready replica.
#[derive(Clone, Debug)]
pub struct IndexSnapshot {
    store: StoreRef,
    generation: u64,
    entries: BTreeMap<Vec<u8>, StoreEntry>,
    by_content: BTreeMap<String, Vec<Vec<u8>>>,
}

/// Consumer-owned coverage policy. Binding a reader does not select an index.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndexSelection {
    #[default]
    None,
    Hashes { ids: Vec<ObjectId> },
    /// Explicitly select this supplied snapshot, not every live Store object.
    Snapshot,
}
impl IndexSnapshot {
    pub fn from_base(base: IndexBase, limits: IndexLimits) -> Result<Self> {
        limits.validate()?;
        base.store.validate()?;
        if base.schema != "polar.cas.store-index-base.v1" || base.entries.len() > limits.max_entries { return Err(Error::TooLarge); }
        let mut entries = BTreeMap::new();
        let mut previous = None;
        for entry in base.entries {
            entry.validate()?;
            let slot = entry.slot()?;
            if previous.as_ref().is_some_and(|p| p >= &slot) { return Err(Error::Invalid("unsorted or duplicate index slots".into())); }
            previous = Some(slot.clone());
            entries.insert(slot, entry);
        }
        let mut value = Self { store: base.store, generation: base.generation, entries, by_content: BTreeMap::new() };
        value.reindex()?;
        Ok(value)
    }
    pub fn empty(store: StoreRef) -> Result<Self> {
        Self::from_base(IndexBase { schema: "polar.cas.store-index-base.v1".into(), store, generation: 0, entries: vec![] }, IndexLimits::default())
    }
    pub fn store(&self) -> &StoreRef { &self.store }
    pub fn generation(&self) -> u64 { self.generation }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn entries(&self) -> impl Iterator<Item = &StoreEntry> { self.entries.values() }
    /// Exact digest metadata lookup. It does not establish a verified size or access grant.
    pub fn candidates_for_id<'a>(&'a self, id: &ObjectId) -> impl Iterator<Item = &'a StoreEntry> {
        self.by_content.get(&id.to_string()).into_iter().flatten()
            .map(|key| &self.entries[key])
    }

    pub fn select(&self, selection: &IndexSelection) -> Result<Self> {
        let selected: BTreeSet<String> = match selection {
            IndexSelection::Snapshot => return Ok(self.clone()),
            IndexSelection::None => BTreeSet::new(),
            IndexSelection::Hashes { ids } => {
                if ids.len() > 1_000_000 { return Err(Error::TooLarge); }
                let keys: BTreeSet<_> = ids.iter().map(ToString::to_string).collect();
                if keys.len() != ids.len() { return Err(Error::Invalid("duplicate selected hash".into())); }
                keys
            }
        };
        let entries = selected.iter().filter_map(|id| self.by_content.get(id)).flatten()
            .map(|slot| (slot.clone(), self.entries[slot].clone())).collect();
        let mut snapshot = Self { store: self.store.clone(), generation: self.generation,
            entries, by_content: BTreeMap::new() };
        snapshot.reindex()?;
        Ok(snapshot)
    }
    pub fn base(&self) -> IndexBase {
        IndexBase { schema: "polar.cas.store-index-base.v1".into(), store: self.store.clone(), generation: self.generation, entries: self.entries.values().cloned().collect() }
    }
    pub fn candidates<'a>(&'a self, content: &'a ContentRef) -> impl Iterator<Item = Result<&'a StoreEntry>> + 'a {
        self.by_content.get(&content.id.to_string()).into_iter().flatten().map(move |k| {
            let e = &self.entries[k];
            if e.content != *content { return Err(Error::ClaimMismatch); }
            Ok(e)
        })
    }
    pub fn lookup(&self, content: &ContentRef) -> Result<Vec<StoreEntry>> {
        self.candidates(content).map(|entry| entry.cloned()).collect()
    }
    pub fn apply(&self, delta: &IndexDelta, limits: IndexLimits) -> Result<Self> {
        limits.validate()?;
        if delta.schema != "polar.cas.store-index-delta.v1" || delta.store != self.store
            || delta.from != self.generation || delta.to != self.generation.checked_add(1).ok_or(Error::TooLarge)?
            || delta.changes.len() > limits.max_entries { return Err(Error::Conflict); }
        let mut next = self.clone();
        let mut touched = BTreeSet::new();
        for c in &delta.changes {
            c.entry().validate()?;
            let key = c.entry().slot()?;
            if !touched.insert(key.clone()) { return Err(Error::Invalid("repeated delta slot".into())); }
            match c {
                IndexChange::Add { entry } => {
                    if next.entries.get(&key).is_some_and(|old| old != entry) { return Err(Error::Conflict); }
                    next.entries.insert(key, entry.clone());
                }
                IndexChange::Remove { entry } => {
                    if next.entries.get(&key).is_some_and(|old| old != entry) { return Err(Error::Conflict); }
                    next.entries.remove(&key);
                }
            }
            if next.entries.len() > limits.max_entries { return Err(Error::TooLarge); }
        }
        next.generation = delta.to;
        next.reindex()?;
        Ok(next)
    }
    fn reindex(&mut self) -> Result<()> {
        self.by_content.clear();
        let mut sizes = BTreeMap::new();
        for (slot, entry) in &self.entries {
            let id = entry.content.id.to_string();
            if sizes.insert(id.clone(), entry.content.size_bytes).is_some_and(|s| s != entry.content.size_bytes) { return Err(Error::Integrity); }
            self.by_content.entry(id).or_default().push(slot.clone());
        }
        Ok(())
    }
}

pub fn encode<T: Serialize>(value: &T, max: usize) -> Result<(ContentRef, Vec<u8>)> {
    let bytes = canonical_json_bytes(value).map_err(|_| Error::Invalid("index encoding".into()))?;
    if bytes.len() > max { return Err(Error::TooLarge); }
    Ok((ContentRef { id: ObjectId::for_bytes(&bytes), size_bytes: bytes.len() as u64 }, bytes))
}
pub fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8], expected: Option<&ContentRef>, max: usize) -> Result<T> {
    if bytes.len() > max { return Err(Error::TooLarge); }
    if let Some(c) = expected {
        if c.size_bytes != bytes.len() as u64 || c.id != ObjectId::for_bytes(bytes) { return Err(Error::Integrity); }
    }
    let value: T = serde_json::from_slice(bytes).map_err(|_| Error::Invalid("index decoding".into()))?;
    // Deny unknown/repeated fields, noncanonical maps/UUIDs and alternate encodings.
    if canonical_json_bytes(&value).map_err(|_| Error::Integrity)? != bytes { return Err(Error::Integrity); }
    Ok(value)
}

/// The ETag is only a compare-and-exchange token, never a content hash.
#[derive(Clone, Debug)]
pub struct IndexHead { pub bytes: Vec<u8>, pub etag: String }
#[async_trait]
pub trait StoreIndexTransport: Send + Sync {
    async fn read_head(&self, max_bytes: usize) -> Result<Option<IndexHead>>;
    async fn read_segment(&self, content: &ContentRef, max_bytes: usize) -> Result<Vec<u8>>;
    async fn write_segment(&self, content: &ContentRef, bytes: &[u8]) -> Result<()>;
    /// Implementations MUST fail closed if conditional writes are unsupported.
    async fn compare_exchange_head(&self, expected_etag: Option<&str>, bytes: &[u8]) -> Result<()>;
}
