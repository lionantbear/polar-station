//! In-process Host preparation and optional detached readonly native mount.
//! No path publication, async C callbacks or GC deletion.
//! Caller supplies files fetched/sealed by the existing CAS transport. One
//! registry must be shared by all users of a Host store; it is NOT a durable
//! substitute for Catalog projection references or cross-process GC ownership.
use crate::{
    projection::{cas_error, descriptor, verify_projection},
    verity::{self, VerityDigest, VerityPolicy},
    Image,
};
use polar_cas::{
    native_fs::{NativeProjection, VerifiedNativeProjection},
    projection::ProjectionTree,
    ContentRef, Error, ObjectId, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    os::unix::fs::FileExt,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct HostPins {
    counts: Arc<Mutex<BTreeMap<String, (ContentRef, u64)>>>,
}
struct Lease {
    owner: HostPins,
    contents: Vec<ContentRef>,
}
#[derive(Clone)]
pub struct PinLease(Arc<Lease>);
impl HostPins {
    pub fn pin(&self, contents: &[ContentRef]) -> Result<PinLease> {
        if contents.len() > polar_cas::native_fs::MAX_NATIVE_PACKS + 1 {
            return Err(Error::TooLarge);
        }
        let mut keys = BTreeSet::new();
        let mut unique = Vec::new();
        let mut counts = self
            .counts
            .lock()
            .map_err(|_| Error::Backend("pin registry poisoned".into()))?;
        for content in contents {
            if !keys.insert(content.id.to_string()) {
                if unique
                    .iter()
                    .any(|x: &ContentRef| x.id == content.id && x != content)
                {
                    return Err(Error::Integrity);
                }
                continue;
            }
            if let Some((known, n)) = counts.get(&content.id.to_string()) {
                if known != content {
                    return Err(Error::Integrity);
                }
                n.checked_add(1).ok_or(Error::TooLarge)?;
            }
            unique.push(content.clone());
        }
        // Only mutate after all validation, so failure never leaks partial pins.
        for content in &unique {
            counts
                .entry(content.id.to_string())
                .and_modify(|x| x.1 += 1)
                .or_insert((content.clone(), 1));
        }
        Ok(PinLease(Arc::new(Lease {
            owner: self.clone(),
            contents: unique,
        })))
    }
    pub fn is_pinned(&self, content: &ContentRef) -> Result<bool> {
        let counts = self
            .counts
            .lock()
            .map_err(|_| Error::Backend("pin registry poisoned".into()))?;
        match counts.get(&content.id.to_string()) {
            Some((known, _)) if known != content => Err(Error::Integrity),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }
}
impl PinLease {
    pub fn contains(&self, content: &ContentRef) -> bool {
        self.0.contents.contains(content)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        // A poisoned registry remains fail-closed; do not declare anything free.
        if let Ok(mut counts) = self.owner.counts.lock() {
            for content in &self.contents {
                let key = content.id.to_string();
                if let Some((_, count)) = counts.get_mut(&key) {
                    *count -= 1;
                    if *count == 0 {
                        counts.remove(&key);
                    }
                }
            }
        }
    }
}
struct Positional<'a> {
    file: &'a File,
    offset: u64,
}
impl Read for Positional<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let count = loop {
            match self.file.read_at(buf, self.offset) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        };
        self.offset = self
            .offset
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("file offset overflow"))?;
        Ok(count)
    }
}
/// Owns an immutable, content-verified O_RDONLY inode plus the live pin.
/// Fields are private: a serialized verity receipt cannot construct this type.
pub struct PreparedImage {
    file: File,
    content: ContentRef,
    verity: VerityDigest,
    _lease: PinLease,
}
impl PreparedImage {
    pub fn prepare(
        file: File,
        content: ContentRef,
        policy: VerityPolicy,
        lease: PinLease,
    ) -> Result<Self> {
        descriptor(&content)?;
        if !lease.contains(&content) {
            return Err(Error::Invalid("image not covered by Host pin".into()));
        }
        if file.metadata()?.len() != content.size_bytes {
            return Err(Error::Integrity);
        }
        let measured = verity::require(&file, policy).map_err(|e| Error::Backend(e.to_string()))?;
        // Verify AFTER enabling: verity now prevents writers from changing the
        // bytes between this full-file hash and later kernel consumption.
        let hash = ObjectId::for_reader(Positional {
            file: &file,
            offset: 0,
        })?;
        if hash != content.id || file.metadata()?.len() != content.size_bytes {
            return Err(Error::Integrity);
        }
        Ok(Self {
            file,
            content,
            verity: measured,
            _lease: lease,
        })
    }
    /// Duplicate the already verified inode and retain the SAME live lease.
    /// This does not accept a pathname, replace the backing or fabricate new
    /// verification. It lets a native view and a byte-reader share prepared P.
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            content: self.content.clone(),
            verity: self.verity.clone(),
            _lease: self._lease.clone(),
        })
    }
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn content(&self) -> &ContentRef {
        &self.content
    }
    pub fn verity_digest(&self) -> &VerityDigest {
        &self.verity
    }
}
/// Preparation is synchronous and must run on a blocking worker. No C image
/// handle crosses threads. Kernel activation is a separate privileged step.
pub struct PreparedNativeView {
    witness: VerifiedNativeProjection,
    meta: PreparedImage,
    packs: Vec<PreparedImage>,
}
impl PreparedNativeView {
    pub fn prepare(
        tree: &ProjectionTree,
        record: NativeProjection,
        meta: File,
        packs: Vec<File>,
        policy: VerityPolicy,
        pins: &HostPins,
    ) -> Result<Self> {
        record.validate()?;
        if packs.len() != record.packs.len() {
            return Err(Error::Integrity);
        }
        let lease = pins.pin(&record.images().cloned().collect::<Vec<_>>())?;
        let meta = PreparedImage::prepare(meta, record.meta.clone(), policy, lease.clone())?;
        let mut prepared = Vec::with_capacity(packs.len());
        for (file, expected) in packs.into_iter().zip(&record.packs) {
            prepared.push(PreparedImage::prepare(
                file,
                expected.clone(),
                policy,
                lease.clone(),
            )?);
        }
        let images: Vec<Image> = prepared
            .iter()
            .map(|p| {
                let expected = descriptor(p.content())?;
                Image::open(p.file(), &expected.sha256, expected.size_bytes).map_err(cas_error)
            })
            .collect::<Result<_>>()?;
        let refs: Vec<_> = images.iter().collect();
        let witness = verify_projection(tree, &record, meta.file(), &refs)?;
        Ok(Self {
            witness,
            meta,
            packs: prepared,
        })
    }
    pub fn record(&self) -> &NativeProjection {
        self.witness.record()
    }
    /// In-process proof retained alongside the immutable files and pins.
    pub fn verification(&self) -> &VerifiedNativeProjection {
        &self.witness
    }
    pub fn meta(&self) -> &PreparedImage {
        &self.meta
    }
    pub fn packs(&self) -> &[PreparedImage] {
        &self.packs
    }
}

