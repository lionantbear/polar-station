//! Fixed-head Shared Catalog backup and staged, portable-only restore. The host
//! supplies exclusive-root authorization and its existing source freeze guard.
use super::{persistent, CatalogProjection, CatalogSnapshot, State, UnifiedCatalogIndex};
use crate::{shared_index::partitions::ValueSnapshot, LocalCas};
use polar_cas::{
    backup_v2::*, catalog::backup::BackupInventory, shared::PhysicalCandidate, source::LocationId,
    verification::ContentVerifier, ContentRef, Error, ObjectId, Result,
};
use std::{collections::BTreeMap, io::Read, sync::Arc};

/// Domain-specific dependency extraction stays outside generic CAS. Unknown
/// projections must fail closed, not be copied as two opaque top-level roots.
pub trait ProjectionBackupProvider: Send + Sync {
    fn objects(
        &self,
        storage: &LocalCas,
        name: &str,
        projection: &CatalogProjection,
    ) -> Result<Vec<ContentRef>>;
    /// Rebuild query accelerators from checked portable/original data. Do not
    /// activate a domain or group head from inside this callback.
    fn rebuild(
        &self,
        storage: &LocalCas,
        name: &str,
        projection: &CatalogProjection,
        backed: &[ContentRef],
    ) -> Result<CatalogProjection>;
}
pub struct NoProjections;
impl ProjectionBackupProvider for NoProjections {
    fn objects(&self, _: &LocalCas, _: &str, _: &CatalogProjection) -> Result<Vec<ContentRef>> {
        Err(Error::UnsupportedCapability)
    }
    fn rebuild(
        &self,
        _: &LocalCas,
        _: &str,
        _: &CatalogProjection,
        _: &[ContentRef],
    ) -> Result<CatalogProjection> {
        Err(Error::UnsupportedCapability)
    }
}
#[derive(Clone, Copy)]
pub struct SharedBackupLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}
impl Default for SharedBackupLimits {
    fn default() -> Self {
        Self {
            max_files: 100_000,
            max_total_bytes: 1 << 40,
            max_file_bytes: 64 << 30,
        }
    }
}
impl SharedBackupLimits {
    fn validate(self) -> Result<()> {
        if self.max_files == 0
            || self.max_files > 1_000_000
            || self.max_total_bytes == 0
            || self.max_file_bytes == 0
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct PreparedSharedCatalogBackup {
    manifest: SharedCatalogBackup,
    storage: LocalCas,
    _snapshot: CatalogSnapshot,
}
impl PreparedSharedCatalogBackup {
    pub fn manifest(&self) -> &SharedCatalogBackup {
        &self.manifest
    }
    pub fn root_path(&self) -> &std::path::Path {
        self.storage.root()
    }
    pub fn open(&self, content: &ContentRef) -> Result<polar_cas::Reader> {
        if !self.manifest.contains(content) {
            return Err(Error::AccessMaterialUnavailable);
        }
        self.storage
            .open_shared_reader(content)?
            .ok_or(Error::NotFound)
    }
    pub fn cover_inventory(&self, inventory: Arc<dyn BackupInventory>) -> Arc<dyn BackupInventory> {
        Arc::new(CoveredInventory {
            base: inventory,
            shared: self.manifest.clone(),
        })
    }
}
struct CoveredInventory {
    base: Arc<dyn BackupInventory>,
    shared: SharedCatalogBackup,
}
impl BackupInventory for CoveredInventory {
    fn shared_catalog(&self) -> Option<&SharedCatalogBackup> {
        Some(&self.shared)
    }
    fn snapshot(&self) -> &polar_cas::backup_v2::CatalogSnapshot {
        self.base.snapshot()
    }
    fn summary(&self) -> Result<BackupInventorySummary> {
        self.base.summary()
    }
    fn locations(&self, limit: u32) -> Result<Vec<BackupLocation>> {
        self.base.locations(limit)
    }
    fn objects_page(
        &self,
        after: Option<&BackupCursor>,
        limit: u32,
    ) -> Result<BackupInventoryPage> {
        self.base.objects_page(after, limit)
    }
    fn placements(&self, content: &ContentRef, limit: u32) -> Result<Vec<BackupPlacement>> {
        self.base.placements(content, limit)
    }
    fn registered_file(
        &self,
        location: &LocationId,
        key: &str,
        id: &ObjectId,
        size: u64,
    ) -> Result<bool> {
        if location == &self.shared.source_location
            && key == id.content_key()
            && self.shared.contains(&ContentRef {
                id: id.clone(),
                size_bytes: size,
            })
        {
            return Ok(true);
        }
        self.base.registered_file(location, key, id, size)
    }
}

/// Non-deserializable proof that portable facts and all retained bytes were
/// checked and query accelerators rebuilt in THIS managed root before activation.
pub struct PreparedSharedCatalogRestore {
    storage: LocalCas,
    source: ContentRef,
    state: Arc<State>,
}
impl PreparedSharedCatalogRestore {
    pub fn source_root(&self) -> &ContentRef {
        &self.source
    }
    pub fn candidate_root(&self) -> &ContentRef {
        self.state
            .index
            .as_ref()
            .expect("validated restore index")
            .reference()
    }
    /// Protected local writer facts retain their original Store identities.
    /// The runtime must bind these explicitly; a restore never renames them.
    pub fn required_writer_stores(
        &self,
        location: &LocationId,
    ) -> Result<Vec<polar_cas::store_index::StoreRef>> {
        let mut stores = BTreeMap::new();
        self.state
            .index
            .as_ref()
            .ok_or(Error::Integrity)?
            .visit_records(|row| {
                if row.key.starts_with(super::writers::PREFIX) {
                    return Ok(());
                }
                for record in persistent::decode(&row.value)?.writers {
                    if &record.destination == location {
                        if let Some(store) = record.attribution.store {
                            if stores
                                .insert(store.key(), store.clone())
                                .is_some_and(|old| old != store)
                            {
                                return Err(Error::Integrity);
                            }
                        }
                    }
                }
                Ok(())
            })?;
        Ok(stores.into_values().collect())
    }
}

fn include(
    files: &mut BTreeMap<String, ContentRef>,
    reference: &ContentRef,
    limits: SharedBackupLimits,
) -> Result<()> {
    if reference.size_bytes > limits.max_file_bytes {
        return Err(Error::TooLarge);
    }
    let key = reference.id.to_string();
    if let Some(old) = files.get(&key) {
        if old != reference {
            return Err(Error::Integrity);
        }
    } else {
        if files.len() == limits.max_files {
            return Err(Error::TooLarge);
        }
        files.insert(key, reference.clone());
    }
    Ok(())
}
fn verify_file(storage: &LocalCas, content: &ContentRef, limits: SharedBackupLimits) -> Result<()> {
    if content.size_bytes > limits.max_file_bytes {
        return Err(Error::TooLarge);
    }
    let mut file = storage.open_shared_file(content)?.ok_or(Error::NotFound)?;
    let mut verifier = ContentVerifier::new(&[content.id.algorithm()])?;
    let mut buffer = [0u8; 64 << 10];
    let mut bytes = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or(Error::TooLarge)?;
        if bytes > content.size_bytes {
            return Err(Error::Integrity);
        }
        verifier.update(&buffer[..count])?;
    }
    if verifier.finish().reference(&content.id)? != *content {
        return Err(Error::Integrity);
    }
    Ok(())
}
fn physical_files(
    state: &State,
    storage: &LocalCas,
    source: &LocationId,
    files: &mut BTreeMap<String, ContentRef>,
    limits: SharedBackupLimits,
) -> Result<()> {
    let values = state.index.as_ref().ok_or(Error::NotFound)?;
    include(files, values.reference(), limits)?;
    for partition in values.partition_refs().values() {
        include(files, partition.portable_file(), limits)?;
    }
    values.visit_records(|row| {
        if row.key.starts_with(super::writers::PREFIX) {
            return Ok(());
        }
        let positions = persistent::decode(&row.value)?;
        for location in &positions.local {
            let physical = match &location.location {
                PhysicalCandidate::Loose => &location.content,
                PhysicalCandidate::PackMember { pack } => pack,
            };
            include(files, physical, limits)?;
        }
        for writer in &positions.writers {
            if storage.contains_shared(&writer.content)? {
                include(files, &writer.content, limits)?;
            } else if &writer.destination == source && positions.local.is_empty() {
                // A historical writer fact cannot replace missing protected bytes.
                return Err(Error::NotFound);
            }
        }
        Ok(())
    })?;
    for pack in &state.local.covered {
        include(files, &pack.pack, limits)?;
    }
    Ok(())
}

// Native accelerator bytes and fulltext segment IDs may differ after a cold
// rebuild. Idempotence compares complete portable facts + domain selections,
// never just the old root pointer or a digest supplied by the caller.
fn same_portable_state(a: &State, b: &State) -> Result<bool> {
    let (Some(a_index), Some(b_index)) = (&a.index, &b.index) else {
        return Ok(false);
    };
    if !a_index.same_records(b_index)?
        || a.projections.len() != b.projections.len()
        || a.projections.iter().any(|(name, p)| {
            b.projections
                .get(name)
                .is_none_or(|other| other.selection != p.selection)
        })
    {
        return Ok(false);
    }
    let mut a_metadata = persistent::metadata(a)?;
    let mut b_metadata = persistent::metadata(b)?;
    a_metadata
        .as_object_mut()
        .ok_or(Error::Integrity)?
        .remove("projections");
    b_metadata
        .as_object_mut()
        .ok_or(Error::Integrity)?
        .remove("projections");
    Ok(a_metadata == b_metadata)
}

impl UnifiedCatalogIndex {
    pub fn managed_root_path(&self) -> Result<&std::path::Path> {
        Ok(self
            .storage
            .as_ref()
            .ok_or(Error::UnsupportedCapability)?
            .root())
    }
    /// Explicit backup enumeration, never called from a lookup or implicit read.
    pub fn prepare_shared_backup(
        &self,
        source_location: LocationId,
        provider: &dyn ProjectionBackupProvider,
        limits: SharedBackupLimits,
    ) -> Result<PreparedSharedCatalogBackup> {
        limits.validate()?;
        if !self.supports_writer_records() {
            return Err(Error::UnsupportedCapability);
        }
        self.refresh()?;
        let storage = self.storage.clone().ok_or(Error::UnsupportedCapability)?;
        let state = self.current()?;
        let root = state
            .index
            .as_ref()
            .ok_or(Error::NotFound)?
            .reference()
            .clone();
        let mut files = BTreeMap::new();
        physical_files(&state, &storage, &source_location, &mut files, limits)?;
        let mut projections = BTreeMap::new();
        for (name, projection) in &state.projections {
            let mut objects = BTreeMap::new();
            for object in provider.objects(&storage, name, projection)? {
                include(&mut objects, &object, limits)?;
                include(&mut files, &object, limits)?;
            }
            projections.insert(
                name.clone(),
                SharedProjectionBackup {
                    query: projection.query.clone(),
                    selection: projection.selection.clone(),
                    objects: objects.into_values().collect(),
                },
            );
        }
        let manifest = SharedCatalogBackup {
            schema: "polar.cas.shared-catalog-backup.v1".into(),
            root,
            source_location,
            files: files.into_values().collect(),
            projections,
        };
        manifest.validate(limits.max_files, limits.max_total_bytes)?;
        for file in &manifest.files {
            verify_file(&storage, file, limits)?;
        }
        Ok(PreparedSharedCatalogBackup {
            manifest,
            storage,
            _snapshot: CatalogSnapshot::new(state),
        })
    }

