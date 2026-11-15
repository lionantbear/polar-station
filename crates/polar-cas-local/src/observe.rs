//! A revalidated local directory observation, not an atomic snapshot.
//!
//! Native code owns only confined filesystem access. Content identity uses the
//! existing IdentifiedSources construction gate; namespace/links/selection use
//! existing Polar TreeManifest and ProjectionTree. No payload spool or Catalog
//! publication occurs in this module. Run it on a dedicated blocking worker.
use cap_std::fs::Dir;
use polar_cas::{
    manifest::bundle::BundleLimits,
    projection::{ProjectionEntry, ProjectionLayout, ProjectionNode, ProjectionTree},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};
use polar_cas_fs::{IdentifiedSources, IdentityLimits, PreparedSource};
use polar_cas_fs_sys as sys;
use std::{
    cell::Cell,
    ffi::{c_void, CStr},
    fs::File,
    io,
    marker::PhantomData,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{FileExt, MetadataExt},
    },
    path::{Component, Path},
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Clone, Copy, Debug)]
pub struct ObservationLimits {
    pub max_entries: u64,
    pub max_files: u64,
    pub max_depth: u32,
    pub max_directory_fds: u32,
    pub max_input_bytes: u64,
    pub max_path_bytes: u64,
}
impl Default for ObservationLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_files: 100_000,
            max_depth: 128,
            max_directory_fds: 130,
            max_input_bytes: 64 << 30,
            max_path_bytes: 64 << 20,
        }
    }
}

/// Only authoritative Tree types are retained. This is an input observation and
/// planning result, NOT a storage admission witness. Tin describes the whole
/// observed scope, including entries excluded from the selected Target Tree.
#[derive(Clone, Debug)]
pub struct ObservedTreePlan {
    input: ProjectionTree,
    target: ProjectionTree,
    source_contents: Vec<ContentRef>,
}
impl ObservedTreePlan {
    pub fn input(&self) -> &ProjectionTree {
        &self.input
    }
    pub fn target(&self) -> &ProjectionTree {
        &self.target
    }
    /// Ordered by native source token, not logical target path.
    pub fn source_contents(&self) -> &[ContentRef] {
        &self.source_contents
    }
}
struct CancelContext {
    cancelled: Arc<AtomicBool>,
}
unsafe extern "C" fn is_cancelled(context: *mut c_void) -> i32 {
    // SAFETY: the owning Box is retained until after tree_destroy; only an
    // atomic load is performed here and no Rust panic can cross the C ABI.
    let context = unsafe { &*context.cast::<CancelContext>() };
    i32::from(context.cancelled.load(Ordering::Relaxed))
}
pub(crate) fn native_status(status: i32) -> Result<()> {
    match status {
        sys::PCFS_OK => Ok(()),
        sys::PCFS_CORRUPT => Err(Error::SourceContentMismatch),
        sys::PCFS_LIMIT => Err(Error::TooLarge),
        sys::PCFS_NOT_FOUND => Err(Error::NotFound),
        sys::PCFS_UNSUPPORTED => Err(Error::UnsupportedCapability),
        sys::PCFS_CANCELLED => Err(Error::Io(io::Error::from(io::ErrorKind::Interrupted))),
        sys::PCFS_INVALID => Err(Error::Invalid("invalid local tree observation".into())),
        _ => Err(Error::Backend("local tree observation failed".into())),
    }
}
fn open_directory(root: &Dir, key: &str) -> Result<File> {
    if key.is_empty() {
        use rustix::fs::{openat, Mode, OFlags};
        return Ok(File::from(
            openat(
                root,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        ));
    }
    open_confined(root, key, true)
}
fn open_confined(root: &Dir, key: &str, directory: bool) -> Result<File> {
    use rustix::fs::{openat, Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(root, ".", flags, Mode::empty()).map_err(io::Error::from)?;
    let mut nonempty = false;
    let mut components = Path::new(key).components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(Error::Invalid(
                "local tree requires a confined source key".into(),
            ));
        };
        let flags = if !directory && components.peek().is_none() {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
        } else {
            flags
        };
        fd = openat(&fd, name, flags, Mode::empty()).map_err(io::Error::from)?;
        nonempty = true;
    }
    if !nonempty {
        return Err(Error::Invalid("empty local tree source key".into()));
    }
    Ok(File::from(fd))
}

