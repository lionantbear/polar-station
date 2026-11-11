//! Cross-process snapshot acquisition and root lifetime. A collector must hold
//! the acquisition file exclusively while probing roots, and preserve the full
//! closure of every current or leased root. Managed history collection preserves all unregistered roots.
use crate::LocalCas;
use polar_cas::{ContentRef, Error, Result};
use rustix::fs::{openat, Mode, OFlags};
use std::fs::File;

/// Shared during preparation/open, before the immutable root lease exists.
/// Publishers may prepare concurrently; maintenance cannot collect their inputs
/// or unpublished output while this guard is held.
fn guard_file(store: &LocalCas) -> Result<File> {
    store.dir.create_dir_all(".polar-index-artifacts-v1")?;
    let directory = openat(
        &*store.dir,
        ".polar-index-artifacts-v1",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let guard = File::from(
        openat(
            &directory,
            "lifecycle.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(std::io::Error::from)?,
    );
    if !guard.metadata()?.is_file() {
        return Err(Error::Integrity);
    }
    Ok(guard)
}

/// Caller holds acquire() until this lease is attached to the returned snapshot.
/// Reuse the root inode itself; no per-query or per-generation lease file.
pub(super) fn pin(store: &LocalCas, root: &ContentRef) -> Result<File> {
    let file = store.open_shared_file(root)?.ok_or(Error::NotFound)?;
    #[cfg(any(test, feature = "query-metrics"))]
    let started = std::time::Instant::now();
    file.lock_shared()?;
    #[cfg(any(test, feature = "query-metrics"))]
    super::partitions::write_metrics::record("pin_wait", started.elapsed(), 0, 0);
    Ok(file)
}

mod collect;
mod registry;
pub use collect::CollectionReceipt;
pub(crate) use registry::{keep, register_head, selecting};

pub(crate) fn acquire(store: &LocalCas) -> Result<File> {
    let guard = guard_file(store)?;
    #[cfg(any(test, feature = "query-metrics"))]
    let started = std::time::Instant::now();
    guard.lock_shared()?;
    #[cfg(any(test, feature = "query-metrics"))]
    super::partitions::write_metrics::record("acquire_wait", started.elapsed(), 0, 0);
    Ok(guard)
}


impl LocalCas {
    /// Maintenance only. Unknown roots retain their original lifetime; active
    /// readers and preparations take precedence over this best-effort attempt.
    pub fn collect_index_artifacts(&self) -> Result<CollectionReceipt> {
        ;
        if !matches!(self.key_layout, polar_cas::ObjectKeyLayout::SharedContent) {
            return Err(Error::UnsupportedCapability);
        }
        collect::collect(self)
    }
    pub fn retain_index_artifact(&self, reference: &ContentRef) -> Result<()> {
        let _acquisition = acquire(self)?;
        self.open_shared_file(reference)?.ok_or(Error::NotFound)?;
        keep(self, reference)
    }
}

/// The existing atomic Head, available to owners of generic ValueSnapshots.
/// A ContentRef alone does not pin managed history after maintenance; keep a
/// snapshot lease or explicitly retain it for durable historical selection.
pub struct ManagedIndexHead {
    store: LocalCas,
    head: crate::catalog_index::head::Head,
}
impl ManagedIndexHead {
    pub fn open(store: LocalCas, path: &std::path::Path) -> Result<Self> {
        let head = crate::catalog_index::head::Head::open_bound(&store, path)?;
        Ok(Self { store, head })
    }
    pub fn current(&self) -> Result<Option<ContentRef>> {
        self.head.current()
    }
    pub fn select(
        &self,
        expected: Option<&ContentRef>,
        next: &super::partitions::ValueSnapshot,
    ) -> Result<()> {
        let _acquisition = acquire(&self.store)?;
        let root = super::partitions::Root::read(&self.store, next.reference())?;
        // A snapshot from another physical store is not proof that this store
        // has its closure. Check presence without decoding or rehashing segments.
        for part in root.partitions.values() {
            self.store
                .open_shared_file(part.portable_file())?
                .ok_or(Error::NotFound)?;
            if part.storage.is_none() {
                self.store
                    .open_shared_file(&part.native)?
                    .ok_or(Error::NotFound)?;
            }
        }
        let _writer = self.head.writer()?;
        if self.head.current()?.as_ref() != expected {
            return Err(Error::Conflict);
        }
        self.head.publish(next.reference())
    }
}


fn directory(
    root: &cap_std::fs::Dir,
    path: impl AsRef<std::path::Path>,
) -> std::io::Result<cap_std::fs::Dir> {
    let mut dir = root.try_clone()?;
    for component in path.as_ref().components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        };
        let fd = openat(
            &dir,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        dir = cap_std::fs::Dir::from_std_file(File::from(fd));
    }
    Ok(dir)
}
