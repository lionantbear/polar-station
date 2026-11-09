use super::*;
use crate::{
    shared_index::{
        partitions::{ValueChange, ValueSnapshot},
        values::ValueRecord,
    },
    LocalCas,
};
use polar_cas::store_index::{IndexBase, IndexLimits};
mod binary;
pub(super) use binary::key;

pub(super) use binary::{contents_page, decode_contents, encode_contents, extension};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedLocation {
    scope: Option<ObjectId>,
    location: IndexedLocation,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    coverage: StoreCoverage,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metadata {
    pub(super) schema: String,
    sources: Vec<Source>,
    #[serde(default)]
    writer_records: u64,
    #[serde(default)]
    local: polar_cas::shared::SharedIndexGeneration,
    #[serde(default)]
    pub(super) projections: BTreeMap<String, CatalogProjection>,
}

/// Bounded root metadata checks shared by opening and initial publication.
/// This does not open a partition or enumerate any stored record.
pub(super) fn decode_metadata(value: &serde_json::Value) -> Result<Metadata> {
    if value.get("schema").and_then(serde_json::Value::as_str) == Some("polar.cas.catalog-index.v5")
        && value
            .get("writer_records")
            .and_then(serde_json::Value::as_u64)
            .is_none()
    {
        return Err(Error::Integrity);
    }
    let metadata: Metadata = serde_json::from_value(value.clone()).map_err(|_| Error::Integrity)?;
    if !matches!(
        metadata.schema.as_str(),
        "polar.cas.catalog-index.v2"
            | "polar.cas.catalog-index.v3"
            | "polar.cas.catalog-index.v4"
            | "polar.cas.catalog-index.v5"
    ) || metadata.sources.len() > 256
    {
        return Err(Error::Integrity);
    }
    super::projection::validate(&metadata.projections)?;
    let mut sources = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for source in &metadata.sources {
        source.coverage.store.validate()?;
        if let Some(scope) = &source.coverage.scope {
            scope.require_sha256()?;
        }
        total = total
            .checked_add(source.coverage.indexed_entries)
            .ok_or(Error::TooLarge)?;
        if total > 1_000_000
            || !sources.insert(attachment_key(
                &source.coverage.store,
                source.coverage.scope.as_ref(),
            ))
        {
            return Err(Error::Integrity);
        }
    }
    super::local::coverage_packs(&metadata.local)?;
    Ok(metadata)
}

impl UnifiedCatalogIndex {
    /// Use the Host's existing CAS for immutable index artifacts. No Store is
    /// enrolled, scanned or fetched by this binding, and no SQL is queried.
    pub fn with_storage(storage: LocalCas) -> Self {
        Self {
            publication: Default::default(),
            query_snapshots: Default::default(),
            state: RwLock::default(),
            storage: Some(storage),
            head: None,
        }
    }
    /// Reuse the caller's CAS; only a root pointer and publication lock
    /// live in this group directory. No source enrollment or Store scan.
    pub fn open_managed(storage: LocalCas, directory: &std::path::Path) -> Result<Self> {
        let _acquisition = crate::shared_index::lifecycle::acquire(&storage)?;
        let head = super::head::Head::open_bound(&storage, directory)?;
        let mut index = match head.current()? {
            Some(root) => Self::open_local(storage, &root)?,
            None => Self::with_storage(storage),
        };
        index.head = Some(head);
        Ok(index)
    }
    /// Same local CAS root means one group Catalog. Reuse the handle in-process;
    /// separate processes share immutable CAS files and the atomic head.
    pub fn open_shared(storage: LocalCas) -> Result<Arc<Self>> {
        use std::sync::{Mutex, OnceLock, Weak};
        static OPEN: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Weak<UnifiedCatalogIndex>>>> =
            OnceLock::new();
        let path = storage
            .root()
            .canonicalize()?
            .join(".polar-catalog-index-v2");
        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Backend("shared Catalog registry poisoned".into()))?;
        if let Some(index) = open.get(&path).and_then(Weak::upgrade) {
            return Ok(index);
        }
        let index = Arc::new(Self::open_managed(storage, &path)?);
        open.retain(|_, index| index.strong_count() != 0);
        open.insert(path, Arc::downgrade(&index));
        Ok(index)
    }
    /// Explicit request-boundary refresh; pure lookup never opens current.
    /// Existing CatalogSnapshot handles retain their selected generation.
    pub fn refresh(&self) -> Result<bool> {
        let Some(head) = &self.head else {
            return Ok(false);
        };
        // The current snapshot already pins its immutable root. An unchanged
        // head needs neither publication serialization nor a new root lease;
        // historical collection must not stall these request-boundary reads.
        let current = self.current()?;
        if head.current()? == current.index.as_ref().map(|index| index.reference().clone()) {
            return Ok(false);
        }
        let _publication = self.publication_guard()?;
        let _acquisition = crate::shared_index::lifecycle::acquire(
            self.storage.as_ref().ok_or(Error::UnsupportedCapability)?,
        )?;
        let old = self.current()?;
        let expected = old.index.as_ref().map(|i| i.reference().clone());
        let selected = head.current()?;
        if selected == expected {
            return Ok(false);
        }
        let root = selected.ok_or(Error::Integrity)?;
        let next = Self::open_local(
            self.storage.clone().ok_or(Error::UnsupportedCapability)?,
            &root,
        )?;
        let mut current = self
            .state
            .write()
            .map_err(|_| Error::Backend("Catalog state poisoned".into()))?;
        if !Arc::ptr_eq(&current, &old) {
            return Err(Error::Conflict);
        }
        *current = next.current()?;
        Ok(true)
    }
    /// Rebuild verified portable records from an explicitly selected root.
    /// Foreign native accelerators are never deserialized.
    pub fn rebuild_portable(storage: LocalCas, root: &ContentRef) -> Result<Self> {
        let index = ValueSnapshot::rebuild_portable_with_local(&storage, root, inline_local)?;
        Self::from_values(storage, index)
    }
    pub fn root(&self) -> Result<Option<ContentRef>> {
        Ok(self
            .current()?
            .index
            .as_ref()
            .map(|i| i.reference().clone()))
    }
    /// Caller selected a locally published Catalog root. Portable import is a
    /// separate operation so foreign native bytes never reach epserde.
    pub fn open_local(storage: LocalCas, root: &ContentRef) -> Result<Self> {
        let index = ValueSnapshot::open_local(&storage, root)?;
        Self::from_values(storage, index)
    }
    pub(super) fn from_values(storage: LocalCas, index: ValueSnapshot) -> Result<Self> {
        let metadata = decode_metadata(index.metadata())?;
        let sources: BTreeMap<_, _> = metadata
            .sources
            .iter()
            .map(|source| {
                (
                    attachment_key(&source.coverage.store, source.coverage.scope.as_ref()),
                    source,
                )
            })
            .collect();
        let local_packs = super::local::coverage_packs(&metadata.local)?;
        let mut rows = BTreeMap::<String, Vec<StoreEntry>>::new();
        let mut expected_writers = BTreeMap::<Vec<u8>, BTreeMap<String, ContentRef>>::new();
        let mut actual_writers = BTreeMap::<Vec<u8>, Vec<ContentRef>>::new();
        let mut writer_records = 0u64;
        index.visit_records(|row| {
            if row.key.starts_with(super::writers::PREFIX) {
                if metadata.schema != "polar.cas.catalog-index.v5" {
                    return Err(Error::Integrity);
                }
                let contents = super::writers::validate_bucket(&row)?;
                if actual_writers.insert(row.key, contents).is_some() {
                    return Err(Error::Integrity);
                }
                return Ok(());
            }
            if row.key != key(&row.locator) && row.key != row.locator.to_string().as_bytes() {
                return Err(Error::Integrity);
            }
            let positions = decode(&row.value)?;
            validate_positions(&row.locator, &positions)?;
            if !positions.writers.is_empty() && metadata.schema != "polar.cas.catalog-index.v5" {
                return Err(Error::Integrity);
            }
            writer_records = writer_records
                .checked_add(positions.writers.len() as u64)
                .ok_or(Error::TooLarge)?;
            for record in &positions.writers {
                expected_writers
                    .entry(super::writers::bucket_key(
                        &record.attribution.writer,
                        &row.locator,
                    ))
                    .or_default()
                    .insert(row.locator.to_string(), record.content.clone());
            }
            for local in &positions.local {
                if local.content.id != row.locator {
                    return Err(Error::Integrity);
                }
                super::local::validate(
                    local,
                    &local_packs,
                    !metadata.local.covered_loose.is_empty(),
                )?;
            }
            for selected in positions.remote {
                if selected.location.entry.content.id != row.locator {
                    return Err(Error::Integrity);
                }
                let key = attachment_key(&selected.location.store, selected.scope.as_ref());
                let source = sources.get(&key).ok_or(Error::Integrity)?;
                if selected.location.store != source.coverage.store
                    || selected.location.index_root != source.coverage.root
                    || selected.location.generation != source.coverage.generation
                    || selected.location.received_at_ms != source.coverage.received_at_ms
                {
                    return Err(Error::Integrity);
                }
                let entries = rows.entry(key).or_default();
                if entries.len() >= source.coverage.indexed_entries {
                    return Err(Error::Integrity);
                }
                entries.push(selected.location.entry);
            }
            Ok(())
        })?;
        let expected_writers: BTreeMap<_, _> = expected_writers
            .into_iter()
            .map(|(key, values)| (key, values.into_values().collect::<Vec<_>>()))
            .collect();
        if writer_records != metadata.writer_records || expected_writers != actual_writers {
            return Err(Error::Integrity);
        }
        drop(sources);
        let mut state = State {
            writer_records,
            local: metadata.local,
            projections: metadata.projections,
            ..State::default()
        };
        for source in metadata.sources {
            let c = source.coverage;
            let key = attachment_key(&c.store, c.scope.as_ref());
            let mut entries = rows.remove(&key).unwrap_or_default();
            if entries.len() != c.indexed_entries {
                return Err(Error::Integrity);
            }
            let mut keyed = entries
                .drain(..)
                .map(|entry| Ok((entry.slot()?, entry)))
                .collect::<Result<Vec<_>>>()?;
            keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            entries = keyed.into_iter().map(|(_, entry)| entry).collect();
            let snapshot = IndexSnapshot::from_base(
                IndexBase {
                    schema: "polar.cas.store-index-base.v1".into(),
                    store: c.store,
                    generation: c.generation,
                    entries,
                },
                IndexLimits {
                    max_entries: 1_000_000,
                    ..IndexLimits::default()
                },
            )?;
            if snapshot.select(&c.selection)?.len() != snapshot.len() {
                return Err(Error::Integrity);
            }
            if state
                .stores
                .insert(
                    key,
                    Attached {
                        snapshot: Arc::new(snapshot),
                        root: c.root,
                        received_at_ms: c.received_at_ms,
                        scope: c.scope,
                        selection: c.selection,
                    },
                )
                .is_some()
            {
                return Err(Error::Integrity);
            }
        }
        if !rows.is_empty() {
            return Err(Error::Integrity);
        }
        state.index = Some(Arc::new(index));
        Ok(Self {
            publication: Default::default(),
            query_snapshots: Default::default(),
            state: RwLock::new(Arc::new(state)),
            storage: Some(storage),
            head: None,
        })
    }
}
fn coverage(a: &Attached) -> StoreCoverage {
    StoreCoverage {
        store: a.snapshot.store().clone(),
        generation: a.snapshot.generation(),
        root: a.root.clone(),
        received_at_ms: a.received_at_ms,
        indexed_entries: a.snapshot.len(),
        selection: a.selection.clone(),
        scope: a.scope.clone(),
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Positions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) extensions: Vec<super::extensions::ScopedExtension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) writers: Vec<polar_cas::catalog::writers::WriterRecord>,
    pub(super) local: Vec<polar_cas::shared::LocatedContent>,
    remote: Vec<SelectedLocation>,
}


