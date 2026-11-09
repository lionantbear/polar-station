//! Immutable 256-way value index snapshots, published into an injected CAS.
//! This module has no mutable head, Store selection policy, domain database,
//! background task, or automatic retry. The owner switches its head last.
use super::{
    navigation::identity,
    values::{decode_records, encode_records, LocalPosition, RecordView, ValueIndex, ValueRecord},
};
use crate::LocalCas;
use polar_cas::{canonical::canonical_json_bytes, ContentRef, Error, ObjectId, Result};
use serde::{Deserialize, Serialize};

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::Write,
    sync::Arc,
};
mod directory;
mod segments;
#[cfg(any(test, feature = "query-metrics"))]
#[doc(hidden)]
pub mod write_metrics;

#[derive(Clone, Debug)]
pub enum ValueChange {
    Put(ValueRecord),
    /// Explicit key format migration; the old exact key must still match.
    Rekey {
        previous_key: Vec<u8>,
        row: ValueRecord,
    },
    Remove {
        locator: ObjectId,
        key: Vec<u8>,
    },
}
impl ValueChange {
    fn locator(&self) -> &ObjectId {
        match self {
            Self::Put(r) => &r.locator,
            Self::Rekey { row, .. } => &row.locator,
            Self::Remove { locator, .. } => locator,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionRef {
    pub facts: ContentRef,
    pub native: ContentRef,
    pub platform: String,
    pub records: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<PartitionStorage>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionStorage {
    pub container: ContentRef,
    pub facts_offset: u64,
    pub native_offset: u64,
}
impl PartitionRef {
    /// Portable backup owns the complete immutable segment, not loose copies
    /// of the embedded partitions. Legacy roots still own standalone facts.
    pub fn portable_file(&self) -> &ContentRef {
        self.storage.as_ref().map_or(&self.facts, |s| &s.container)
    }
    fn open_native(
        &self,
        store: &LocalCas,
        mapped: &Arc<memmap2::Mmap>,
    ) -> Result<Option<ValueIndex>> {
        match &self.storage {
            None => store
                .open_shared_file(&self.native)?
                .map(|f| ValueIndex::open(&f))
                .transpose(),
            Some(storage) => {
                if storage
                    .native_offset
                    .checked_add(self.native.size_bytes)
                    .ok_or(Error::Integrity)?
                    > storage.container.size_bytes
                {
                    return Err(Error::Integrity);
                }
                ValueIndex::open_mapped(
                    mapped.clone(),
                    storage.native_offset,
                    self.native.size_bytes,
                )
                .map(Some)
            }
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Root {
    schema: String,
    pub(crate) partitions: BTreeMap<u8, PartitionRef>,
    pub(crate) metadata: serde_json::Value,
}
impl Root {
    pub(crate) fn empty(metadata: serde_json::Value) -> Self {
        Self {
            schema: "polar.cas.value-index-root.v1".into(),
            partitions: BTreeMap::new(),
            metadata,
        }
    }
    /// Read the directory from the handle already held by ordinary open.
    fn open(file: &std::fs::File, reference: &ContentRef) -> Result<Self> {
        use std::io::Read;
        if reference.size_bytes > 4 << 20 {
            return Err(Error::TooLarge);
        }
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let mut bytes = vec![0; reference.size_bytes as usize];
        let mut reader = file;
        reader.read_exact(&mut bytes)?;
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("root_read", started.elapsed(), bytes.len() as u64, 0);
        Self::decode_timed(&bytes)
    }
    /// Read only the bounded immutable root, never its partitions or records.
    pub(crate) fn read(store: &LocalCas, reference: &ContentRef) -> Result<Self> {
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let bytes = store.read_shared_bounded(reference, 4 << 20)?;
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("root_read", started.elapsed(), bytes.len() as u64, 0);
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        if ObjectId::for_bytes(&bytes) != reference.id {
            return Err(Error::Integrity);
        }
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("root_hash", started.elapsed(), bytes.len() as u64, 0);
        Self::decode_timed(&bytes)
    }
    fn decode_timed(bytes: &[u8]) -> Result<Self> {
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let root = Self::decode(bytes)?;
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("root_decode", started.elapsed(), 0, root.partitions.len() as u64);
        Ok(root)
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        directory::decode(bytes)
    }
    /// Preserve the immutable partition references when changing root metadata.
    pub(crate) fn publish(&self, store: &LocalCas) -> Result<ContentRef> {
        if !matches!(store.key_layout, polar_cas::ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        let bytes = directory::encode(self)?;
        publish_bytes(store, &bytes)
    }
}
struct Partition {
    reference: PartitionRef,
    index: PartitionIndex,
    // Immutable inode pin, including portable records used by later updates and
    // backups. A live snapshot never reopens a path removed by maintenance.
    facts: Arc<memmap2::Mmap>,
}

impl Partition {
    fn read_records(&self) -> Result<Vec<ValueRecord>> {
        
        decode_records(facts_slice(&self.reference, &self.facts)?)
    }
}
fn facts_slice<'a>(reference: &PartitionRef, bytes: &'a [u8]) -> Result<&'a [u8]> {
    let offset = reference.storage.as_ref().map_or(0, |s| s.facts_offset);
    let end = offset
        .checked_add(reference.facts.size_bytes)
        .ok_or(Error::Integrity)?;
    let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
    let end = usize::try_from(end).map_err(|_| Error::TooLarge)?;
    bytes.get(start..end).ok_or(Error::Integrity)
}
fn pin_facts(store: &LocalCas, reference: &PartitionRef) -> Result<Arc<memmap2::Mmap>> {
    let file = store
        .open_shared_file(reference.portable_file())?
        .ok_or(Error::NotFound)?;
    // SAFETY: only immutable CAS publications enter this snapshot.
    let map = unsafe { memmap2::Mmap::map(&file)? };
    Ok(Arc::new(map))
}
fn verify_facts(reference: &PartitionRef, bytes: &[u8]) -> Result<()> {
    let facts = facts_slice(reference, bytes)?;
    
    if ObjectId::for_bytes(facts) != reference.facts.id {
        return Err(Error::Integrity);
    }
    Ok(())
}
const SORTED_PLATFORM: &str = "pcasrec1-sorted-v1";
type ScopedExtensionDecoder = for<'a> fn(&'a [u8], &[u8; 32]) -> Result<Option<&'a [u8]>>;
pub(crate) type LocalDecoder =
    fn(&ObjectId, &[u8], &[u8]) -> Result<Option<polar_cas::shared::LocatedContent>>;
const SMALL_PARTITION_RECORDS: usize = 32;

enum PartitionIndex {
    Perfect(Arc<ValueIndex>),
    Sorted(Vec<ValueRecord>),
}
impl PartitionIndex {
    fn row(&self, locator: &ObjectId) -> Result<Option<RecordView<'_>>> {
        match self {
            Self::Perfect(index) => index.row(locator),
            Self::Sorted(rows) => Ok(rows
                .binary_search_by_key(&identity(locator), |r| identity(&r.locator))
                .ok()
                .map(|i| {
                    (
                        Cow::Borrowed(rows[i].key.as_slice()),
                        Cow::Borrowed(rows[i].value.as_slice()),
                    )
                })),
        }
    }
    fn get(&self, locator: &ObjectId, key: &[u8]) -> Result<Option<Cow<'_, [u8]>>> {
        match self {
            Self::Perfect(index) => index.get(locator, key),
            Self::Sorted(rows) => Ok(rows
                .binary_search_by_key(&identity(locator), |r| identity(&r.locator))
                .ok()
                .and_then(|i| {
                    (rows[i].key == key).then_some(Cow::Borrowed(rows[i].value.as_slice()))
                })),
        }
    }
}
#[derive(Clone)]
pub struct ValueSnapshot {
    _root_lease: Arc<std::fs::File>,
    // Portable imports keep their source identity until activation. This second
    // immutable root protects the newly built native ranges during preparation.
    _rebuilt_lease: Option<Arc<std::fs::File>>,
    reference: ContentRef,
    root: Root,
    partitions: BTreeMap<u8, Arc<Partition>>,
    dispatch: [Option<Arc<Partition>>; 256],
    native_dispatch: [Option<Arc<ValueIndex>>; 256],
    store: LocalCas,
}
fn platform() -> String {
    format!(
        "pcasval4-ptrhash2.1.2-epserde0.13.1-{}-{}-{}",
        std::env::consts::ARCH,
        usize::BITS,
        if cfg!(target_endian = "little") {
            "le"
        } else {
            "be"
        }
    )
}
fn publish_bytes(store: &LocalCas, bytes: &[u8]) -> Result<ContentRef> {
    let reference = ContentRef {
        id: ObjectId::for_bytes(bytes),
        size_bytes: bytes.len() as u64,
    };
    let mut tmp = store.index_temporary()?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    store.publish_index_artifact(&reference, tmp.path())?;
    Ok(reference)
}

impl ValueSnapshot {
    pub fn reference(&self) -> &ContentRef {
        &self.reference
    }
    pub fn metadata(&self) -> &serde_json::Value {
        &self.root.metadata
    }
    pub fn partition_refs(&self) -> &BTreeMap<u8, PartitionRef> {
        &self.root.partitions
    }
    /// Explicit rebuild/import enumeration; never used by point lookup.
    pub fn visit_records(&self, mut visit: impl FnMut(ValueRecord) -> Result<()>) -> Result<()> {
        for p in self.partitions.values() {
            for row in p.read_records()? {
                visit(row)?;
            }
        }
        Ok(())
    }
    /// Restore compares logical facts, independent of native slot ordering,
    /// platform or the portable envelope used by an older writer.
    pub(crate) fn same_records(&self, other: &Self) -> Result<bool> {
        if self.partitions.len() != other.partitions.len() {
            return Ok(false);
        }
        for (prefix, a) in &self.partitions {
            let Some(b) = other.partitions.get(prefix) else {
                return Ok(false);
            };
            if a.reference.records != b.reference.records {
                return Ok(false);
            }
            if a.reference.facts != b.reference.facts && a.read_records()? != b.read_records()? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn get(&self, locator: &ObjectId, key: &[u8]) -> Result<Option<Cow<'_, [u8]>>> {
        match &self.dispatch[locator.digest_bytes()[0] as usize] {
            Some(p) => p.index.get(locator, key),
            None => Ok(None),
        }
    }
    /// Callers must validate the returned full key before interpreting a value.
    pub fn row(&self, locator: &ObjectId) -> Result<Option<RecordView<'_>>> {
        self.dispatch[locator.digest_bytes()[0] as usize]
            .as_ref()
            .map(|p| p.index.row(locator))
            .transpose()
            .map(Option::flatten)
    }
    pub fn get_batch<'a>(
        &'a self,
        queries: &[(&ObjectId, &[u8])],
    ) -> Result<Vec<Option<Cow<'a, [u8]>>>> {
        let mut out = vec![None; queries.len().min(1024)];
        self.get_batch_into(queries, &mut out)?;
        Ok(out)
    }
    pub fn get_batch_into<'a>(
        &'a self,
        queries: &[(&ObjectId, &[u8])],
        out: &mut [Option<Cow<'a, [u8]>>],
    ) -> Result<()> {
        if queries.len() > 1024 || out.len() != queries.len() {
            return Err(Error::TooLarge);
        }
        let mut rows: [Option<RecordView<'_>>; 1024] = std::array::from_fn(|_| None);
        self.probe_batch(queries.len(), |n| queries[n].0, &mut rows[..queries.len()])?;
        for (((_, key), row), result) in queries.iter().zip(rows).zip(out) {
            *result = row.and_then(|(stored, value)| (stored.as_ref() == *key).then_some(value));
        }
        Ok(())
    }
    /// Full semantic keys remain available for typed callers and legacy reads.
    /// Probe all partitions together so a random batch retains prefetch depth.
    pub fn rows_batch_into<'a>(
        &'a self,
        ids: &[ObjectId],
        out: &mut [Option<RecordView<'a>>],
    ) -> Result<()> {
        self.probe_batch(ids.len(), |n| &ids[n], out)
    }
    fn probe_batch<'a, 'b>(
        &'a self,
        len: usize,
        id: impl Fn(usize) -> &'b ObjectId,
        out: &mut [Option<RecordView<'a>>],
    ) -> Result<()> {
        if len > 1024 || out.len() != len {
            return Err(Error::TooLarge);
        }
        out.fill(None);
        let mut slots = [0usize; 1024];
        for (n, slot) in slots[..len].iter_mut().enumerate() {
            if let Some(partition) = &self.dispatch[id(n).digest_bytes()[0] as usize] {
                match &partition.index {
                    PartitionIndex::Perfect(index) => *slot = index.prefetch_probe(id(n)),
                    PartitionIndex::Sorted(_) => out[n] = partition.index.row(id(n))?,
                }
            }
        }
        for (n, result) in out.iter_mut().enumerate() {
            if let Some(partition) = &self.dispatch[id(n).digest_bytes()[0] as usize] {
                if let PartitionIndex::Perfect(index) = &partition.index {
                    *result = index.row_at(slots[n], id(n))?;
                }
            }
            if let Some((key, value)) = result {
                super::search::grouped::prefetch(key.as_ref(), 0);
                super::search::grouped::prefetch(value.as_ref(), 0);
            }
        }
        Ok(())
    }
    /// Reuse one native probe for inline and full-record association reads.
    pub(crate) fn scoped_extension(
        &self,
        id: &ObjectId,
        context: &[u8; 32],
        decode: ScopedExtensionDecoder,
    ) -> Result<Option<Cow<'_, [u8]>>> {
        let row = if let Some(index) = &self.native_dispatch[id.digest_bytes()[0] as usize] {
            let slot = index.probe(id);
            if let Some(value) = index.inline_extension_at(slot, id, context)? {
                return Ok(value.map(Cow::Borrowed));
            }
            index.row_at(slot, id)?
        } else {
            self.row(id)?
        };
        let Some((key, value)) = row else {
            return Ok(None);
        };
        if key.as_ref() != identity(id) && key.as_ref() != id.to_string().as_bytes() {
            return Ok(None);
        }
        match value {
            Cow::Borrowed(bytes) => decode(bytes, context).map(|v| v.map(Cow::Borrowed)),
            Cow::Owned(bytes) => decode(&bytes, context).map(|v| v.map(|v| Cow::Owned(v.to_vec()))),
        }
    }
    /// Physical callers use the same navigator and full record fallback. There
    /// is no second local index to build, select or keep consistent.
    #[inline(always)]
    pub(crate) fn locate_local(
        &self,
        id: &ObjectId,
        decode: LocalDecoder,
    ) -> Result<Option<polar_cas::shared::LocatedContent>> {
        if let Some(index) = &self.native_dispatch[id.digest_bytes()[0] as usize] {
            let slot = index.probe(id);
            let mut found = None;
            if index.local_into(slot, id, &mut found)? {
                return Ok(found);
            }
            return index
                .row_at(slot, id)?
                .map(|(k, v)| decode(id, &k, &v))
                .transpose()
                .map(Option::flatten);
        }
        let Some(partition) = &self.dispatch[id.digest_bytes()[0] as usize] else {
            return Ok(None);
        };
        partition
            .index
            .row(id)?
            .map(|(k, v)| decode(id, &k, &v))
            .transpose()
            .map(Option::flatten)
    }
    pub(crate) fn locate_local_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<polar_cas::shared::LocatedContent>],
        decode: LocalDecoder,
    ) -> Result<()> {
        if ids.len() > 1024 || out.len() != ids.len() {
            return Err(Error::TooLarge);
        }
        let mut slots = [0; 1024];
        let mut indexes = [None; 1024];
        for (n, id) in ids.iter().enumerate() {
            if let Some(index) = &self.native_dispatch[id.digest_bytes()[0] as usize] {
                slots[n] = index.prefetch_probe(id);
                indexes[n] = Some(index.as_ref());
            }
        }
        for (n, (id, result)) in ids.iter().zip(out).enumerate() {
            let row = if let Some(index) = indexes[n] {
                if index.local_into(slots[n], id, result)? {
                    continue;
                }
                index.row_at(slots[n], id)?
            } else {
                self.dispatch[id.digest_bytes()[0] as usize]
                    .as_ref()
                    .map(|p| p.index.row(id))
                    .transpose()?
                    .flatten()
            };
            *result = row.map(|(k, v)| decode(id, &k, &v)).transpose()?.flatten();
        }
        Ok(())
    }
    /// All potentially failing work finishes before the caller replaces its
    /// Arc/head. Unchanged partition Arcs and content identities are reused.
    pub fn publish(
        store: &LocalCas,
        previous: Option<&Self>,
        changes: &[ValueChange],
        metadata: serde_json::Value,
    ) -> Result<Self> {
        Self::publish_with_local(store, previous, changes, metadata, None)
    }
    pub(crate) fn publish_with_local(
        store: &LocalCas,
        previous: Option<&Self>,
        changes: &[ValueChange],
        metadata: serde_json::Value,
        local: Option<LocalPosition>,
    ) -> Result<Self> {
        Self::publish_policy(store, previous, changes, metadata, local, true)
    }
    /// Opt-in for callers that arrange explicit maintenance. Retains the file
    /// count bound, while dead-space compaction is deferred until compact().
    pub fn publish_deferred(
        store: &LocalCas,
        previous: Option<&Self>,
        changes: &[ValueChange],
        metadata: serde_json::Value,
    ) -> Result<Self> {
        Self::publish_policy(store, previous, changes, metadata, None, false)
    }
    /// Prepare a compact immutable snapshot; the caller selects it with the
    /// existing expected-head protocol. No head, owner role or task is created.
    pub fn compact(&self) -> Result<Self> {
        Self::publish_policy(
            &self.store,
            Some(self),
            &[],
            self.root.metadata.clone(),
            None,
            true,
        )
    }
    pub(crate) fn publish_policy(
        store: &LocalCas,
        previous: Option<&Self>,
        changes: &[ValueChange],
        metadata: serde_json::Value,
        local: Option<LocalPosition>,
        reclaim: bool,
    ) -> Result<Self> {
        if !matches!(store.key_layout, polar_cas::ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        let _acquisition = super::lifecycle::acquire(store)?;
        let mut selected = BTreeMap::<u8, Vec<&ValueChange>>::new();
        let mut seen = BTreeSet::new();
        for change in changes {
            let id = change.locator();
            if !seen.insert(identity(id)) {
                return Err(Error::Conflict);
            }
            selected
                .entry(id.digest_bytes()[0])
                .or_default()
                .push(change);
        }
        if previous.is_some_and(|p| p.store.root() != store.root()) {
            return Err(Error::Conflict);
        }
        let mut partitions = previous.map(|p| p.partitions.clone()).unwrap_or_default();
        let mut rebuilt = BTreeMap::new();
        for (prefix, changes) in selected {
            // Point-probe before materializing a partition. Repeated imports
            // must not decode/copy every unchanged member in the same prefix.
            let needs_rebuild =
                changes
                    .iter()
                    .try_fold(false, |dirty, change| -> Result<bool> {
                        if dirty {
                            return Ok(true);
                        }
                        let old = partitions
                            .get(&prefix)
                            .map(|p| p.index.row(change.locator()))
                            .transpose()?
                            .flatten();
                        Ok(match change {
                            ValueChange::Put(row) => {
                                old.as_ref().map(|(k, v)| (k.as_ref(), v.as_ref()))
                                    != Some((row.key.as_slice(), row.value.as_slice()))
                            }
                            ValueChange::Remove { key, .. } => {
                                if old
                                    .as_ref()
                                    .is_some_and(|(stored, _)| stored.as_ref() != key.as_slice())
                                {
                                    return Err(Error::Conflict);
                                }
                                old.is_some()
                            }
                            ValueChange::Rekey { previous_key, row } => {
                                if old.as_ref().is_none_or(|(stored, _)| {
                                    stored.as_ref() != previous_key.as_slice()
                                }) {
                                    return Err(Error::Conflict);
                                }
                                old.as_ref().map(|(k, v)| (k.as_ref(), v.as_ref()))
                                    != Some((row.key.as_slice(), row.value.as_slice()))
                            }
                        })
                    })?;
            if !needs_rebuild {
                continue;
            }
            
            let mut dirty = false;
            let mut records: BTreeMap<_, _> = match partitions.get(&prefix) {
                Some(p) => p
                    .read_records()?
                    .into_iter()
                    .map(|r| (identity(&r.locator), r))
                    .collect(),
                None => BTreeMap::new(),
            };
            for change in changes {
                let id = identity(change.locator());
                match change {
                    ValueChange::Rekey { previous_key, row } => {
                        if records.get(&id).is_none_or(|old| old.key != *previous_key) {
                            return Err(Error::Conflict);
                        }
                        records.insert(id, row.clone());
                        dirty = true;
                    }
                    ValueChange::Put(row) => {
                        if records.get(&id).is_some_and(|old| old.key != row.key) {
                            return Err(Error::Conflict);
                        }
                        if records.get(&id) != Some(row) {
                            records.insert(id, row.clone());
                            dirty = true;
                        }
                    }
                    ValueChange::Remove { key, .. } => {
                        if records.get(&id).is_some_and(|old| old.key != *key) {
                            return Err(Error::Conflict);
                        }
                        dirty |= records.remove(&id).is_some();
                    }
                }
            }
            
            if !dirty {
                continue;
            }
            if records.is_empty() {
                partitions.remove(&prefix);
            } else {
                rebuilt.insert(prefix, records.into_values().collect());
            }
        }
        let mut publication = crate::index_publication::IndexPublication::new(store);
        segments::update(
            store,
            &mut publication,
            &mut partitions,
            rebuilt,
            local,
            reclaim,
        )?;
        let root = Root {
            schema: "polar.cas.value-index-root.v1".into(),
            metadata,
            partitions: partitions
                .iter()
                .map(|(p, v)| (*p, v.reference.clone()))
                .collect(),
        };
        if let Some(previous) = previous.filter(|previous| previous.root == root) {
            // Even staging and fsync are unnecessary for an unchanged result.
            return Ok(previous.clone());
        }
        let reference = publication.publish(&directory::encode(&root)?)?;
        publication.finish()?;
        let root_lease = Arc::new(super::lifecycle::pin(store, &reference)?);
        Ok(Self {
            _root_lease: root_lease,
            _rebuilt_lease: None,
            reference,
            root,
            dispatch: std::array::from_fn(|n| partitions.get(&(n as u8)).cloned()),
            native_dispatch: std::array::from_fn(|n| {
                partitions.get(&(n as u8)).and_then(|p| match &p.index {
                    PartitionIndex::Perfect(index) => Some(index.clone()),
                    PartitionIndex::Sorted(_) => None,
                })
            }),
            partitions,
            store: store.clone(),
        })
    }
    /// Reopen a root produced in the trusted LOCAL publication domain. A root
    /// imported from another Store must use rebuild_portable instead.
    pub fn open_local(store: &LocalCas, reference: &ContentRef) -> Result<Self> {
        Self::open(store, reference, true, None)
    }
    pub fn rebuild_portable(store: &LocalCas, reference: &ContentRef) -> Result<Self> {
        Self::open(store, reference, false, None)
    }
    pub(crate) fn rebuild_portable_with_local(
        store: &LocalCas,
        reference: &ContentRef,
        local: LocalPosition,
    ) -> Result<Self> {
        Self::open(store, reference, false, Some(local))
    }
    fn open(
        store: &LocalCas,
        reference: &ContentRef,
        local_native: bool,
        local: Option<LocalPosition>,
    ) -> Result<Self> {
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let _acquisition = super::lifecycle::acquire(store)?;
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("open_acquire", started.elapsed(), 0, 0);
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let root_lease = Arc::new(super::lifecycle::pin(store, reference)?);
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("open_pin", started.elapsed(), 0, 0);
        drop(_acquisition);
        
        #[cfg(any(test, feature = "query-metrics"))]
        let started = std::time::Instant::now();
        let root = if local_native {
            Root::open(&root_lease, reference)?
        } else {
            Root::read(store, reference)?
        };
        #[cfg(any(test, feature = "query-metrics"))]
        write_metrics::record("open_root", started.elapsed(), 0, root.partitions.len() as u64);
        let mut partitions = BTreeMap::new();
        let mut mapped = BTreeMap::<String, Arc<memmap2::Mmap>>::new();
        let mut rebuilt = BTreeMap::new();
        for (prefix, r) in &root.partitions {
            let storage_key = r.portable_file().id.to_string();
            let facts = match mapped.get(&storage_key) {
                Some(map) => map.clone(),
                None => {
                    #[cfg(any(test, feature = "query-metrics"))]
                    let started = std::time::Instant::now();
                    let map = pin_facts(store, r)?;
                    #[cfg(any(test, feature = "query-metrics"))]
                    write_metrics::record("open_map", started.elapsed(), map.len() as u64, 0);
                    mapped.insert(storage_key, map.clone());
                    map
                }
            };
            // The immutable local publisher already admitted these facts. As in
            // SearchIndex, native reopen checks bounded metadata/navigation;
            // portable import or accelerator rebuild verifies the full facts.
            facts_slice(r, &facts)?;
            #[cfg(any(test, feature = "query-metrics"))]
            let started = std::time::Instant::now();
            let native = if local_native
                && (r.platform == platform()
                    || r.platform == platform().replacen("pcasval4", "pcasval3", 1)
                    || r.platform == platform().replacen("pcasval4", "pcasval2", 1)
                    || r.platform == platform().replacen("pcasval4", "pcasval1", 1))
            {
                r.open_native(store, &facts)?
            } else {
                None
            };
            #[cfg(any(test, feature = "query-metrics"))]
            if native.is_some() {
                write_metrics::record("open_native", started.elapsed(), 0, 1);
            }
            let partition = if let Some(index) = native {
                if index.record_count() != r.records {
                    return Err(Error::Integrity);
                }
                Arc::new(Partition {
                    reference: r.clone(),
                    index: PartitionIndex::Perfect(Arc::new(index)),
                    facts,
                })
            } else {
                if !local_native || r.platform != SORTED_PLATFORM {
                    verify_facts(r, &facts)?;
                }
                #[cfg(any(test, feature = "query-metrics"))]
                let started = std::time::Instant::now();
                let records = decode_records(facts_slice(r, &facts)?)?;
                #[cfg(any(test, feature = "query-metrics"))]
                write_metrics::record("open_sorted", started.elapsed(), 0, records.len() as u64);
                if records.is_empty()
                    || records.len() != r.records
                    || records
                        .iter()
                        .any(|row| row.locator.digest_bytes()[0] != *prefix)
                {
                    return Err(Error::Integrity);
                }
                if local_native && r.platform == SORTED_PLATFORM {
                    if r.native != r.facts {
                        return Err(Error::Integrity);
                    }
                    Arc::new(Partition {
                        reference: r.clone(),
                        index: PartitionIndex::Sorted(records),
                        facts,
                    })
                } else {
                    rebuilt.insert(*prefix, records);
                    continue;
                }
            };
            partitions.insert(*prefix, partition);
        }
        // Imported facts are validated above; rebuild only local accelerators
        // and pack all partitions together, never deserialize foreign native bytes.
        let rebuilt_lease = if !rebuilt.is_empty() {
            // The source root protects reads. New output needs publication
            // protection only from preparation until its own root is pinned.
            let _publication_guard = super::lifecycle::acquire(store)?;
            let mut publication = crate::index_publication::IndexPublication::new(store);
            partitions.extend(segments::build(store, &mut publication, rebuilt, local)?);
            let prepared = Root {
                schema: root.schema.clone(),
                metadata: root.metadata.clone(),
                partitions: partitions
                    .iter()
                    .map(|(p, v)| (*p, v.reference.clone()))
                    .collect(),
            };
            if prepared != root {
                let reference = publication.publish(&directory::encode(&prepared)?)?;
                publication.finish()?;
                Some(Arc::new(super::lifecycle::pin(store, &reference)?))
            } else {
                publication.finish()?;
                None
            }
        } else {
            None
        };
        Ok(Self {
            _root_lease: root_lease,
            _rebuilt_lease: rebuilt_lease,
            reference: reference.clone(),
            root,
            dispatch: std::array::from_fn(|n| partitions.get(&(n as u8)).cloned()),
            native_dispatch: std::array::from_fn(|n| {
                partitions.get(&(n as u8)).and_then(|p| match &p.index {
                    PartitionIndex::Perfect(index) => Some(index.clone()),
                    PartitionIndex::Sorted(_) => None,
                })
            }),
            partitions,
            store: store.clone(),
        })
    }
}








