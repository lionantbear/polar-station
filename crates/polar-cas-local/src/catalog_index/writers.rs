//! Writer associations and sharded reverse keys in the existing ValueSnapshot.
//! Only explicit successful saves call record_writes; reads do not mutate state.
use super::{persistent, CatalogSnapshot, UnifiedCatalogIndex};
use crate::shared_index::{partitions::ValueChange, values::ValueRecord};
use polar_cas::{
    catalog::writers::{WriterId, WriterRecord},
    ContentRef, Error, ObjectId, Result,
};
use std::{collections::BTreeMap, sync::Arc};

const QUERY_IDLE: std::time::Duration = std::time::Duration::from_secs(300);
pub(super) struct RetainedSnapshot {
    snapshot: CatalogSnapshot,
    last_used: std::time::Instant,
}
const MAX_ASSOCIATIONS: usize = 4096;
const MAX_BUCKET_CONTENTS: usize = 65_536;
pub(super) const PREFIX: &[u8] = b"writer:v1:";

pub(super) fn bucket_key(writer: &WriterId, content: &ObjectId) -> Vec<u8> {
    format!(
        "writer:v1:{}:{}:{:02x}",
        writer.as_str(),
        content.algorithm().as_str(),
        content.digest_bytes()[0]
    )
    .into_bytes()
}
fn bucket_key_at(writer: &WriterId, algorithm: &str, prefix: u8) -> Vec<u8> {
    format!("writer:v1:{}:{algorithm}:{prefix:02x}", writer.as_str()).into_bytes()
}
fn decode_bucket(key: &[u8], bytes: &[u8]) -> Result<Vec<ContentRef>> {
    if bytes.len() > 16 << 20 {
        return Err(Error::TooLarge);
    }
    let text = std::str::from_utf8(key).map_err(|_| Error::Integrity)?;
    let parts: Vec<_> = text.split(':').collect();
    if parts.len() != 5
        || parts[0] != "writer"
        || parts[1] != "v1"
        || !matches!(parts[3], "sha256" | "blake3")
        || parts[4].len() != 2
    {
        return Err(Error::Integrity);
    }
    let writer = WriterId::new(parts[2]).map_err(|_| Error::Integrity)?;
    let prefix = u8::from_str_radix(parts[4], 16).map_err(|_| Error::Integrity)?;
    if key != bucket_key_at(&writer, parts[3], prefix) {
        return Err(Error::Integrity);
    }
    let contents = persistent::decode_contents(bytes)?;
    if contents.is_empty() || contents.len() > MAX_BUCKET_CONTENTS {
        return Err(Error::Integrity);
    }
    let mut previous = None;
    for content in &contents {
        let id = content.id.to_string();
        if content.size_bytes > i64::MAX as u64
            || bucket_key(&writer, &content.id) != key
            || previous.as_ref().is_some_and(|p| p >= &id)
        {
            return Err(Error::Integrity);
        }
        previous = Some(id);
    }
    Ok(contents)
}
pub(super) fn validate_bucket(row: &ValueRecord) -> Result<Vec<ContentRef>> {
    if row.locator != ObjectId::for_bytes(&row.key) {
        return Err(Error::Integrity);
    }
    decode_bucket(&row.key, &row.value)
}
pub(super) fn validate_forward(id: &ObjectId, records: &[WriterRecord]) -> Result<()> {
    if records.len() > MAX_ASSOCIATIONS {
        return Err(Error::TooLarge);
    }
    let mut previous = None;
    for record in records {
        record.validate()?;
        let key = record.association_key();
        if record.content.id != *id
            || records
                .first()
                .is_some_and(|first| first.content != record.content)
            || previous.as_ref().is_some_and(|p| p >= &key)
        {
            return Err(Error::Integrity);
        }
        previous = Some(key);
    }
    Ok(())
}

