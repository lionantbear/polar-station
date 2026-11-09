//! Per-Pack/loose admission records and the shared Catalog local view.
//! All files belong to the same explicitly trusted immutable storage domain.
pub mod binary;
mod format;
pub mod lifecycle;
mod navigation;
pub mod partitions;
pub(crate) mod position;
mod search;
pub mod values;
pub use search::{SearchIndex, SearchLayout};
mod loose;
pub use loose::ManagedCandidates;
mod receipt;
mod scan;
use crate::{
    admission::files::{open_read_file, open_read_handle},
    LocalCas,
};
use cap_std::{ambient_authority, fs::Dir};
use format::{Index, Record};
use polar_cas::{
    shared::{
        CompletePackIndex, IndexMaintenanceReceipt, PackIndexRef, PhysicalCandidate,
        SharedIndexGeneration,
    },
    ContentRef, DigestAlgorithm, Error, ObjectId, ObjectKeyLayout, Result,
};
pub use scan::LooseScanReceipt;
use std::{
    collections::BTreeMap,
    fs::{File, TryLockError},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const DIRECTORY: &str = ".polar-pack-index-v1";
const INDEX_CACHE_CAPACITY: usize = 64;
const MAX_PACKS: usize = 100_000;
const MERGE_FAN_IN: usize = 32;

type SnapshotCache = Option<(Option<ObjectId>, Arc<SharedIndexSnapshot>)>;

#[derive(Clone)]
pub struct SharedPackStore {
    store: LocalCas,
    dir: Arc<Dir>,
    path: PathBuf,
    snapshot: Arc<Mutex<SnapshotCache>>,
    indexes: Arc<Mutex<lru::LruCache<String, Arc<Index>>>>,
    scan: Arc<Mutex<Option<scan::LooseScan>>>,
    catalog: Arc<crate::catalog_index::UnifiedCatalogIndex>,
}

pub struct SharedIndexSnapshot {
    generation: SharedIndexGeneration,
    catalog: crate::catalog_index::CatalogSnapshot,
    covered_loose: BTreeMap<String, ObjectId>,
    covered: BTreeMap<String, ObjectId>,
}
impl SharedIndexSnapshot {
    /// Pure search within this immutable generation. No payload or directory I/O.
    pub fn locate(&self, id: &ObjectId) -> Result<Option<polar_cas::shared::LocatedContent>> {
        self.catalog.locate_local(id)
    }
    pub fn locate_batch(
        &self,
        ids: &[ObjectId],
    ) -> Result<Vec<Option<polar_cas::shared::LocatedContent>>> {
        if ids.len() > 1024 {
            return Err(Error::TooLarge);
        }
        let mut result = vec![None; ids.len()];
        self.locate_batch_into(ids, &mut result)?;
        Ok(result)
    }
    pub fn locate_batch_into(
        &self,
        ids: &[ObjectId],
        out: &mut [Option<polar_cas::shared::LocatedContent>],
    ) -> Result<()> {
        if ids.len() > 1024 || ids.len() != out.len() {
            return Err(Error::TooLarge);
        }
        self.catalog.locate_local_batch_into(ids, out)
    }
    /// Compatibility Pack-only metadata query. Ordinary unified search uses locate.
    pub fn lookup(&self, content: &ContentRef, budget: usize) -> Result<Vec<ContentRef>> {
        if budget == 0 || budget > MAX_PACKS {
            return Err(Error::TooLarge);
        }
        let mut found = Vec::new();
        for row in self.catalog.local_candidates(&content.id)? {
            if row.content != *content {
                return Err(Error::Integrity);
            }
            if let PhysicalCandidate::PackMember { pack } = row.location {
                if found.len() == budget {
                    break;
                }
                found.push(pack);
            }
        }
        Ok(found)
    }

    pub fn covered_packs(&self) -> usize {
        self.covered.len()
    }
    pub fn segments(&self) -> usize {
        self.catalog.partition_count()
    }
}

/// Holds the real per-Pack OS lock through payload commit and index publication.
pub struct PreparedPackIndex {
    store: SharedPackStore,
    reference: PackIndexRef,
    _guard: File,
}
impl PreparedPackIndex {
    pub fn reference(&self) -> &PackIndexRef {
        &self.reference
    }
    /// Only publishes when the ordinary immutable Pack body is durable. A
    /// failed call keeps the already persisted admission index for recovery.
    pub fn publish(self) -> Result<PackIndexRef> {
        self.store.finish(&self.reference)?;
        Ok(self.reference.clone())
    }
}

fn cancelled(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Relaxed) {
        Err(Error::Io(std::io::ErrorKind::Interrupted.into()))
    } else {
        Ok(())
    }
}
fn missing(error: &Error) -> bool {
    matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
        || matches!(error, Error::NotFound)
}