    /// The transport must place immutable bytes into the admitted target before
    /// this call. Neither imported native accelerators nor the source head are activated.
    pub fn prepare_shared_restore(
        &self,
        manifest: &SharedCatalogBackup,
        provider: &dyn ProjectionBackupProvider,
        limits: SharedBackupLimits,
    ) -> Result<PreparedSharedCatalogRestore> {
        limits.validate()?;
        manifest.validate(limits.max_files, limits.max_total_bytes)?;
        let storage = self.storage.clone().ok_or(Error::UnsupportedCapability)?;
        if !self.supports_writer_records() {
            return Err(Error::UnsupportedCapability);
        }
        let _acquisition = crate::shared_index::lifecycle::acquire(&storage)?;
        for file in &manifest.files {
            verify_file(&storage, file, limits)?;
        }
        let imported = ValueSnapshot::rebuild_portable_with_local(
            &storage,
            &manifest.root,
            persistent::inline_local,
        )?;
        let decoded = Self::from_values(storage.clone(), imported)?;
        let mut state = (*decoded.current()?).clone();
        let mut required = BTreeMap::new();
        physical_files(
            &state,
            &storage,
            &manifest.source_location,
            &mut required,
            limits,
        )?;
        if required.values().any(|file| !manifest.contains(file)) {
            return Err(Error::Integrity);
        }
        if state.projections.len() != manifest.projections.len() {
            return Err(Error::Integrity);
        }
        for (name, projection) in &state.projections {
            let saved = manifest.projections.get(name).ok_or(Error::Integrity)?;
            if saved.query != projection.query || saved.selection != projection.selection {
                return Err(Error::Integrity);
            }
        }
        // Validate each domain's declared closure even when an equivalent head
        // is already active. Reuse the existing rebuild contract; generated
        // artifacts remain immutable and are not activated by the provider.
        for (name, projection) in &mut state.projections {
            let saved = manifest.projections.get(name).ok_or(Error::Integrity)?;
            let restored = provider.rebuild(&storage, name, projection, &saved.objects)?;
            if restored.selection != projection.selection {
                return Err(Error::Integrity);
            }
            *projection = restored;
        }
        // Cold retry may encounter the previously activated local rebuild. A
        // later writer mutation changes the portable facts and cannot pass this
        // comparison; it remains subject to the expected-head CAS below.
        self.refresh()?;
        let active = self.current()?;
        if same_portable_state(&state, &active)? {
            for (name, projection) in &active.projections {
                for file in provider.objects(&storage, name, projection)? {
                    verify_file(&storage, &file, limits)?;
                }
            }
            return Ok(PreparedSharedCatalogRestore {
                storage,
                source: manifest.root.clone(),
                state: active,
            });
        }
        // Re-encode the local root. rebuild_portable's source reference is not a
        // promise that its embedded foreign native refs can be trusted on reopen.
        state.index = Some(Arc::new(persistent::publish_values(
            &storage,
            state.index.as_deref(),
            &[],
            persistent::metadata(&state)?,
        )?));
        Ok(PreparedSharedCatalogRestore {
            storage,
            source: manifest.root.clone(),
            state: Arc::new(state),
        })
    }