/// Confined single-file observation. Reopening bounds descriptor use; identity
/// and encoded reads still go through IdentifiedSources, not metadata trust.
pub struct ObservedLocalFile {
    root: Arc<Dir>,
    key: String,
    stamp: std::fs::Metadata,
}
impl ObservedLocalFile {
    pub(crate) fn open(root: &Arc<Dir>, key: &str) -> Result<Self> {
        let stamp = open_confined(root, key, false)?.metadata()?;
        if !stamp.is_file() {
            return Err(Error::UnsupportedCapability);
        }
        Ok(Self {
            root: root.clone(),
            key: key.into(),
            stamp,
        })
    }
    fn file(&self) -> Result<File> {
        let file = open_confined(&self.root, &self.key, false)?;
        let now = file.metadata()?;
        let stamp = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mode(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if !now.is_file() || stamp(&now) != stamp(&self.stamp) {
            return Err(Error::SourceContentMismatch);
        }
        Ok(file)
    }
    pub fn revalidate(&self) -> Result<()> {
        self.file().map(|_| ())
    }
}
impl PreparedSource for ObservedLocalFile {
    fn size(&self) -> io::Result<u64> {
        Ok(self.stamp.len())
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        FileExt::read_at(&self.file().map_err(io::Error::other)?, buffer, offset)
    }
    fn open_readonly_fd(&self) -> io::Result<Option<OwnedFd>> {
        Ok(Some(self.file().map_err(io::Error::other)?.into()))
    }
}

/// Owns one directory capability and copied namespace metadata. May move to
/// another worker, but cannot be called concurrently (!Sync).
pub struct ObservedLocalTree {
    handle: NonNull<sys::pcfs_observed_tree>,
    context: Box<CancelContext>,
    binding_root: Arc<Dir>,
    binding_key: String,
    root_identity: (u64, u64),
    stats: sys::pcfs_tree_stats_v1,
    _not_sync: PhantomData<Cell<()>>,
}
// SAFETY: the native handle owns only Linux FDs, heap allocations, and a pointer
// into our stable Box. It has no thread-local state. The sole callback reads an
// Arc<AtomicBool>. Moving the sole owner is safe; Sync is intentionally absent.
unsafe impl Send for ObservedLocalTree {}
impl ObservedLocalTree {
    pub(crate) fn open_confined(
        root: &Arc<Dir>,
        key: &str,
        limits: ObservationLimits,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let file = open_directory(root, key)?;
        let stamp = file.metadata()?;
        let mut context = Box::new(CancelContext { cancelled: cancel });
        let callbacks = sys::pcfs_tree_cancel_v1 {
            context: (&mut *context as *mut CancelContext).cast(),
            is_cancelled: Some(is_cancelled),
        };
        let options = sys::pcfs_tree_options_v1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<sys::pcfs_tree_options_v1>() as u32,
            max_depth: limits.max_depth,
            max_directory_fds: limits.max_directory_fds,
            max_entries: limits.max_entries,
            max_sources: limits.max_files,
            max_input_bytes: limits.max_input_bytes,
            max_path_bytes: limits.max_path_bytes,
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: C duplicates file; on error it frees partial state. Context is
        // heap-stable and retained by the returned owner, never an external DTO.
        native_status(unsafe {
            sys::pcfs_tree_observe(file.as_raw_fd(), &options, &callbacks, &mut handle)
        })?;
        let handle = NonNull::new(handle).ok_or(Error::Integrity)?;
        let mut value = Self {
            handle,
            context,
            binding_root: Arc::clone(root),
            binding_key: key.to_owned(),
            root_identity: (stamp.dev(), stamp.ino()),
            stats: Default::default(),
            _not_sync: PhantomData,
        };
        // SAFETY: owned tree and correctly sized output; Drop handles errors.
        native_status(unsafe { sys::pcfs_tree_stats(handle.as_ptr(), &mut value.stats) })?;
        value.revalidate()?;
        Ok(value)
    }
    pub(crate) fn lock_kernel(
        &mut self,
        enable: bool,
    ) -> std::result::Result<sys::pcfs_tree_lock_report_v1, crate::locked_tree::DirectoryLockError>
    {
        let mut report = sys::pcfs_tree_lock_report_v1::default();
        // SAFETY: exclusive live native tree. No callback or handle escapes;
        // ENABLE is explicit and may irreversibly seal a prefix on failure.
        let status =
            unsafe { sys::pcfs_tree_lock_v1(self.handle.as_ptr(), u32::from(enable), &mut report) };
        if status != sys::PCFS_OK {
            return Err(crate::locked_tree::DirectoryLockError {
                status,
                os_error: report.os_error,
                checked_nodes: report.checked_nodes,
                changed_nodes: report.changed_nodes,
            });
        }
        Ok(report)
    }
    pub(crate) fn bound_to(&self, root: &Arc<Dir>, key: &str) -> bool {
        Arc::ptr_eq(&self.binding_root, root) && self.binding_key == key
    }
    pub fn file_count(&self) -> u64 {
        self.stats.file_count
    }
    pub fn entry_count(&self) -> u64 {
        self.stats.entry_count
    }
    pub fn namespace_walks(&self) -> u64 {
        self.stats.directory_walks
    }
    pub(crate) fn revalidate_binding(&self) -> Result<()> {
        // The native tree pins the old root even if its name is replaced. Check
        // the source Location/key binding as well before accepting an observation.
        let current = open_directory(&self.binding_root, &self.binding_key)?;
        let stamp = current.metadata()?;
        if self.root_identity != (stamp.dev(), stamp.ino()) {
            return Err(Error::SourceContentMismatch);
        }
        Ok(())
    }
    pub fn revalidate(&self) -> Result<()> {
        self.revalidate_binding()?;
        // SAFETY: no concurrent use because this owner is !Sync. Native state
        // mutation is confined behind the opaque foreign handle.
        native_status(unsafe { sys::pcfs_tree_revalidate(self.handle.as_ptr()) })
    }
    /// Native seal observations only; never re-read payload for a cold reopen.
    pub(crate) fn seal_witnesses(&self) -> Result<Vec<polar_cas::locked_directory::InodeWitness>> {
        use polar_cas::locked_directory::{InodeWitness, VerityWitness};
        self.revalidate_binding()?;
        let count = usize::try_from(self.entry_count()).map_err(|_| Error::TooLarge)?;
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            let mut output = std::mem::MaybeUninit::<sys::pcfs_tree_seal_entry_v1>::uninit();
            // SAFETY: live exclusively accessed observer; C initializes output
            // on success. It does not retain the pointer or read file payloads.
            native_status(unsafe {
                sys::pcfs_tree_seal_entry_v1(self.handle.as_ptr(), i, output.as_mut_ptr())
            })?;
            let w = unsafe { output.assume_init() };
            let verity = if w.kind == sys::PCFS_TREE_FILE {
                if w.verity.digest_size > 64 {
                    return Err(Error::Integrity);
                }
                let proof = VerityWitness {
                    algorithm: w.verity.algorithm,
                    digest: w.verity.digest[..w.verity.digest_size as usize]
                        .iter()
                        .map(|v| format!("{v:02x}"))
                        .collect::<String>(),
                };
                proof.validate()?;
                Some(proof)
            } else {
                if w.verity.algorithm != 0 || w.verity.digest_size != 0 {
                    return Err(Error::Integrity);
                }
                None
            };
            result.push(InodeWitness {
                device: w.device,
                inode: w.inode,
                link_count: w.link_count,
                size_bytes: w.size_bytes,
                mode: w.mode,
                uid: w.uid,
                gid: w.gid,
                mtime_seconds: w.mtime_seconds,
                mtime_nanos: w.mtime_nanos,
                ctime_seconds: w.ctime_seconds,
                ctime_nanos: w.ctime_nanos,
                verity,
            });
        }
        Ok(result)
    }
    pub(crate) fn open_entry_file(&self, index: usize) -> Result<File> {
        self.revalidate_binding()?;
        let entry = self.entry(index)?;
        if entry.kind != sys::PCFS_TREE_FILE {
            return Err(Error::Invalid("not a file member".into()));
        }
        // open_file checks the selected inode AND every ancestor. Do not walk
        // every sibling for every read; full namespace was checked at reopen.
        Ok(File::from(self.open_file(entry.source_token)?))
    }
    fn entry(&self, index: usize) -> Result<sys::pcfs_tree_entry_v1> {
        let mut output = std::mem::MaybeUninit::uninit();
        // SAFETY: C fills the whole entry on success; pointers remain borrowed
        // only while this tree owner lives and are copied by the planning code.
        native_status(unsafe {
            sys::pcfs_tree_entry(self.handle.as_ptr(), index, output.as_mut_ptr())
        })?;
        Ok(unsafe { output.assume_init() })
    }
    fn open_file(&self, token: u64) -> io::Result<OwnedFd> {
        let mut fd = -1;
        // SAFETY: C transfers exactly one new readonly FD on success; owner
        // retains the observation and cancellation context for the entire call.
        native_status(unsafe { sys::pcfs_tree_open_source(self.handle.as_ptr(), token, &mut fd) })
            .map_err(io::Error::other)?;
        if fd < 0 {
            return Err(io::Error::other("invalid source descriptor"));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
    /// Stage-boundary callback, never invoked from C. It may select source
    /// indices and build into PRIVATE staging using existing IdentifiedSources.
    /// Do not publish Source/Tree/CAS relationships inside it: final namespace
    /// revalidation must succeed first. No async/native handles escape this call.
    pub fn with_identified_tree<T>(
        &self,
        limits: IdentityLimits,
        layout: &ProjectionLayout,
        tree_limits: BundleLimits,
        build: impl FnOnce(&ObservedTreePlan, &mut IdentifiedSources<'_>) -> Result<T>,
    ) -> Result<(ObservedTreePlan, T)> {
        self.revalidate()?;
        let providers = self.file_sources()?;
        let refs: Vec<&dyn PreparedSource> =
            providers.iter().map(|p| p as &dyn PreparedSource).collect();
        let mut identities =
            IdentifiedSources::identify_cancellable(&refs, limits, &self.context.cancelled)
                .map_err(polar_cas_fs::projection::cas_error)?;
        self.revalidate()?;
        let plan = self.plan(&identities, layout, tree_limits)?;
        let value = build(&plan, &mut identities)?;
        drop(identities); // close borrowed source FDs before final revalidation
        self.revalidate()?;
        Ok((plan, value))
    }
    /// Source tokens are local to this observation and are not content evidence.
    pub fn file_sources(&self) -> Result<Vec<TreeFileSource<'_>>> {
        let count = usize::try_from(self.file_count()).map_err(|_| Error::TooLarge)?;
        let mut providers = Vec::with_capacity(count);
        for token in 0..count {
            let mut node = 0;
            // SAFETY: output is initialized and no pointer is retained by C.
            native_status(unsafe {
                sys::pcfs_tree_source_node(self.handle.as_ptr(), token as u64, &mut node)
            })?;
            let e = self.entry(node)?;
            providers.push(TreeFileSource {
                tree: self,
                token: token as u64,
                size: e.size_bytes,
            });
        }
        Ok(providers)
    }
    fn plan(
        &self,
        identities: &IdentifiedSources<'_>,
        layout: &ProjectionLayout,
        limits: BundleLimits,
    ) -> Result<ObservedTreePlan> {
        let mut contents = Vec::with_capacity(identities.len());
        for i in 0..identities.len() {
            let id = identities
                .identity(i)
                .map_err(polar_cas_fs::projection::cas_error)?;
            contents.push(ContentRef {
                id: ObjectId::from_sha256(id.sha256),
                size_bytes: id.size_bytes,
            });
        }
        let entries = self
            .source_entries()?
            .into_iter()
            .map(|entry| {
                Ok(ProjectionEntry {
                    path: entry.path,
                    node: entry.node.try_map_content(|index| {
                        contents.get(index).cloned().ok_or(Error::Integrity)
                    })?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let input = ProjectionTree::from_entries(entries, DigestAlgorithm::Sha256, limits)?;
        let target = ProjectionTree::plan(
            input.output().root(),
            input.output().directories(),
            layout,
            DigestAlgorithm::Sha256,
            limits,
        )?;
        Ok(ObservedTreePlan {
            input,
            target,
            source_contents: contents,
        })
    }
    /// Namespace metadata with file-token slots for composing multiple scopes.
    /// This does not scan again, read payload, or register any content.
    pub fn source_entries(&self) -> Result<Vec<ProjectionEntry<usize>>> {
        let count = usize::try_from(self.entry_count()).map_err(|_| Error::TooLarge)?;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let e = self.entry(i)?;
            // SAFETY: non-null NUL-terminated names are owned by this live tree.
            if e.path.is_null() {
                return Err(Error::Integrity);
            }
            let path = unsafe { CStr::from_ptr(e.path) }
                .to_str()
                .map_err(|_| Error::Invalid("source name is not UTF-8".into()))?
                .to_owned();
            let node = match e.kind {
                sys::PCFS_TREE_DIRECTORY => ProjectionNode::Directory,
                sys::PCFS_TREE_FILE => {
                    let content = usize::try_from(e.source_token).map_err(|_| Error::TooLarge)?;
                    ProjectionNode::File {
                        content,
                        executable: e.mode & 0o111 != 0,
                    }
                }
                sys::PCFS_TREE_SYMLINK => {
                    if e.symlink_target.is_null() {
                        return Err(Error::Integrity);
                    }
                    let target = unsafe { CStr::from_ptr(e.symlink_target) }
                        .to_str()
                        .map_err(|_| Error::Invalid("symlink is not UTF-8".into()))?
                        .to_owned();
                    ProjectionNode::Symlink { target }
                }
                _ => return Err(Error::UnsupportedCapability),
            };
            entries.push(ProjectionEntry { path, node });
        }
        Ok(entries)
    }
}
impl Drop for ObservedLocalTree {
    fn drop(&mut self) {
        // SAFETY: sole owner; release native state BEFORE the callback Box.
        unsafe { sys::pcfs_tree_destroy(self.handle.as_ptr()) };
    }
}
pub struct TreeFileSource<'a> {
    tree: &'a ObservedLocalTree,
    token: u64,
    size: u64,
}
impl PreparedSource for TreeFileSource<'_> {
    fn size(&self) -> io::Result<u64> {
        Ok(self.size)
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        let file = File::from(self.tree.open_file(self.token)?);
        FileExt::read_at(&file, buffer, offset)
    }
    fn open_readonly_fd(&self) -> io::Result<Option<OwnedFd>> {
        self.tree.open_file(self.token).map(Some)
    }
    fn is_cancelled(&self) -> bool {
        self.tree.context.cancelled.load(Ordering::Relaxed)
    }
}