pub(super) fn decode(bytes: &[u8]) -> Result<Positions> {
    if bytes.starts_with(b"PCASPOS1") || bytes.starts_with(b"PCASPOS2") {
        return binary::decode(bytes);
    }
    // v2 roots contain a remote-only vector. New writes always use v3 envelopes.
    if bytes.first() == Some(&b'[') {
        Ok(Positions {
            extensions: vec![],
            writers: vec![],
            remote: serde_json::from_slice(bytes).map_err(|_| Error::Integrity)?,
            local: vec![],
        })
    } else {
        serde_json::from_slice(bytes).map_err(|_| Error::Integrity)
    }
}
pub(super) fn selected_candidates(
    state: &State,
    id: &ObjectId,
    scope: Option<&ObjectId>,
    limit: usize,
) -> Result<Vec<IndexedLocation>> {
    let mut candidates = Vec::new();
    for row in positions(state, id)?.remote {
        if row.scope.is_some() && row.scope.as_ref() != scope {
            continue;
        }
        if row.location.entry.content.id != *id {
            return Err(Error::Integrity);
        }
        if candidates.len() == limit {
            return Err(Error::TooLarge);
        }
        let mut candidate = row.location;
        candidate.scope = row.scope;
        candidates.push(candidate);
    }
    Ok(candidates)
}
pub(super) fn positions(state: &State, id: &ObjectId) -> Result<Positions> {
    match state
        .index
        .as_ref()
        .map(|i| record(i, id))
        .transpose()?
        .flatten()
    {
        Some(bytes) => decode(&bytes),
        None => Ok(Positions::default()),
    }
}
pub(super) fn record<'a>(
    index: &'a ValueSnapshot,
    id: &ObjectId,
) -> Result<Option<std::borrow::Cow<'a, [u8]>>> {
    let Some((stored, value)) = index.row(id)? else {
        return Ok(None);
    };
    if stored.as_ref() == key(id) {
        return Ok(Some(value));
    }
    // Read-only compatibility; missing/binary lookups allocate no hexadecimal key.
    Ok((stored.as_ref() == id.to_string().as_bytes()).then_some(value))
}
pub(super) fn local(bytes: &[u8]) -> Result<Vec<polar_cas::shared::LocatedContent>> {
    if bytes.starts_with(b"PCASPOS1") || bytes.starts_with(b"PCASPOS2") {
        binary::local(bytes)
    } else {
        Ok(decode(bytes)?.local)
    }
}
pub(super) fn inline_local(row: &ValueRecord) -> Result<Option<polar_cas::shared::LocatedContent>> {
    if row.key != key(&row.locator) {
        return Ok(None);
    }
    first_local(&row.value)
}
pub(super) fn local_record(
    id: &ObjectId,
    stored: &[u8],
    value: &[u8],
) -> Result<Option<polar_cas::shared::LocatedContent>> {
    if stored == key(id) || stored == id.to_string().as_bytes() {
        first_local(value)
    } else {
        Ok(None)
    }
}
pub(super) fn first_local(bytes: &[u8]) -> Result<Option<polar_cas::shared::LocatedContent>> {
    if bytes.starts_with(b"PCASPOS1") || bytes.starts_with(b"PCASPOS2") {
        binary::first_local(bytes)
    } else {
        Ok(decode(bytes)?.local.into_iter().next())
    }
}
fn validate_positions(id: &ObjectId, positions: &Positions) -> Result<()> {
    super::writers::validate_forward(id, &positions.writers)?;
    let mut size = None;
    for content in positions
        .writers
        .iter()
        .map(|r| &r.content)
        .chain(positions.local.iter().map(|r| &r.content))
        .chain(positions.remote.iter().map(|r| &r.location.entry.content))
    {
        if content.id != *id || size.is_some_and(|old| old != content.size_bytes) {
            return Err(Error::Integrity);
        }
        size = Some(content.size_bytes);
    }
    Ok(())
}
pub(super) fn change(id: ObjectId, positions: Positions) -> Result<ValueChange> {
    validate_positions(&id, &positions)?;
    let key = key(&id).to_vec();
    if positions.local.is_empty()
        && positions.remote.is_empty()
        && positions.writers.is_empty()
        && positions.extensions.is_empty()
    {
        Ok(ValueChange::Remove { locator: id, key })
    } else {
        Ok(ValueChange::Put(ValueRecord {
            locator: id,
            key,
            value: binary::encode(&positions)?,
        }))
    }
}
pub(super) fn publish_values(
    storage: &LocalCas,
    previous: Option<&ValueSnapshot>,
    changes: &[ValueChange],
    metadata: serde_json::Value,
) -> Result<ValueSnapshot> {
    let mut changes = changes.to_vec();
    if let Some(previous) = previous {
        for change in &mut changes {
            let (id, new_key) = match change {
                ValueChange::Put(row) => (&row.locator, &row.key),
                ValueChange::Remove { locator, key } => (&*locator, &*key),
                ValueChange::Rekey { .. } => continue,
            };
            if new_key.as_slice() != key(id) || previous.get(id, new_key)?.is_some() {
                continue;
            }
            let previous_key = id.to_string().into_bytes();
            if previous.get(id, &previous_key)?.is_none() {
                continue;
            }
            *change = match change {
                ValueChange::Put(row) => ValueChange::Rekey {
                    previous_key,
                    row: row.clone(),
                },
                ValueChange::Remove { locator, .. } => ValueChange::Remove {
                    locator: locator.clone(),
                    key: previous_key,
                },
                ValueChange::Rekey { .. } => unreachable!(),
            };
        }
    }
    ValueSnapshot::publish_policy(
        storage,
        previous,
        &changes,
        metadata,
        Some(inline_local),
        false,
    )
}
pub(super) fn metadata(state: &State) -> Result<serde_json::Value> {
    serde_json::to_value(Metadata {
        schema: "polar.cas.catalog-index.v5".into(),
        writer_records: state.writer_records,
        sources: state
            .stores
            .values()
            .map(|a| Source {
                coverage: coverage(a),
            })
            .collect(),
        local: state.local.clone(),
        projections: state.projections.clone(),
    })
    .map_err(|_| Error::Integrity)
}
pub(super) fn lookup(
    state: &State,
    content: &ContentRef,
    limit: usize,
    scope: Option<&ObjectId>,
) -> Result<CatalogLookup> {
    let mut out = CatalogLookup {
        snapshot: state.index.as_ref().map(|i| i.reference().clone()),
        candidates: vec![],
        local: vec![],
        coverage: vec![],
        truncated: false,
    };
    let Some(index) = &state.index else {
        return Ok(out);
    };
    // Only coverage metadata is enumerated. Content lookup goes to one partition,
    // never into each Store's index or the transactional SQL catalog.
    out.coverage = state
        .stores
        .values()
        .filter(|a| a.scope.is_none() || a.scope.as_ref() == scope)
        .map(coverage)
        .collect();
    let Some(bytes) = record(index, &content.id)? else {
        return Ok(out);
    };
    let positions = decode(&bytes)?;
    for local in positions.local {
        if local.content != *content {
            return Err(Error::ClaimMismatch);
        }
        if out.local.len() == limit {
            out.truncated = true;
            break;
        }
        out.local.push(local);
    }
    for row in positions.remote {
        if row.scope.is_some() && row.scope.as_ref() != scope {
            continue;
        }
        if row.location.entry.content != *content {
            return Err(Error::ClaimMismatch);
        }
        if out.candidates.len() + out.local.len() == limit {
            out.truncated = true;
            break;
        }
        let mut location = row.location;
        location.scope = row.scope;
        out.candidates.push(location);
    }
    Ok(out)
}
pub(super) fn publish(
    storage: &LocalCas,
    old: &State,
    next: &mut State,
    changed: &str,
) -> Result<()> {
    let mut touched = BTreeMap::new();
    for attached in [old.stores.get(changed), next.stores.get(changed)]
        .into_iter()
        .flatten()
    {
        for entry in attached.snapshot.entries() {
            touched.insert(entry.content.id.to_string(), entry.content.clone());
        }
    }
    let mut changes = Vec::with_capacity(touched.len());
    for (_, content) in touched {
        let mut candidates = Vec::new();
        for a in next.stores.values() {
            for entry in a.snapshot.candidates(&content) {
                let entry = entry?;
                candidates.push(SelectedLocation {
                    scope: a.scope.clone(),
                    location: IndexedLocation {
                        scope: a.scope.clone(),
                        store: a.snapshot.store().clone(),
                        index_root: a.root.clone(),
                        generation: a.snapshot.generation(),
                        received_at_ms: a.received_at_ms,
                        entry: entry.clone(),
                    },
                });
            }
        }
        let mut positions = positions(old, &content.id)?;
        positions.remote = candidates;
        changes.push(change(content.id, positions)?);
    }
    next.index = Some(Arc::new(publish_values(
        storage,
        old.index.as_deref(),
        &changes,
        metadata(next)?,
    )?));
    Ok(())
}




