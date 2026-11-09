//! Group-shared local and selected remote physical positions. Pure lookup never
//! refreshes a store, accesses credentials, or promotes a location to Ready.
use polar_cas::{
    store_index::{IndexSelection, IndexSnapshot, StoreEntry, StoreRef},
    ContentRef, Error, ObjectId, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
#[cfg(unix)]
pub mod backup;
#[cfg(unix)]
pub(crate) mod head;
#[cfg(unix)]
pub(crate) use head::decode_current;
#[cfg(unix)]
mod local;
#[cfg(unix)]
mod persistent;
#[cfg(unix)]
mod projection;
#[cfg(unix)]
pub mod extensions;
#[cfg(unix)]
mod writers;
#[cfg(unix)]
pub use writers::{WriterPublication, WRITER_PUBLICATION_LOCK_TIMEOUT};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedLocation {
    /// Query-only provenance copied from the authoritative selected partition.
    /// Persisted SelectedLocation.scope remains the sole stored scope; a
    /// deserialized diagnostic candidate is never an access grant.
    #[serde(skip)]
    pub scope: Option<ObjectId>,
    pub store: StoreRef,
    pub index_root: ContentRef,
    pub generation: u64,
    pub received_at_ms: u64,
    pub entry: StoreEntry,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreCoverage {
    pub store: StoreRef,
    pub generation: u64,
    pub root: ContentRef,
    pub received_at_ms: u64,
    /// Complete for the supplied index snapshot, NEVER for the live bucket.
    pub indexed_entries: usize,
    pub selection: IndexSelection,
    pub scope: Option<ObjectId>,
}
#[derive(Clone, Debug)]
pub struct CatalogLookup {
    /// Exact immutable union used by this query; None on the legacy adapter.
    pub snapshot: Option<ContentRef>,
    pub candidates: Vec<IndexedLocation>,
    /// Locations relative to this Catalog's trusted shared root, not Store access grants.
    pub local: Vec<polar_cas::shared::LocatedContent>,
    pub coverage: Vec<StoreCoverage>,
    pub truncated: bool,
}
/// Domain-owned immutable query and original-selection roots, published together.
/// These references do not grant access to the selected documents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjection {
    pub query: ContentRef,
    pub selection: ContentRef,
}
#[derive(Clone)]
struct Attached {
    snapshot: Arc<IndexSnapshot>,
    root: ContentRef,
    received_at_ms: u64,
    selection: IndexSelection,
    scope: Option<ObjectId>,
}
#[derive(Clone, Default)]
struct State {
    #[cfg(unix)]
    writer_records: u64,
    stores: BTreeMap<String, Attached>,
    #[cfg(unix)]
    projections: BTreeMap<String, CatalogProjection>,
    #[cfg(unix)]
    local: polar_cas::shared::SharedIndexGeneration,
    #[cfg(unix)]
    index: Option<Arc<crate::shared_index::partitions::ValueSnapshot>>,
}
#[derive(Default)]
pub struct UnifiedCatalogIndex {
    /// Bounded retention of trusted, already-open query roots. Untrusted cursor
    /// references never become native-index deserialization authority.
    #[cfg(unix)]
    query_snapshots: std::sync::Mutex<std::collections::VecDeque<writers::RetainedSnapshot>>,
    /// Serialize publications by this shared owner; disk-head CAS still guards other owners.
    publication: std::sync::Mutex<()>,
    state: RwLock<Arc<State>>,
    #[cfg(unix)]
    storage: Option<crate::LocalCas>,
    #[cfg(unix)]
    head: Option<head::Head>,
}
impl UnifiedCatalogIndex {
    fn publication_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        let guard = self
            .publication
            .lock()
            .map_err(|_| Error::Backend("Catalog publication poisoned".into()))?;
        // The existing shared-store maintenance calls refresh even when idle.
        // Expired cursor leases must not wait for another user query to close.
        #[cfg(unix)]
        self.expire_idle_query_snapshots()?;
        Ok(guard)
    }

    /// Attributed saves require a durable shared head, not merely a writable
    /// snapshot cache. An in-memory index cannot acknowledge durable ownership.
    pub fn supports_writer_records(&self) -> bool {
        #[cfg(unix)]
        {
            self.storage.is_some() && self.head.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Match caller-owned local read authority to this index's exact trusted
    /// storage root. Index contents alone never grant storage access.
    #[cfg(unix)]
    pub fn matches_local_root(&self, root: &std::path::Path) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|storage| storage.root() == root)
    }

    #[cfg(unix)]
    pub(crate) fn accepts_local_writer(&self, root: &std::path::Path) -> bool {
        self.supports_writer_records()
            && self
                .storage
                .as_ref()
                .is_some_and(|storage| storage.root() == root)
    }

    /// Attach physical facts only. Caller-local access bindings never enter
    /// the shared snapshot. A Store UUID does not grant endpoint access.
    /// expected_previous guards concurrent install; monotonicity prevents an
    /// older generation or same-generation fork replacing the current one.
    pub fn attach_snapshot(
        &self,
        root: ContentRef,
        snapshot: IndexSnapshot,
        received_at_ms: u64,
        expected_previous: Option<&ContentRef>,
    ) -> Result<()> {
        self.attach_selected_snapshot(
            root,
            snapshot,
            received_at_ms,
            expected_previous,
            IndexSelection::Snapshot,
            None,
        )
    }
    /// Create this Store/scope selection only if its shared slot is absent.
    /// This primitive does not grant the caller permission to publish a selection.
    pub fn create_selected_snapshot(
        &self,
        root: ContentRef,
        snapshot: IndexSnapshot,
        received_at_ms: u64,
        selection: IndexSelection,
        scope: Option<ObjectId>,
    ) -> Result<()> {
        self.attach_selected_snapshot_inner(
            root,
            snapshot,
            received_at_ms,
            None,
            selection,
            scope,
            true,
        )
    }
    /// Selection and visibility come from the receiving Polar, never from the
    /// remote index. Replacement is atomic even when selection changes.
    #[allow(clippy::too_many_arguments)]
    pub fn attach_selected_snapshot(
        &self,
        root: ContentRef,
        snapshot: IndexSnapshot,
        received_at_ms: u64,
        expected_previous: Option<&ContentRef>,
        selection: IndexSelection,
        scope: Option<ObjectId>,
    ) -> Result<()> {
        self.attach_selected_snapshot_inner(
            root,
            snapshot,
            received_at_ms,
            expected_previous,
            selection,
            scope,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn attach_selected_snapshot_inner(
        &self,
        root: ContentRef,
        snapshot: IndexSnapshot,
        received_at_ms: u64,
        expected_previous: Option<&ContentRef>,
        selection: IndexSelection,
        scope: Option<ObjectId>,
        require_absent: bool,
    ) -> Result<()> {
        let _publication = self.publication_guard()?;
        if let Some(scope) = &scope {
            scope.require_sha256()?;
        }
        let snapshot = snapshot.select(&selection)?;
        let old = self.current()?;
        let mut next = (*old).clone();
        let stores = &mut next.stores;
        let key = attachment_key(snapshot.store(), scope.as_ref());
        match stores.get(&key) {
            Some(_) if require_absent => return Err(Error::Conflict),
            Some(old)
                if expected_previous.is_some_and(|e| e != &old.root)
                    || old.snapshot.store() != snapshot.store()
                    || snapshot.generation() < old.snapshot.generation()
                    || (snapshot.generation() == old.snapshot.generation() && root != old.root) =>
            {
                return Err(Error::Conflict);
            }
            None if expected_previous.is_some() => return Err(Error::Conflict),
            None if stores.len() >= 256 => return Err(Error::TooLarge),
            _ => {}
        }
        let total: usize = stores
            .iter()
            .filter(|(k, _)| *k != &key)
            .map(|(_, a)| a.snapshot.len())
            .sum();
        if total
            .checked_add(snapshot.len())
            .is_none_or(|n| n > 1_000_000)
        {
            return Err(Error::TooLarge);
        }
        stores.insert(
            key.clone(),
            Attached {
                root,
                snapshot: Arc::new(snapshot),
                received_at_ms,
                selection,
                scope,
            },
        );
        self.replace(&old, next, &key)
    }
    pub fn lookup(&self, content: &ContentRef, limit: usize) -> Result<CatalogLookup> {
        self.lookup_scoped(content, limit, None)
    }
    pub fn lookup_scoped(
        &self,
        content: &ContentRef,
        limit: usize,
        scope: Option<&ObjectId>,
    ) -> Result<CatalogLookup> {
        self.snapshot()?.lookup_scoped(content, limit, scope)
    }
    /// Pin the current in-memory snapshot. This method performs no filesystem I/O.
    pub fn snapshot(&self) -> Result<CatalogSnapshot> {
        Ok(CatalogSnapshot::new(self.current()?))
    }
    pub fn checkpoint(
        &self,
        store: &StoreRef,
    ) -> Result<Option<polar_cas::store_index::StoreIndexCheckpoint>> {
        self.checkpoint_scoped(store, None)
    }
    pub fn checkpoint_scoped(
        &self,
        store: &StoreRef,
        scope: Option<&ObjectId>,
    ) -> Result<Option<polar_cas::store_index::StoreIndexCheckpoint>> {
        let state = self.current()?;
        let stores = &state.stores;
        Ok(stores.get(&attachment_key(store, scope)).map(|a| {
            polar_cas::store_index::StoreIndexCheckpoint {
                root: a.root.clone(),
                generation: a.snapshot.generation(),
            }
        }))
    }
    /// Explicitly remove this group's selected source; shared hashes in other Stores
    /// or scopes remain discoverable. No remote files are removed.
    pub fn detach_snapshot(
        &self,
        store: &StoreRef,
        scope: Option<&ObjectId>,
        expected: &ContentRef,
    ) -> Result<()> {
        let _publication = self.publication_guard()?;
        let old = self.current()?;
        let mut next = (*old).clone();
        let stores = &mut next.stores;
        let key = attachment_key(store, scope);
        if stores.get(&key).is_none_or(|a| a.root != *expected) {
            return Err(Error::Conflict);
        }
        stores.remove(&key);
        self.replace(&old, next, &key)
    }
    fn current(&self) -> Result<Arc<State>> {
        Ok(self
            .state
            .read()
            .map_err(|_| Error::Backend("index lock poisoned".into()))?
            .clone())
    }
    /// Any instance may prepare maintenance for its selected local root. No
    /// permanent ownership is acquired; commit validates the shared disk head.
    #[cfg(unix)]
    pub fn compact_index(&self, stop: &std::sync::atomic::AtomicBool) -> Result<bool> {
        let check = || {
            if stop.load(std::sync::atomic::Ordering::Acquire) {
                Err(Error::Io(std::io::ErrorKind::Interrupted.into()))
            } else {
                Ok(())
            }
        };
        check()?;
        self.refresh()?;
        let _publication = self.publication_guard()?;
        let old = self.current()?;
        let Some(index) = &old.index else {
            return Ok(false);
        };
        let compact = index.compact()?;
        check()?;
        if compact.reference() == index.reference() {
            return Ok(false);
        }
        
        check()?;
        let mut next = (*old).clone();
        next.index = Some(Arc::new(compact));
        self.commit(&old, next)?;
        Ok(true)
    }

    fn replace(&self, old: &Arc<State>, mut next: State, changed_store: &str) -> Result<()> {
        #[cfg(unix)]
        if let Some(storage) = &self.storage {
            persistent::publish(storage, old, &mut next, changed_store)?;
        }
        #[cfg(not(unix))]
        let _ = (&mut next, changed_store);
        self.commit(old, next)
    }
    fn commit(&self, old: &Arc<State>, next: State) -> Result<()> {
        let mut current = self
            .state
            .write()
            .map_err(|_| Error::Backend("index lock poisoned".into()))?;
        if !Arc::ptr_eq(&current, old) {
            return Err(Error::Conflict);
        }
        #[cfg(unix)]
        if let Some(head) = &self.head {
            let _writer = head.writer().inspect_err(|error| {
                if matches!(error, Error::Conflict) {
                    conflict_diagnostics::record(
                        "publication_lock_contention",
                        old,
                        &next,
                        head.current().ok().flatten(),
                    );
                }
            })?;
            let expected = old.index.as_ref().map(|i| i.reference().clone());
            let actual = head.current()?;
            if actual != expected {
                conflict_diagnostics::record("current_head_mismatch", old, &next, actual);
                return Err(Error::Conflict);
            }
            head.publish(next.index.as_ref().ok_or(Error::Integrity)?.reference())?;
        }
        *current = Arc::new(next);
        Ok(())
    }
}

/// Best-effort conflict diagnostics: never change the typed production error,
/// or include paths, projection names, payloads, source locators or credentials.
#[cfg(unix)]
mod conflict_diagnostics {
    use super::*;
    
    pub(super) fn record(
        stage: &str,
        old: &State,
        next: &State,
        observed_head: Option<ContentRef>,
    ) {
        let changed = next
            .projections
            .iter()
            .filter(|(name, projection)| old.projections.get(*name) != Some(*projection))
            .count();
        let event = serde_json::json!({
            "stage": stage,
            "expected_head": old.index.as_ref().map(|index| index.reference().id.to_string()),
            "observed_head": observed_head.map(|root| root.id.to_string()),
            "changed_projection_count": changed,
            "expected_writer_records": old.writer_records,
            "next_writer_records": next.writer_records,
        });
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), "catalog_conflict {event}");
        
    }
    
}
fn attachment_key(store: &StoreRef, scope: Option<&ObjectId>) -> String {
    format!(
        "{}#{}",
        store.key(),
        scope.map(ToString::to_string).unwrap_or_default()
    )
}