/// Maximum time an active save waits to enter the existing summary lock domain.
pub const WRITER_PUBLICATION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A current-save publication owns the existing summary lock until all required
/// batches finish. Explicit stale-snapshot APIs do not acquire this guard.
pub struct WriterPublication<'a> {
    index: &'a UnifiedCatalogIndex,
    stop: &'a std::sync::atomic::AtomicBool,
    _summary: std::fs::File,
}


impl WriterPublication<'_> {
    pub fn record_writes(&self, records: &[WriterRecord]) -> Result<()> {
        if self.stop.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Io(std::io::ErrorKind::Interrupted.into()));
        }
        self.index
            .record_writes_with_cancel(records, Some(self.stop))
    }
}

impl UnifiedCatalogIndex {
    pub(super) fn expire_idle_query_snapshots(&self) -> Result<()> {
        let now = std::time::Instant::now();
        self.query_snapshots
            .lock()
            .map_err(|_| Error::Backend("query snapshot retention poisoned".into()))?
            .retain(|old| now.saturating_duration_since(old.last_used) < QUERY_IDLE);
        Ok(())
    }

    /// Enter the shared physical-publication domain before the one request-boundary
    /// refresh. Deadline/cancellation govern lock acquisition, not an atomic commit
    /// already in progress. No Conflict is retried or rebased.
    pub fn begin_writer_publication<'a>(
        &'a self,
        stop: &'a std::sync::atomic::AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<WriterPublication<'a>> {
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        if !self.supports_writer_records() {
            return Err(Error::UnsupportedCapability);
        }
        let summary = crate::shared_index::save_publication_lock(storage, stop, deadline)?;
        self.refresh()?;
        if stop.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Io(std::io::ErrorKind::Interrupted.into()));
        }
        Ok(WriterPublication {
            index: self,
            stop,
            _summary: summary,
        })
    }

    /// Publish forward facts and reverse keys together under the existing head.
    /// No retry/refresh: stale writers return Conflict without changing their head.
    /// Unchanged repeated associations leave the root byte-for-byte unchanged.
    pub fn record_writes(&self, records: &[WriterRecord]) -> Result<()> {
        self.record_writes_with_cancel(records, None)
    }

    fn record_writes_with_cancel(
        &self,
        records: &[WriterRecord],
        stop: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        let check_cancelled = || {
            if stop.is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire)) {
                Err(Error::Io(std::io::ErrorKind::Interrupted.into()))
            } else {
                Ok(())
            }
        };
        check_cancelled()?;
        if records.is_empty() {
            return Ok(());
        }
        if records.len() > 1024 {
            return Err(Error::TooLarge);
        }
        for record in records {
            record.validate()?;
        }
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        
        let _publication = self.publication_guard()?;
        check_cancelled()?;
        let old = self.current()?;
        let mut next = (*old).clone();
        let mut positions = BTreeMap::new();
        let mut buckets = BTreeMap::<Vec<u8>, BTreeMap<String, ContentRef>>::new();
        for record in records {
            let content_key = record.content.id.to_string();
            if !positions.contains_key(&content_key) {
                positions.insert(
                    content_key.clone(),
                    persistent::positions(&old, &record.content.id)?,
                );
            }
            let row = positions.get_mut(&content_key).ok_or(Error::Integrity)?;
            if row
                .writers
                .first()
                .is_some_and(|first| first.content != record.content)
                || row
                    .local
                    .iter()
                    .any(|local| local.content != record.content)
            {
                return Err(Error::Integrity);
            }
            let key = record.association_key();
            if let Some(previous) = row.writers.iter_mut().find(|r| r.association_key() == key) {
                // Store keys omit the definition size; an equal association key
                // must not acknowledge a different exact immutable attribution.
                // Runtime destination aliases still deduplicate as before.
                if previous.attribution != record.attribution {
                    return Err(Error::Conflict);
                }
                previous.first_saved_at_ms =
                    previous.first_saved_at_ms.min(record.first_saved_at_ms);
            } else {
                if row.writers.len() >= MAX_ASSOCIATIONS {
                    return Err(Error::TooLarge);
                }
                row.writers.push(record.clone());
                row.writers.sort_by_key(WriterRecord::association_key);
                next.writer_records = next.writer_records.checked_add(1).ok_or(Error::TooLarge)?;
            }
            let key = bucket_key(&record.attribution.writer, &record.content.id);
            if !buckets.contains_key(&key) {
                let locator = ObjectId::for_bytes(&key);
                let previous = old
                    .index
                    .as_ref()
                    .map(|i| i.get(&locator, &key))
                    .transpose()?
                    .flatten();
                let entries = previous
                    .map(|b| decode_bucket(&key, &b))
                    .transpose()?
                    .unwrap_or_default();
                buckets.insert(
                    key.clone(),
                    entries.into_iter().map(|c| (c.id.to_string(), c)).collect(),
                );
            }
            let bucket = buckets.get_mut(&key).ok_or(Error::Integrity)?;
            if bucket
                .get(&content_key)
                .is_some_and(|c| c != &record.content)
            {
                return Err(Error::Integrity);
            }
            bucket.insert(content_key, record.content.clone());
            if bucket.len() > MAX_BUCKET_CONTENTS {
                return Err(Error::TooLarge);
            }
        }
        let mut changes = Vec::new();
        for (id, row) in positions {
            let id = ObjectId::try_from(id)?;
            let change = persistent::change(id.clone(), row)?;
            let ValueChange::Put(ref record) = change else {
                return Err(Error::Integrity);
            };
            let value = &record.value;
            if old
                .index
                .as_ref()
                .map(|i| persistent::record(i, &id))
                .transpose()?
                .flatten()
                != Some(std::borrow::Cow::Borrowed(value.as_slice()))
            {
                changes.push(change);
            }
        }
        for (key, contents) in buckets {
            let locator = ObjectId::for_bytes(&key);
            let value = persistent::encode_contents(&contents.into_values().collect::<Vec<_>>())?;
            if old
                .index
                .as_ref()
                .map(|i| i.get(&locator, &key))
                .transpose()?
                .flatten()
                != Some(std::borrow::Cow::Borrowed(value.as_slice()))
            {
                changes.push(ValueChange::Put(ValueRecord {
                    locator,
                    key,
                    value,
                }));
            }
        }
        check_cancelled()?;
        if changes.is_empty() {
            // A stale process must not claim durable success using an old root.
            if let Some(head) = &self.head {
                let _lock = head.writer()?;
                let reference = old
                    .index
                    .as_ref()
                    .map(|i| i.reference())
                    .ok_or(Error::Integrity)?;
                head.confirm(reference)?;
            }
            return Ok(());
        }
        next.index = Some(Arc::new(persistent::publish_values(
            storage,
            old.index.as_deref(),
            &changes,
            persistent::metadata(&next)?,
        )?));
        check_cancelled()?;
        self.commit(&old, next)
    }

    /// Retain an already-open trusted root for stateless cursor continuations.
    /// At most 64 roots, with a five-minute idle limit. Expired/restarted
    /// cursors must restart; reads never rebuild native indexes from user input.
    pub fn pin_query_snapshot(&self) -> Result<CatalogSnapshot> {
        self.retain_query_snapshot(self.snapshot()?)
    }
    fn retain_query_snapshot(&self, snapshot: CatalogSnapshot) -> Result<CatalogSnapshot> {
        if snapshot.root().is_none() {
            return Ok(snapshot);
        }
        let mut retained = self
            .query_snapshots
            .lock()
            .map_err(|_| Error::Backend("query snapshot retention poisoned".into()))?;
        let now = std::time::Instant::now();
        retained.retain(|old| now.saturating_duration_since(old.last_used) < QUERY_IDLE);
        if let Some(old) = retained
            .iter_mut()
            .find(|old| old.snapshot.root() == snapshot.root())
        {
            old.last_used = now;
        } else {
            while retained.len() >= 64 {
                retained.pop_front();
            }
            retained.push_back(RetainedSnapshot {
                snapshot: snapshot.clone(),
                last_used: now,
            });
        }
        Ok(snapshot)
    }
    /// Accept only a trusted root already retained for queries by this handle.
    /// A current head alone does not revive an expired or restarted cursor.
    /// A CAS content hash alone never grants open_local authority.
    pub fn snapshot_at(&self, root: &ContentRef) -> Result<CatalogSnapshot> {
        root.id.require_sha256()?;
        let now = std::time::Instant::now();
        let mut retained = self
            .query_snapshots
            .lock()
            .map_err(|_| Error::Backend("query snapshot retention poisoned".into()))?;
        retained.retain(|old| now.saturating_duration_since(old.last_used) < QUERY_IDLE);
        let stored = retained
            .iter_mut()
            .find(|stored| stored.snapshot.root() == Some(root))
            .ok_or(Error::Conflict)?;
        stored.last_used = now;
        Ok(stored.snapshot.clone())
    }
}