    /// Compare-and-switch only after preparation. Retain the prepared candidate
    /// across an OutcomeUncertain result and explicitly invoke this method again.
    pub fn activate_shared_restore(
        &self,
        prepared: &PreparedSharedCatalogRestore,
        expected: Option<&ContentRef>,
    ) -> Result<ContentRef> {
        let storage = self.storage.as_ref().ok_or(Error::UnsupportedCapability)?;
        if storage.root() != prepared.storage.root() {
            return Err(Error::AccessMaterialUnavailable);
        }
        self.refresh()?;
        let _publication = self.publication_guard()?;
        let current = self.current()?;
        let root = current.index.as_ref().map(|i| i.reference());
        if root == Some(prepared.candidate_root()) {
            let head = self.head.as_ref().ok_or(Error::UnsupportedCapability)?;
            let _lock = head.writer()?;
            head.confirm(prepared.candidate_root())?;
            return Ok(prepared.candidate_root().clone());
        }
        if root != expected {
            return Err(Error::Conflict);
        }
        self.commit(&current, (*prepared.state).clone())?;
        Ok(prepared.candidate_root().clone())
    }
}

impl polar_cas::catalog::backup::SharedCatalogBackupSource for PreparedSharedCatalogBackup {
    fn descriptor(&self) -> &SharedCatalogBackup {
        self.manifest()
    }
    fn trusted_root(&self) -> &std::path::Path {
        self.root_path()
    }
    fn open(&self, content: &ContentRef) -> Result<polar_cas::Reader> {
        Self::open(self, content)
    }
    fn cover_inventory(&self, inventory: Arc<dyn BackupInventory>) -> Arc<dyn BackupInventory> {
        Self::cover_inventory(self, inventory)
    }
}