fn open_directory(store: &LocalCas) -> Result<Arc<Dir>> {
    if !matches!(store.key_layout, ObjectKeyLayout::SharedContent) {
        return Err(Error::UnsupportedCapability);
    }
    let mut created = match store.dir.create_dir(DIRECTORY) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.into()),
    };
    let fd = rustix::fs::openat(
        &*store.dir,
        DIRECTORY,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let dir = Arc::new(Dir::from_std_file(File::from(fd)));
    for name in ["packs", "pending", "loose", "locks", "tmp"] {
        match dir.create_dir(name) {
            Ok(()) => created = true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        let fd = rustix::fs::openat(
            &*dir,
            name,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let _checked_directory = File::from(fd);
    }
    if created {
        dir.open(".")?.into_std().sync_all()?;
        store.dir.open(".")?.into_std().sync_all()?;
    }
    Ok(dir)
}

fn lock_in(dir: &Arc<Dir>, name: &str) -> Result<File> {
    let key = format!("locks/{name}.lock");
    let fd = rustix::fs::openat(
        &**dir,
        key,
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .map_err(std::io::Error::from)?;
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(Error::Integrity);
    }
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Error::InProgress),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Acquire the existing physical-publication lock once. Contention and expired
/// caller deadlines return InProgress; neither acquisition nor Conflict retries.
pub(crate) fn save_publication_lock(
    store: &LocalCas,
    stop: &AtomicBool,
    deadline: std::time::Instant,
) -> Result<File> {
    cancelled(stop)?;
    if std::time::Instant::now() >= deadline {
        return Err(Error::InProgress);
    }
    let dir = open_directory(store)?;
    lock_in(&dir, "summary")
}



impl SharedPackStore {
    pub fn open(store: LocalCas) -> Result<Self> {
        let dir = open_directory(&store)?;
        let path = store.root.join(DIRECTORY);
        let catalog = crate::catalog_index::UnifiedCatalogIndex::open_shared(store.clone())?;
        Ok(Self {
            store,
            catalog,
            dir,
            path,
            snapshot: Default::default(),
            scan: Default::default(),
            indexes: Arc::new(Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(INDEX_CACHE_CAPACITY).unwrap(),
            ))),
        })
    }
    pub fn local_store(&self) -> &LocalCas {
        &self.store
    }
    pub fn root(&self) -> &Path {
        self.store.root()
    }

    fn open_file(&self, key: &str) -> Result<File> {
        Ok(open_read_file(&self.dir, Path::new(key))?.0)
    }
    fn sync(&self, directory: &str) -> Result<()> {
        self.dir.open(directory)?.into_std().sync_all()?;
        Ok(())
    }
    fn lock(&self, name: &str) -> Result<File> {
        lock_in(&self.dir, name)
    }
    fn temporary(&self) -> Result<tempfile::NamedTempFile> {
        Ok(tempfile::NamedTempFile::new_in(self.path.join("tmp"))?)
    }
    fn publish_file(&self, temp: &tempfile::NamedTempFile, key: &str) -> Result<()> {
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o444))?;
        temp.as_file().sync_all()?;
        let from = Dir::open_ambient_dir(
            temp.path().parent().ok_or(Error::Integrity)?,
            ambient_authority(),
        )?;
        match from.hard_link(
            temp.path().file_name().ok_or(Error::Integrity)?,
            &self.dir,
            key,
        ) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        self.sync(
            Path::new(key)
                .parent()
                .and_then(Path::to_str)
                .ok_or(Error::Integrity)?,
        )
    }
    fn index(&self, key: &str, expected: Option<&ObjectId>) -> Result<Arc<Index>> {
        let mut cache = self
            .indexes
            .lock()
            .map_err(|_| Error::Backend("shared index cache poisoned".into()))?;
        if let Some(index) = cache.get(key) {
            if expected.is_some_and(|id| id != &index.id) {
                return Err(Error::Integrity);
            }
            return Ok(index.clone());
        }
        let index = Arc::new(Index::open(&self.open_file(key)?, expected)?);
        cache.put(key.into(), index.clone());
        Ok(index)
    }
    fn listing(&self, directory: &str) -> Result<Vec<String>> {
        self.listing_except(directory, None)
    }
    fn listing_except(
        &self,
        directory: &str,
        covered: Option<&BTreeMap<String, ObjectId>>,
    ) -> Result<Vec<String>> {
        
        let mut result = Vec::new();
        for (count, entry) in self.dir.read_dir(directory)?.enumerate() {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Integrity)?;
            let Some(hex) = name.strip_suffix(".idx") else {
                return Err(Error::Integrity);
            };
            // Published keys have exactly 64 lowercase ASCII hex digits. Avoid
            // parsing an ObjectId and allocating/re-encoding it for every miss.
            if hex.len() != 64
                || !hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                // Preserve the existing invalid-identifier error for malformed keys.
                let id = ObjectId::try_from(format!("sha256:{hex}"))?;
                if id.hex() != hex {
                    return Err(Error::Integrity);
                }
            }
            if !entry.file_type()?.is_file() {
                return Err(Error::Integrity);
            }
            if count == MAX_PACKS {
                return Err(Error::TooLarge);
            }
            if covered.is_some_and(|covered| covered.contains_key(hex)) {
                continue;
            }
            result.push(name);
        }
        result.sort_unstable();
        Ok(result)
    }

    pub fn known_pack(&self, pack: &ContentRef) -> Result<bool> {
        let key = format!("packs/{}.idx", pack.id.hex());
        match self.index(&key, None) {
            Ok(index) => {
                if index.pack()? != pack {
                    return Err(Error::Integrity);
                }
                if index.is_receipt() {
                    self.receipt_snapshot(&index)?;
                }
                Ok(self.store.open_shared_file(pack)?.is_some())
            }
            Err(error) if missing(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub fn member_in_pack(&self, pack: &ContentRef, content: &ContentRef) -> Result<bool> {
        content.id.require_sha256()?;
        if !self.known_pack(pack)? {
            return Ok(false);
        }
        let index = self.index(&format!("packs/{}.idx", pack.id.hex()), None)?;
        if index.is_receipt() {
            return self.receipt_contains(&index, content);
        }
        let rows = index.find(content.id.digest_bytes(), 1)?;
        if rows.iter().any(|row| row.size != content.size_bytes) {
            return Err(Error::Integrity);
        }
        Ok(!rows.is_empty())
    }

    /// Prepare a complete trusted member index BEFORE publishing the payload.
    /// Busy returns InProgress; async callers retry without occupying a worker.
    pub fn prepare(
        &self,
        admission: &CompletePackIndex,
        stop: &AtomicBool,
    ) -> Result<PreparedPackIndex> {
        cancelled(stop)?;
        let _guard = self.lock(&admission.pack().id.hex())?;
        let final_key = format!("packs/{}.idx", admission.pack().id.hex());
        let reference = match self.index(&final_key, None) {
            Ok(index) => {
                if index.pack()? != admission.pack()
                    || index.count != admission.members().len() as u64
                {
                    return Err(Error::Integrity);
                }
                if index.is_receipt() {
                    self.receipt_snapshot(&index)?;
                }
                PackIndexRef {
                    pack: admission.pack().clone(),
                    index: index.id.clone(),
                }
            }
            Err(error) if missing(&error) => {
                let mut temp = self.temporary()?;
                let (id, _) = format::write_index(
                    temp.as_file_mut(),
                    Some(admission.pack()),
                    admission
                        .members()
                        .iter()
                        .map(|member| Record::new(member, admission.pack())),
                    stop,
                )?;
                let key = format!("pending/{}.idx", admission.pack().id.hex());
                self.publish_file(&temp, &key)?;
                let index = self.index(&key, Some(&id))?;
                if index.pack()? != admission.pack() {
                    return Err(Error::Integrity);
                }
                PackIndexRef {
                    pack: admission.pack().clone(),
                    index: id,
                }
            }
            Err(error) => return Err(error),
        };
        Ok(PreparedPackIndex {
            store: self.clone(),
            reference,
            _guard,
        })
    }
    fn finish(&self, reference: &PackIndexRef) -> Result<()> {
        let file = self
            .store
            .open_shared_file(&reference.pack)?
            .ok_or(Error::NotFound)?;
        file.sync_all()?;
        self.store.sync_parents(&reference.pack.id.content_key())?;
        let final_key = format!("packs/{}.idx", reference.pack.id.hex());
        match self.index(&final_key, Some(&reference.index)) {
            Ok(index) => {
                if index.pack()? != &reference.pack {
                    return Err(Error::Integrity);
                }
                self.sync("packs")?;
                self.sync(".")?;
                self.store.dir.open(".")?.into_std().sync_all()?;
                return Ok(());
            }
            Err(error) if missing(&error) => (),
            Err(error) => return Err(error),
        }
        let pending = format!("pending/{}.idx", reference.pack.id.hex());
        self.index(&pending, Some(&reference.index))?;
        match self.dir.hard_link(&pending, &self.dir, &final_key) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        self.index(&final_key, Some(&reference.index))?;
        self.sync("packs")?;
        self.sync(".")?;
        self.store.dir.open(".")?.into_std().sync_all()?;
        // Retain the complete list only until aggregate durably transfers its
        // members to Catalog; compact_members then removes the duplicate list.
        Ok(())
    }
    /// Explicit publication barrier for already durable contents. Physical checks
    /// occur here on the write side, never as a side effect of a search miss.
    pub fn publish_contents(&self, contents: &[ContentRef], stop: &AtomicBool) -> Result<()> {
        cancelled(stop)?;
        let _guard = self.lock("summary")?;
        let current = self.snapshot()?;
        let mut batches = Vec::new();
        for chunk in contents.chunks(4096) {
            let mut loose = Vec::new();
            for content in chunk {
                cancelled(stop)?;
                match current.locate(&content.id)? {
                    Some(found) if found.content.size_bytes != content.size_bytes => {
                        return Err(Error::Integrity);
                    }
                    Some(found) if found.location == PhysicalCandidate::Loose => continue,
                    Some(_) => {
                        if !self.store.contains_shared(content)? {
                            continue;
                        }
                    }
                    None => {
                        if !self.store.contains_shared(content)? {
                            return Err(Error::NotFound);
                        }
                    }
                }
                loose.push(content.clone());
            }
            batches.extend(
                self.publish_loose(&loose, stop)?
                    .into_iter()
                    .map(|id| format!("{}.idx", id.hex())),
            );
        }
        batches.sort_unstable();
        batches.dedup();
        if !batches.is_empty() {
            self.publish_catalog_local(stop, false, Some((Vec::new(), batches)), || {})?;
        }
        Ok(())
    }
    /// Publish only the selected durable Pack admission, without directory scans.
    pub fn publish_pack(&self, reference: &PackIndexRef, stop: &AtomicBool) -> Result<()> {
        cancelled(stop)?;
        let _guard = self.lock("summary")?;
        let index = self.index(
            &format!("packs/{}.idx", reference.pack.id.hex()),
            Some(&reference.index),
        )?;
        if index.pack()? != &reference.pack {
            return Err(Error::Integrity);
        }
        self.publish_catalog_local(
            stop,
            false,
            Some((vec![format!("{}.idx", reference.pack.id.hex())], Vec::new())),
            || {},
        )?;
        Ok(())
    }
    pub fn recover(&self, stop: &AtomicBool) -> Result<u64> {
        let mut recovered = 0;
        let snapshot = self.snapshot()?;
        for name in self.listing("pending")? {
            cancelled(stop)?;
            if snapshot.covered.contains_key(name.trim_end_matches(".idx")) {
                continue;
            }
            let _guard = match self.lock(name.trim_end_matches(".idx")) {
                Ok(v) => v,
                Err(Error::InProgress) => continue,
                Err(e) => return Err(e),
            };
            let index = match self.index(&format!("pending/{name}"), None) {
                Ok(index) => index,
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error),
            };
            let reference = PackIndexRef {
                pack: index.pack()?.clone(),
                index: index.id.clone(),
            };
            if reference.pack.id.hex() != name.trim_end_matches(".idx") {
                return Err(Error::Integrity);
            }
            match self.finish(&reference) {
                Ok(()) => recovered += 1,
                Err(e) if missing(&e) => (),
                Err(e) => return Err(e),
            }
        }
        match self.lock("summary") {
            Ok(_guard) => self.compact_members(self.snapshot()?.as_ref(), stop)?,
            Err(Error::InProgress) => (),
            Err(error) => return Err(error),
        }
        Ok(recovered)
    }
    /// Load or explicitly refresh the disk generation. Call outside search timing.
    pub fn snapshot(&self) -> Result<Arc<SharedIndexSnapshot>> {
        self.catalog.refresh()?;
        let catalog = self.catalog.snapshot()?;
        let root = catalog.root().map(|r| r.id.clone());
        let mut cache = self
            .snapshot
            .lock()
            .map_err(|_| Error::Backend("shared snapshot cache poisoned".into()))?;
        if let Some((old, snapshot)) = &*cache {
            if old == &root {
                return Ok(snapshot.clone());
            }
        }
        let generation = catalog.local_coverage().clone();
        if !generation.runs.is_empty()
            || !generation.loose_runs.is_empty()
            || generation.covered.len() > MAX_PACKS
            || generation.covered_loose.len() > MAX_PACKS
        {
            return Err(Error::Integrity);
        }
        let mut covered = BTreeMap::new();
        for reference in &generation.covered {
            reference.index.require_sha256()?;
            if covered
                .insert(reference.pack.id.hex(), reference.index.clone())
                .is_some()
            {
                return Err(Error::Integrity);
            }
        }
        let mut covered_loose = BTreeMap::new();
        for id in &generation.covered_loose {
            id.require_sha256()?;
            if covered_loose.insert(id.hex(), id.clone()).is_some() {
                return Err(Error::Integrity);
            }
        }
        let snapshot = Arc::new(SharedIndexSnapshot {
            generation,
            catalog,
            covered,
            covered_loose,
        });
        // Release cached old member-list mappings when observing the new head.
        // Pinned query snapshots use Catalog segments, never these staging files.
        {
            let mut indexes = self
                .indexes
                .lock()
                .map_err(|_| Error::Backend("shared index cache poisoned".into()))?;
            for hex in snapshot.covered.keys() {
                indexes.pop(&format!("packs/{hex}.idx"));
                indexes.pop(&format!("pending/{hex}.idx"));
            }
        }
        *cache = Some((root, snapshot.clone()));
        Ok(snapshot)
    }
    /// An already loaded view, with no filesystem work. The first acquisition
    /// loads the generation; runtime initialization performs that before search.
    pub fn search_snapshot(&self) -> Result<Arc<SharedIndexSnapshot>> {
        let view = self
            .snapshot
            .lock()
            .map_err(|_| Error::Backend("shared snapshot cache poisoned".into()))?
            .as_ref()
            .map(|(_, view)| view.clone());
        match view {
            Some(view) => Ok(view),
            None => self.snapshot(),
        }
    }
    /// Legacy Pack discovery including uncovered tails and backing validation.
    /// Used by explicit recovery/diagnostics, never by runtime content search.
    pub fn lookup(&self, content: &ContentRef, budget: usize) -> Result<Vec<ContentRef>> {
        if budget == 0 || budget > MAX_PACKS {
            return Err(Error::TooLarge);
        }
        content.id.require_sha256()?;
        let cached = self
            .snapshot
            .lock()
            .map_err(|_| Error::Backend("shared snapshot cache poisoned".into()))?
            .as_ref()
            .map(|(_, snapshot)| snapshot.clone());
        let mut cached_empty = false;
        if let Some(snapshot) = &cached {
            let candidates = snapshot.lookup(content, budget)?;
            cached_empty = candidates.is_empty();
            let mut found = Vec::new();
            for pack in candidates {
                if self.store.open_shared_file(&pack)?.is_some() {
                    found.push(pack);
                }
            }
            if !found.is_empty() {
                return Ok(found);
            }
        }
        let snapshot = self.snapshot()?;
        // Reuse only this request's miss after refreshing current. The live
        // uncovered listing still has to be checked even on the same snapshot.
        if cached_empty
            && cached
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &snapshot))
        {
            return self.lookup_uncovered(content, budget, &snapshot);
        }
        let candidates = snapshot.lookup(content, budget)?;
        let remaining = budget.saturating_sub(candidates.len());
        let mut found = Vec::new();
        for pack in candidates {
            if self.store.open_shared_file(&pack)?.is_some() {
                found.push(pack);
            }
        }
        if !found.is_empty() {
            return Ok(found);
        }
        self.lookup_uncovered(content, remaining, &snapshot)
    }
    pub fn lookup_uncovered(
        &self,
        content: &ContentRef,
        budget: usize,
        snapshot: &SharedIndexSnapshot,
    ) -> Result<Vec<ContentRef>> {
        let mut remaining = budget;
        let mut result = Vec::new();
        for (position, name) in self
            .listing_except("packs", Some(&snapshot.covered))?
            .into_iter()
            .enumerate()
        {
            let key = &name[..64];
            if remaining == 0 {
                return Err(Error::TooLarge);
            }
            remaining -= 1;
            let path = format!("packs/{name}");
            // Oversized sequential scans must not evict their own reusable
            // prefix. The remainder is still fully searched without retention.
            let index = if position < INDEX_CACHE_CAPACITY {
                self.index(&path, None)?
            } else {
                Arc::new(Index::open(&self.open_file(&path)?, None)?)
            };
            if index.pack()?.id.hex() != key {
                return Err(Error::Integrity);
            }
            if index.is_receipt() {
                if self.receipt_contains(&index, content)? {
                    result.push(index.pack()?.clone());
                }
                continue;
            }
            for record in index.find(content.id.digest_bytes(), 1)? {
                if record.size != content.size_bytes {
                    return Err(Error::Integrity);
                }
                result.push(record.image());
            }
        }
        Ok(result)
    }
    /// At most one writer builds a generation. Pack publishers never take this
    /// lock. A round admits at most 32 new Pack and loose indexes into the same
    /// Catalog partitions; repeat while added_packs or added_loose_batches is nonzero.
    pub fn aggregate(&self, stop: &AtomicBool) -> Result<IndexMaintenanceReceipt> {
        self.aggregate_with(stop, || {})
    }
    fn aggregate_with(
        &self,
        stop: &AtomicBool,
        after_snapshot: impl FnOnce(),
    ) -> Result<IndexMaintenanceReceipt> {
        cancelled(stop)?;
        let started = std::time::Instant::now();
        let _guard = self.lock("summary")?;
        let acquired = std::time::Instant::now();
        let mut receipt = self.aggregate_locked(stop, after_snapshot)?;
        receipt.lock_acquire_ns = acquired
            .duration_since(started)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        receipt.lock_hold_ns = acquired.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        Ok(receipt)
    }
    /// Explicit historical maintenance. Never called by publication or reads.
    pub fn compact(&self, stop: &AtomicBool) -> Result<()> {
        cancelled(stop)?;
        let _guard = self.lock("summary")?;
        self.catalog.compact_index(stop)?;
        self.compact_members(self.snapshot()?.as_ref(), stop)?;
        Ok(())
    }
    fn aggregate_locked(
        &self,
        stop: &AtomicBool,
        after_snapshot: impl FnOnce(),
    ) -> Result<IndexMaintenanceReceipt> {
        self.publish_catalog_local(stop, false, None, after_snapshot)
    }
    fn publish_catalog_local(
        &self,
        stop: &AtomicBool,
        reset: bool,
        selected: Option<(Vec<String>, Vec<String>)>,
        after_snapshot: impl FnOnce(),
    ) -> Result<IndexMaintenanceReceipt> {
        let old = self.snapshot()?;
        let mut generation = if reset {
            SharedIndexGeneration::default()
        } else {
            old.generation.clone()
        };
        generation.version = 4;
        let mut rows = Vec::new();
        let mut retained = BTreeMap::new();
        let registered: BTreeMap<_, _> = old
            .generation
            .covered
            .iter()
            .map(|reference| (reference.pack.id.hex(), reference))
            .collect();
        let mut added_packs = 0;
        let max = if reset { MAX_PACKS } else { MERGE_FAN_IN };
        let (packs, loose): (Vec<String>, Vec<String>) = match selected {
            Some((packs, loose)) => (
                packs
                    .into_iter()
                    .filter(|name| !old.covered.contains_key(name.trim_end_matches(".idx")))
                    .collect(),
                loose
                    .into_iter()
                    .filter(|name| {
                        !old.covered_loose
                            .contains_key(name.trim_end_matches(".idx"))
                    })
                    .collect(),
            ),
            None => (
                self.listing_except("packs", (!reset).then_some(&old.covered))?
                    .into_iter()
                    .take(max)
                    .collect(),
                self.listing_except("loose", (!reset).then_some(&old.covered_loose))?
                    .into_iter()
                    .take(max)
                    .collect(),
            ),
        };
        for name in packs {
            cancelled(stop)?;
            let index = self.index(&format!("packs/{name}"), None)?;
            if index.pack()?.id.hex() != name[..64] {
                return Err(Error::Integrity);
            }
            generation.covered.push(PackIndexRef {
                pack: index.pack()?.clone(),
                index: index.id.clone(),
            });
            if index.is_receipt() {
                let previous = registered.get(&name[..64]).ok_or(Error::Integrity)?;
                if !reset || previous.pack != *index.pack()? || previous.index != index.id {
                    return Err(Error::Integrity);
                }
                retained.insert(
                    index.pack()?.id.to_string(),
                    (index.pack()?.clone(), index.count, 0u64),
                );
                added_packs += 1;
                continue;
            }
            for at in 0..index.count {
                cancelled(stop)?;
                let row = index.row(at)?;
                rows.push(polar_cas::shared::LocatedContent {
                    content: ContentRef {
                        id: ObjectId::from_sha256(row.digest),
                        size_bytes: row.size,
                    },
                    location: PhysicalCandidate::PackMember { pack: row.image() },
                });
            }
            added_packs += 1;
        }
        if !retained.is_empty() {
            old.catalog.visit_local(|row| {
                cancelled(stop)?;
                if let PhysicalCandidate::PackMember { pack } = &row.location {
                    if let Some((expected, _, found)) = retained.get_mut(&pack.id.to_string()) {
                        if pack != expected {
                            return Err(Error::Integrity);
                        }
                        *found += 1;
                        rows.push(row);
                    }
                }
                Ok(())
            })?;
            if retained
                .values()
                .any(|(_, expected, found)| expected != found)
            {
                return Err(Error::Integrity);
            }
        }
        after_snapshot();
        let mut added_loose = 0;
        for name in loose {
            cancelled(stop)?;
            let id = ObjectId::try_from(format!("sha256:{}", &name[..64]))?;
            let index = self.index(&format!("loose/{name}"), Some(&id))?;
            if index.loose_algorithm.is_none() {
                return Err(Error::Integrity);
            }
            generation.covered_loose.push(id);
            for at in 0..index.count {
                cancelled(stop)?;
                rows.push(polar_cas::shared::LocatedContent {
                    content: index.loose_row(at)?,
                    location: PhysicalCandidate::Loose,
                });
            }
            added_loose += 1;
        }
        if added_packs == 0 && added_loose == 0 && !reset {
            return Ok(IndexMaintenanceReceipt {
                covered_packs: generation.covered.len() as u64,
                segments: old.segments() as u64,
                ..Default::default()
            });
        }
        generation
            .covered
            .sort_unstable_by(|a, b| a.pack.id.digest_bytes().cmp(b.pack.id.digest_bytes()));
        generation
            .covered_loose
            .sort_unstable_by(|a, b| a.digest_bytes().cmp(b.digest_bytes()));
        let mut receipt =
            self.catalog
                .publish_local(&old.catalog, generation, rows, reset, stop)?;
        receipt.added_packs = added_packs;
        receipt.added_loose_batches = added_loose;
        self.snapshot()?;
        Ok(receipt)
    }
    /// Rebuild from registered Catalog members and unaggregated admission lists.
    /// Preserve remote facts and old snapshots; never remove the shared head first.
    pub fn rebuild_summary(&self, stop: &AtomicBool) -> Result<()> {
        let _guard = self.lock("summary")?;
        self.publish_catalog_local(stop, true, None, || {})?;
        Ok(())
    }
    /// Explicit metadata audit, never part of normal trusted lookup.
    pub fn audit(&self) -> Result<()> {
        for name in self.listing("packs")? {
            let index = self.index(&format!("packs/{name}"), None)?;
            index.audit()?;
            if index.is_receipt() {
                self.receipt_snapshot(&index)?;
            }
        }
        for name in self.listing("loose")? {
            self.index(&format!("loose/{name}"), None)?.audit()?;
        }
        if let Some(root) = self.catalog.root()? {
            crate::catalog_index::UnifiedCatalogIndex::rebuild_portable(self.store.clone(), &root)?;
        }
        Ok(())
    }
}

