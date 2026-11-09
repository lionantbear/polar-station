//! Linux Host integration for Hybrid M. Providers can only be made from
//! verified content or immutable pack FDs, not arbitrary live CAS directories.
//! This module owns process-local lifetimes; it does not authorize global GC,
//! publish directory paths. Workspace recovery uses explicit persisted inode bindings.
use crate::{
    host::PreparedImage,
    hybrid_projection,
    projection::{cas_error, descriptor},
    Image,
};
use polar_cas::{
    hybrid_fs::{HybridCompilePlan, HybridProjection},
    projection::ProjectionTree,
    ContentRef, Error, Result,
};
use polar_cas_fs_sys as sys;
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd},
        unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn private_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    let m = fs::symlink_metadata(parent)?;
    if !m.is_dir() || m.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid(
            "Host generation parent must be private".into(),
        ));
    }
    for _ in 0..128 {
        let p = parent.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match DirBuilder::new().mode(0o700).create(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Conflict)
}
struct PrivateGeneration(PathBuf);
impl Drop for PrivateGeneration {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn owned_mount(status: i32, fd: i32, errno: i32) -> Result<File> {
    if status != 0 || fd < 0 {
        return Err(Error::Backend(format!(
            "mount status {status}, errno {errno}"
        )));
    }
    // SAFETY: successful native mount returns one uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn mount_image(image: &PreparedImage, devices: &[PreparedImage], exec: bool) -> Result<File> {
    let fds: Vec<_> = devices.iter().map(|p| p.file().as_raw_fd()).collect();
    let (mut fd, mut errno) = (-1, 0);
    // SAFETY: exact verified image FDs are retained by PreparedHybridView and
    // native code only borrows them while constructing the detached mount.
    let status = unsafe {
        sys::pcfs_native_mount_detached(
            image.file().as_raw_fd(),
            fds.as_ptr(),
            fds.len(),
            u32::from(exec),
            &mut fd,
            &mut errno,
        )
    };
    owned_mount(status, fd, errno)
}
fn canonical_required(contents: &[ContentRef]) -> Result<BTreeMap<String, ContentRef>> {
    let mut result = BTreeMap::new();
    if contents.len() > 1_000_000 {
        return Err(Error::TooLarge);
    }
    for c in contents {
        c.id.require_sha256()?;
        if let Some(old) = result.insert(c.id.to_string(), c.clone()) {
            if old != *c {
                return Err(Error::Integrity);
            }
        }
    }
    Ok(result)
}
enum ProviderBacking {
    Loose {
        _generation: PrivateGeneration,
        _contents: Vec<PreparedImage>,
    },
    Pack {
        _image: PreparedImage,
        checked_for: BTreeMap<String, ContentRef>,
    },
}
/// No raw-directory constructor: loose namespace must be frozen, and packed
/// namespaces must have their actual required paths decoded and verified.
pub struct ImmutableDataProvider {
    root: File,
    present: BTreeMap<String, ContentRef>,
    backing: ProviderBacking,
}
impl ImmutableDataProvider {
    /// Generates only the needed hash namespace. Data are hardlinked, never
    /// chmod'ed, copied, or unpacked. Source files must already have fs-verity.
    /// The private generation is removed only after the owning session is gone.
    pub fn freeze_loose(parent: &Path, mut contents: Vec<PreparedImage>) -> Result<Self> {
        if contents.len() > 1_000_000 {
            return Err(Error::TooLarge);
        }
        contents.sort_by_key(|c| c.content().id.to_string());
        let generation = PrivateGeneration(private_directory(parent, "cas-generation")?);
        let root = File::open(&generation.0)?;
        let mut present = BTreeMap::new();
        for c in &contents {
            let id = c.content().id.to_string();
            if let Some(previous) = present.get(&id) {
                if previous != c.content() {
                    return Err(Error::Integrity);
                }
                continue;
            }
            let expected = descriptor(c.content())?;
            let mut errno = 0;
            // SAFETY: the root is a new private generation; source has verified
            // digest/size after fs-verity and its immutable lease is retained.
            let status = unsafe {
                sys::pcfs_frozen_link_content(
                    root.as_raw_fd(),
                    c.file().as_raw_fd(),
                    expected.sha256.as_ptr(),
                    expected.size_bytes,
                    &mut errno,
                )
            };
            if status != 0 {
                return Err(Error::Backend(format!(
                    "freeze CAS content: status {status}, errno {errno}"
                )));
            }
            present.insert(id, c.content().clone());
        }
        // Do not change the shared regular inode's mode. No mutation API exists
        // after this constructor, so lower directory entries remain stable.
        Ok(Self {
            root,
            present,
            backing: ProviderBacking::Loose {
                _generation: generation,
                _contents: contents,
            },
        })
    }
    /// Validate every required key that is present, not just a declared subset:
    /// otherwise an earlier provider could shadow a later one with bad bytes.
    pub fn from_pack(image: PreparedImage, all_required: &[ContentRef]) -> Result<Self> {
        let checked_for = canonical_required(all_required)?;
        let mut present = BTreeMap::new();
        {
            let d = descriptor(image.content())?;
            let pack = Image::open(image.file(), &d.sha256, d.size_bytes).map_err(cas_error)?;
            for (id, c) in &checked_for {
                match pack.member(c.id.digest_bytes(), c.size_bytes) {
                    Ok(member) => {
                        member.verify().map_err(cas_error)?;
                        present.insert(id.clone(), c.clone());
                    }
                    Err(e) if e.status == sys::PCFS_NOT_FOUND => (),
                    Err(e) => return Err(cas_error(e)),
                }
            }
        }
        let root = mount_image(&image, &[], false)?;
        Ok(Self {
            root,
            present,
            backing: ProviderBacking::Pack {
                _image: image,
                checked_for,
            },
        })
    }
    fn check_scope(&self, required: &BTreeMap<String, ContentRef>) -> Result<()> {
        if let ProviderBacking::Pack { checked_for, .. } = &self.backing {
            if checked_for != required {
                return Err(Error::Invalid(
                    "packed provider was checked for another redirect scope".into(),
                ));
            }
        }
        Ok(())
    }
}
/// Persistent exclusive upper/work pair. Drop keeps user edits. Reopen requires
/// the exact Host-owned storage record; it never creates missing directories.
pub struct HybridWorkspace {
    root: PathBuf,
    root_fd: File,
    upper: File,
    work: File,
    _lock: File,
    identity: polar_cas::workspace::WorkspaceStorageRecord,
}
impl HybridWorkspace {
    pub fn create(parent: &Path) -> Result<Self> {
        let mut generation = PrivateGeneration(private_directory(parent, "cas-workspace")?);
        DirBuilder::new()
            .mode(0o700)
            .create(generation.0.join("upper"))?;
        DirBuilder::new()
            .mode(0o700)
            .create(generation.0.join("work"))?;
        let lock = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(generation.0.join("lease"))?;
        lock.try_lock()
            .map_err(|e| Error::Backend(format!("workspace lock: {e}")))?;
        let upper = File::open(generation.0.join("upper"))?;
        let work = File::open(generation.0.join("work"))?;
        let root_fd = File::open(&generation.0)?;
        upper.sync_all()?;
        work.sync_all()?;
        lock.sync_all()?;
        root_fd.sync_all()?;
        File::open(parent)?.sync_all()?;
        let root = std::mem::take(&mut generation.0);
        let identity = workspace::record(&root_fd, &upper, &work, &lock)?;
        Ok(Self {
            root,
            root_fd,
            upper,
            work,
            _lock: lock,
            identity,
        })
    }
    pub fn storage_root(&self) -> &Path {
        &self.root
    }
}
/// Verified M with exact native devices and a validated redirect requirement set.
pub struct PreparedHybridView {
    record: HybridProjection,
    meta: PreparedImage,
    packs: Vec<PreparedImage>,
    required: Vec<ContentRef>,
}
impl PreparedHybridView {
    /// Internal handoff from the complete transfer verification gate. No
    /// serialized claim can call this constructor or skip the codec checks.
    pub(crate) fn from_verified_parts(
        witness: &polar_cas::hybrid_fs::VerifiedHybridProjection,
        meta: PreparedImage,
        packs: Vec<PreparedImage>,
        required: Vec<ContentRef>,
    ) -> Result<Self> {
        let record = witness.record();
        if meta.content() != &record.meta
            || packs.len() != record.native_packs.len()
            || packs
                .iter()
                .zip(&record.native_packs)
                .any(|(p, r)| p.content() != r)
            || required.is_empty() == record.has_redirects()
        {
            return Err(Error::Integrity);
        }
        canonical_required(&required)?;
        Ok(Self {
            record: record.clone(),
            meta,
            packs,
            required,
        })
    }
    pub fn prepare(
        tree: &ProjectionTree,
        plan: &HybridCompilePlan,
        record: &HybridProjection,
        meta: PreparedImage,
        packs: Vec<PreparedImage>,
    ) -> Result<Self> {
        if meta.content() != &record.meta || packs.len() != record.native_packs.len() {
            return Err(Error::Integrity);
        }
        let mut images = Vec::with_capacity(packs.len());
        for (p, e) in packs.iter().zip(&record.native_packs) {
            if p.content() != e {
                return Err(Error::Integrity);
            }
            let d = descriptor(e)?;
            images.push(Image::open(p.file(), &d.sha256, d.size_bytes).map_err(cas_error)?);
        }
        let refs: Vec<_> = images.iter().collect();
        let required =
            hybrid_projection::redirect_requirements(tree, plan, record, meta.file(), &refs)?;
        drop(refs);
        drop(images);
        Ok(Self {
            record: record.clone(),
            meta,
            packs,
            required,
        })
    }
    /// Host receive path: the Tree and immutable M/P are sufficient; there is
    /// no need to transfer a redundant per-file mapping database.
    pub fn receive(
        tree: &ProjectionTree,
        record: &HybridProjection,
        meta: PreparedImage,
        packs: Vec<PreparedImage>,
    ) -> Result<Self> {
        if meta.content() != &record.meta || packs.len() != record.native_packs.len() {
            return Err(Error::Integrity);
        }
        let mut images = Vec::with_capacity(packs.len());
        for (p, e) in packs.iter().zip(&record.native_packs) {
            if p.content() != e {
                return Err(Error::Integrity);
            }
            let d = descriptor(e)?;
            images.push(Image::open(p.file(), &d.sha256, d.size_bytes).map_err(cas_error)?);
        }
        let refs: Vec<_> = images.iter().collect();
        let (_, required) = hybrid_projection::verify_received(tree, record, meta.file(), &refs)?;
        drop(refs);
        drop(images);
        Ok(Self {
            record: record.clone(),
            meta,
            packs,
            required,
        })
    }
    pub fn redirect_requirements(&self) -> &[ContentRef] {
        &self.required
    }
    pub fn record(&self) -> &HybridProjection {
        &self.record
    }
    pub fn activate(
        self,
        providers: Vec<ImmutableDataProvider>,
        workspace: Option<HybridWorkspace>,
        exec: bool,
    ) -> Result<HybridMountSession> {
        if providers.len() > 128 {
            return Err(Error::TooLarge);
        }
        let required = canonical_required(&self.required)?;
        for p in &providers {
            p.check_scope(&required)?;
        }
        for (id, content) in &required {
            let found = providers
                .iter()
                .find_map(|p| p.present.get(id))
                .ok_or(Error::MissingLocal)?;
            if found != content {
                return Err(Error::Integrity);
            }
        }
        // Zero redirects uses the native fast path; providers are not attached
        // merely because an unrelated local CAS directory happened to exist.
        if required.is_empty() && !providers.is_empty() {
            return Err(Error::Invalid("unused data providers".into()));
        }
        let lower = mount_image(&self.meta, &self.packs, exec)?;
        if required.is_empty() && workspace.is_none() {
            return Ok(HybridMountSession {
                mount: lower,
                _lower: None,
                _view: self,
                _providers: providers,
                workspace,
            });
        }
        let fds: Vec<_> = providers.iter().map(|p| p.root.as_raw_fd()).collect();
        let upper = workspace.as_ref().map_or(-1, |w| w.upper.as_raw_fd());
        let work = workspace.as_ref().map_or(-1, |w| w.work.as_raw_fd());
        let (mut fd, mut errno) = (-1, 0);
        // SAFETY: all providers are frozen/immutable and checked for this exact
        // requirement set. Session retains their roots, data pins and M/P.
        let status = unsafe {
            sys::pcfs_overlay_mount_detached(
                lower.as_raw_fd(),
                fds.as_ptr(),
                fds.len(),
                upper,
                work,
                u32::from(exec),
                &mut fd,
                &mut errno,
            )
        };
        let mount = owned_mount(status, fd, errno)?;
        Ok(HybridMountSession {
            mount,
            _lower: Some(lower),
            _view: self,
            _providers: providers,
            workspace,
        })
    }
}
/// Field order closes final mount before lower mounts, provider namespaces and
/// immutable pins. Do not duplicate/pass this FD without retaining the session.
pub struct HybridMountSession {
    mount: File,
    _lower: Option<File>,
    _view: PreparedHybridView,
    _providers: Vec<ImmutableDataProvider>,
    workspace: Option<HybridWorkspace>,
}
impl HybridMountSession {
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.mount.as_fd()
    }
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace.as_ref().map(|w| w.storage_root())
    }
}

mod workspace;
pub use workspace::WorkspaceMount;