/// Immutable query view, retained across group publications and refreshes.
#[derive(Clone)]
pub struct CatalogSnapshot(
    Arc<State>,
    #[cfg(unix)] Option<Arc<crate::shared_index::partitions::ValueSnapshot>>,
);
impl CatalogSnapshot {
    fn new(state: Arc<State>) -> Self {
        #[cfg(unix)]
        {
            Self(state.clone(), state.index.clone())
        }
        #[cfg(not(unix))]
        {
            Self(state)
        }
    }
}
impl CatalogSnapshot {
    /// Selected metadata from one immutable digest partition, never a read grant.
    /// Fail closed if the bounded candidate set cannot be represented completely.
    pub fn selected_candidates(
        &self,
        id: &ObjectId,
        scope: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<IndexedLocation>> {
        if limit == 0 || limit > 100_000 {
            return Err(Error::TooLarge);
        }
        if let Some(scope) = scope {
            scope.require_sha256()?;
        }
        let mut candidates = Vec::new();
        #[cfg(unix)]
        if self.0.index.is_some() {
            return persistent::selected_candidates(&self.0, id, scope, limit);
        }
        for attached in self.0.stores.values() {
            if attached.scope.is_some() && attached.scope.as_ref() != scope {
                continue;
            }
            for entry in attached.snapshot.candidates_for_id(id) {
                if candidates.len() == limit {
                    return Err(Error::TooLarge);
                }
                candidates.push(IndexedLocation {
                    scope: attached.scope.clone(),
                    store: attached.snapshot.store().clone(),
                    index_root: attached.root.clone(),
                    generation: attached.snapshot.generation(),
                    received_at_ms: attached.received_at_ms,
                    entry: entry.clone(),
                });
            }
        }
        Ok(candidates)
    }
    pub fn lookup_scoped(
        &self,
        content: &ContentRef,
        limit: usize,
        scope: Option<&ObjectId>,
    ) -> Result<CatalogLookup> {
        if let Some(scope) = scope {
            scope.require_sha256()?;
        }
        if limit == 0 || limit > 100_000 {
            return Err(Error::TooLarge);
        }
        let state = &self.0;
        let stores = &state.stores;
        #[cfg(unix)]
        if state.index.is_some() {
            return persistent::lookup(state, content, limit, scope);
        }
        let mut out = CatalogLookup {
            snapshot: None,
            candidates: Vec::new(),
            local: Vec::new(),
            coverage: Vec::new(),
            truncated: false,
        };
        for a in stores.values() {
            if a.scope.is_some() && a.scope.as_ref() != scope {
                continue;
            }
            out.coverage.push(StoreCoverage {
                store: a.snapshot.store().clone(),
                generation: a.snapshot.generation(),
                root: a.root.clone(),
                received_at_ms: a.received_at_ms,
                indexed_entries: a.snapshot.len(),
                selection: a.selection.clone(),
                scope: a.scope.clone(),
            });
            for entry in a.snapshot.candidates(content) {
                let entry = entry?;
                if out.candidates.len() == limit {
                    out.truncated = true;
                    break;
                }
                out.candidates.push(IndexedLocation {
                    scope: a.scope.clone(),
                    store: a.snapshot.store().clone(),
                    index_root: a.root.clone(),
                    generation: a.snapshot.generation(),
                    received_at_ms: a.received_at_ms,
                    entry: entry.clone(),
                });
            }
        }
        Ok(out)
    }
}