impl CatalogSnapshot {
    pub fn writer_record_count(&self) -> u64 {
        self.0.writer_records
    }
    /// Exact content lookup. Private scopes are not exposed by a public query;
    /// the runtime must additionally filter the returned destination bindings.
    pub fn writer_records(
        &self,
        content: &ContentRef,
        writer: Option<&WriterId>,
        scope: Option<&ObjectId>,
    ) -> Result<Vec<WriterRecord>> {
        if let Some(scope) = scope {
            scope.require_sha256()?;
        }
        let records = self.writer_records_for_id(&content.id, writer, scope)?;
        if records.iter().any(|record| record.content != *content) {
            return Err(Error::Integrity);
        }
        Ok(records)
    }
    /// Exact identity lookup for a shared object not yet registered in this
    /// caller's transactional Catalog. This does not invent an alias or size.
    pub fn writer_records_for_id(
        &self,
        content: &ObjectId,
        writer: Option<&WriterId>,
        scope: Option<&ObjectId>,
    ) -> Result<Vec<WriterRecord>> {
        if let Some(scope) = scope {
            scope.require_sha256()?;
        }
        Ok(persistent::positions(&self.0, content)?
            .writers
            .into_iter()
            .filter(|record| {
                record.visible_in(scope) && writer.is_none_or(|id| &record.attribution.writer == id)
            })
            .collect())
    }
    /// Stable lexical ContentRef candidate page in this fixed snapshot. Reads
    /// only this writer's algorithm/prefix buckets, never all Catalog rows.
    /// Candidates may be private or inaccessible: callers MUST intersect facts
    /// with their scope and Store bindings before returning any result.
    pub fn writer_contents(
        &self,
        writer: &WriterId,
        after: Option<&ObjectId>,
        limit: u32,
    ) -> Result<Vec<ContentRef>> {
        if !(1..=201).contains(&limit) {
            return Err(Error::Invalid("invalid writer candidate page".into()));
        }
        let Some(index) = &self.0.index else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for algorithm in ["blake3", "sha256"] {
            if after.is_some_and(|a| a.algorithm().as_str() > algorithm) {
                continue;
            }
            for prefix in 0..=u8::MAX {
                if after.is_some_and(|a| {
                    a.algorithm().as_str() == algorithm && prefix < a.digest_bytes()[0]
                }) {
                    continue;
                }
                let key = bucket_key_at(writer, algorithm, prefix);
                let locator = ObjectId::for_bytes(&key);
                let Some(value) = index.get(&locator, &key)? else {
                    continue;
                };
                if let Some(page) =
                    persistent::contents_page(&value, after, limit as usize - out.len())?
                {
                    for content in page {
                        if content.size_bytes > i64::MAX as u64
                            || bucket_key(writer, &content.id) != key
                        {
                            return Err(Error::Integrity);
                        }
                        out.push(content);
                    }
                    if out.len() == limit as usize {
                        return Ok(out);
                    }
                    continue;
                }
                for content in decode_bucket(&key, &value)? {
                    if after.is_some_and(|a| content.id.to_string() <= a.to_string()) {
                        continue;
                    }
                    out.push(content);
                    if out.len() == limit as usize {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }
}