impl LocalCas {
    /// Metadata-only open under the explicit SharedContent immutable contract.
    pub fn open_shared_file(&self, content: &ContentRef) -> Result<Option<File>> {
        Ok(self.open_shared_entry(content)?.map(|(file, _)| file))
    }
    /// Open an already indexed object in the exclusively managed namespace.
    /// Admission/audit owns type, length and mode checks, not ordinary opens.
    pub fn open_indexed_shared_reader(&self, id: &ObjectId) -> Result<Option<polar_cas::Reader>> {
        self.indexed_shared_reader(id, false)
    }
    /// Same open, but only if the kernel can resolve the path from cache.
    /// WouldBlock requests the existing background path; it is not a miss.
    pub fn try_open_cached_shared_reader(
        &self,
        id: &ObjectId,
    ) -> Result<Option<polar_cas::Reader>> {
        self.indexed_shared_reader(id, true)
    }
    fn indexed_shared_reader(
        &self,
        id: &ObjectId,
        cached_only: bool,
    ) -> Result<Option<polar_cas::Reader>> {
        if !matches!(self.key_layout, ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        match open_read_handle(&self.dir, Path::new(&id.content_key()), cached_only) {
            Ok(file) => Ok(Some(Box::pin(tokio::fs::File::from_std(file)))),
            Err(error) if missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn open_shared_entry(&self, content: &ContentRef) -> Result<Option<(File, std::fs::Metadata)>> {
        if !matches!(self.key_layout, ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        let (file, metadata) = match open_read_file(&self.dir, Path::new(&content.id.content_key()))
        {
            Ok(opened) => opened,
            Err(error) if missing(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file()
            || metadata.len() != content.size_bytes
            || metadata.mode() & 0o7022 != 0
        {
            return Err(Error::Integrity);
        }
        Ok(Some((file, metadata)))
    }
}