/// RAII owner of an UNATTACHED kernel filesystem root. Drop closes the mount
/// before releasing M/P files and pins (field declaration order is deliberate).
/// A Host that later attaches or duplicates the root must retain this whole
/// owner until successful detach; path-publication/recovery is not provided.
pub struct DetachedNativeView {
    mount: std::os::fd::OwnedFd,
    prepared: PreparedNativeView,
}
impl PreparedNativeView {
    pub fn mount_detached(self, allow_exec: bool) -> Result<DetachedNativeView> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let fds: Vec<_> = self.packs.iter().map(|p| p.file().as_raw_fd()).collect();
        let mut mount_fd = -1;
        let mut os_error = 0;
        // SAFETY: all FDs are verified immutable members of this owned closure.
        // C only borrows inputs, returns a new owned FD, and never attaches it.
        let status = unsafe {
            polar_cas_fs_sys::pcfs_native_mount_detached(
                self.meta.file().as_raw_fd(),
                fds.as_ptr(),
                fds.len(),
                u32::from(allow_exec),
                &mut mount_fd,
                &mut os_error,
            )
        };
        if status != polar_cas_fs_sys::PCFS_OK {
            return Err(Error::Backend(format!(
                "native detached mount {status}: {}",
                std::io::Error::from_raw_os_error(os_error)
            )));
        }
        if mount_fd < 0 {
            return Err(Error::Integrity);
        }
        // SAFETY: native success transferred exactly one new owned descriptor.
        let mount = unsafe { std::os::fd::OwnedFd::from_raw_fd(mount_fd) };
        Ok(DetachedNativeView {
            mount,
            prepared: self,
        })
    }
}
impl DetachedNativeView {
    /// Borrowed root only; retain this owner across any I/O or Host handoff.
    pub fn root_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.mount.as_fd()
    }
    pub fn record(&self) -> &NativeProjection {
        self.prepared.record()
    }
}
