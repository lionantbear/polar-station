//! First-ingest building blocks: no pre-existing ContentRef and no loose copy.
//!
//! Identification and encoded reads authenticate the same bytes. Temporary
//! block digests are bounded operation memory, never CAS chunk objects. These
//! APIs do not scan a filesystem namespace, admit a Pack, or compile its Tree.
use crate::{Compression, Error, PackResult, PreparedSource};
use polar_cas_fs_sys as sys;
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, IntoRawFd};
#[cfg(feature = "polar")]
use std::os::unix::fs::MetadataExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug)]
pub struct IdentityLimits {
    pub block_bytes: u32,
    pub max_sources: u64,
    pub max_input_bytes: u64,
    pub max_index_bytes: u64,
}
impl Default for IdentityLimits {
    fn default() -> Self {
        Self {
            block_bytes: 1 << 20,
            max_sources: 100_000,
            max_input_bytes: 64 << 30,
            max_index_bytes: 128 << 20,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct DirectPackOptions {
    pub compression: Compression,
    pub max_output_bytes: u64,
}
impl Default for DirectPackOptions {
    fn default() -> Self {
        Self {
            compression: Compression::Lz4,
            max_output_bytes: 64 << 30,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceReadStats {
    pub identity_bytes: u64,
    pub encoding_source_bytes: u64,
    pub delivered_bytes: u64,
    pub verified_blocks: u64,
    pub cache_hits: u64,
    pub source_opens: u64,
    pub peak_open_sources: u64,
    pub index_bytes: u64,
    pub cache_bytes: u64,
}
/// Returned only after the identified-source gate and actual encoder succeed.
/// This local result is NOT a CAS admission, immutable inode seal, signature or
/// authenticated report from a remote producer. The caller must still publish
/// the exact output using the existing verified CAS storage boundary.
#[derive(Debug)]
pub struct PackConstruction {
    pack: PackResult,
    members_digest: [u8; 32],
    reads: SourceReadStats,
    #[cfg(feature = "polar")]
    output: GeneratedIdentity,
}
/// Exact encoder output inode. This is provenance, not an immutability claim.
#[cfg(feature = "polar")]
#[derive(Debug)]
struct GeneratedIdentity {
    dev: u64,
    ino: u64,
    size: u64,
    uid: u32,
    gid: u32,
    mtime: i64,
    mtime_nsec: i64,
}
#[cfg(feature = "polar")]
impl GeneratedIdentity {
    fn capture(file: &File) -> Result<Self, Error> {
        let m = file.metadata().map_err(|_| Error {
            status: sys::PCFS_IO,
        })?;
        if !m.is_file() {
            return Err(Error {
                status: sys::PCFS_CORRUPT,
            });
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            uid: m.uid(),
            gid: m.gid(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        })
    }
    fn matches(&self, m: &std::fs::Metadata) -> bool {
        m.is_file()
            && self.dev == m.dev()
            && self.ino == m.ino()
            && self.size == m.len()
            && self.uid == m.uid()
            && self.gid == m.gid()
            && self.mtime == m.mtime()
            && self.mtime_nsec == m.mtime_nsec()
    }
}
impl PackConstruction {
    pub fn pack(&self) -> &PackResult {
        &self.pack
    }
    pub fn members_digest(&self) -> &[u8; 32] {
        &self.members_digest
    }
    pub fn reads(&self) -> &SourceReadStats {
        &self.reads
    }
    #[cfg(feature = "polar")]
    pub(crate) fn matches_generated_inode(&self, metadata: &std::fs::Metadata) -> bool {
        self.output.matches(metadata)
    }
}
struct Context<'a> {
    sources: Vec<&'a dyn PreparedSource>,
    cancelled: Option<&'a AtomicBool>,
}
fn check(status: i32) -> Result<(), Error> {
    if status == sys::PCFS_OK {
        Ok(())
    } else {
        Err(Error { status })
    }
}
unsafe extern "C" fn open_source(ctx: *mut c_void, token: u64, fd: *mut i32) -> i32 {
    // SAFETY: C borrows this boxed Context synchronously while its owner lives.
    let ctx = unsafe { &*ctx.cast::<Context<'_>>() };
    let Some(source) = usize::try_from(token).ok().and_then(|i| ctx.sources.get(i)) else {
        return sys::PCFS_INVALID;
    };
    match catch_unwind(AssertUnwindSafe(|| {
        if source.is_cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        source.open_readonly_fd()
    })) {
        Ok(Ok(Some(owned))) => {
            // SAFETY: C passes a valid initialized out-FD and assumes ownership.
            unsafe { *fd = owned.into_raw_fd() };
            sys::PCFS_OK
        }
        Ok(Ok(None)) => sys::PCFS_NO_FD,
        Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => sys::PCFS_CANCELLED,
        _ => sys::PCFS_IO,
    }
}
unsafe extern "C" fn read_source(
    ctx: *mut c_void,
    token: u64,
    offset: u64,
    buffer: *mut u8,
    capacity: usize,
    got: *mut usize,
) -> i32 {
    // SAFETY: boxed callback context is retained until destroy, before its Drop.
    let ctx = unsafe { &*ctx.cast::<Context<'_>>() };
    let Some(source) = usize::try_from(token).ok().and_then(|i| ctx.sources.get(i)) else {
        return sys::PCFS_INVALID;
    };
    // SAFETY: nonempty C scratch buffers are valid and not retained past call.
    let bytes = if capacity == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(buffer, capacity) }
    };
    match catch_unwind(AssertUnwindSafe(|| {
        if source.is_cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        source.read_at(offset, bytes)
    })) {
        Ok(Ok(n)) if n <= capacity => {
            unsafe { *got = n };
            sys::PCFS_OK
        }
        Ok(Ok(_)) => sys::PCFS_INVALID,
        Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => sys::PCFS_CANCELLED,
        _ => sys::PCFS_IO,
    }
}
unsafe extern "C" fn cancelled(ctx: *mut c_void) -> i32 {
    // SAFETY: same live context. No per-read O(number-of-files) provider walk.
    let ctx = unsafe { &*ctx.cast::<Context<'_>>() };
    i32::from(ctx.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)))
}
/// Single-owner temporary input capability. Mutation cannot change the expected
/// identities; source bytes are checked on every cache fill before delivery.
/// Construct/use/drop the entire set within one blocking worker. !Send/!Sync
/// intentionally avoid sharing non-thread-safe original providers.
pub struct IdentifiedSources<'a> {
    handle: NonNull<sys::pcfs_verified_sources>,
    _context: Box<Context<'a>>,
    count: usize,
    limits: IdentityLimits,
    _single_owner: PhantomData<Rc<()>>,
}
impl<'a> IdentifiedSources<'a> {
    pub fn identify(
        sources: &[&'a dyn PreparedSource],
        limits: IdentityLimits,
    ) -> Result<Self, Error> {
        Self::identify_inner(sources, limits, None)
    }
    pub fn identify_cancellable(
        sources: &[&'a dyn PreparedSource],
        limits: IdentityLimits,
        cancel: &'a AtomicBool,
    ) -> Result<Self, Error> {
        Self::identify_inner(sources, limits, Some(cancel))
    }
    fn identify_inner(
        sources: &[&'a dyn PreparedSource],
        limits: IdentityLimits,
        cancel: Option<&'a AtomicBool>,
    ) -> Result<Self, Error> {
        if sources.len() as u64 > limits.max_sources || limits.max_sources > 1_000_000 {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let mut specs = Vec::with_capacity(sources.len());
        for (index, source) in sources.iter().enumerate() {
            let size = source.size().map_err(|_| Error {
                status: sys::PCFS_IO,
            })?;
            specs.push(sys::pcfs_source_spec_v1 {
                source_token: index as u64,
                size_bytes: size,
            });
        }
        let mut context = Box::new(Context {
            sources: sources.to_vec(),
            cancelled: cancel,
        });
        let provider = sys::pcfs_sources_v1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<sys::pcfs_sources_v1>() as u32,
            context: (&mut *context as *mut Context<'a>).cast(),
            open_fd: Some(open_source),
            read_at: Some(read_source),
            is_cancelled: Some(cancelled),
        };
        let options = sys::pcfs_verify_options_v1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<sys::pcfs_verify_options_v1>() as u32,
            block_bytes: limits.block_bytes,
            reserved: 0,
            max_sources: limits.max_sources,
            max_input_bytes: limits.max_input_bytes,
            max_index_bytes: limits.max_index_bytes,
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: C copies provider callbacks but not the context. The Box is
        // stored here on success; C destroys partial state before an error.
        check(unsafe {
            sys::pcfs_verified_sources_identify(
                &options,
                &provider,
                specs.as_ptr(),
                specs.len(),
                &mut handle,
            )
        })?;
        Ok(Self {
            handle: NonNull::new(handle).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            _context: context,
            count: sources.len(),
            limits,
            _single_owner: PhantomData,
        })
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn identity(&self, index: usize) -> Result<SourceIdentity, Error> {
        let mut id = sys::pcfs_source_identity_v1::default();
        // SAFETY: reads a single owned immutable identity; no pointer escapes.
        check(unsafe {
            sys::pcfs_verified_sources_identity(self.handle.as_ptr(), index, &mut id)
        })?;
        Ok(SourceIdentity {
            sha256: id.sha256,
            size_bytes: id.size_bytes,
        })
    }
    pub fn read_at(&mut self, index: usize, offset: u64, bytes: &mut [u8]) -> Result<usize, Error> {
        let mut got = 0;
        // SAFETY: exclusive handle; C verifies its scratch before writing bytes.
        check(unsafe {
            sys::pcfs_verified_sources_pread(
                self.handle.as_ptr(),
                index,
                offset,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut got,
            )
        })?;
        Ok(got)
    }
    pub fn stats(&self) -> Result<SourceReadStats, Error> {
        let mut s = sys::pcfs_verified_stats_v1::default();
        // SAFETY: owned handle and correctly sized output.
        check(unsafe { sys::pcfs_verified_sources_stats(self.handle.as_ptr(), &mut s) })?;
        Ok(SourceReadStats {
            identity_bytes: s.identity_bytes,
            encoding_source_bytes: s.encoding_source_bytes,
            delivered_bytes: s.delivered_bytes,
            verified_blocks: s.verified_blocks,
            cache_hits: s.cache_hits,
            source_opens: s.source_opens,
            peak_open_sources: s.peak_open_sources,
            index_bytes: s.index_bytes,
            cache_bytes: s.cache_bytes,
        })
    }
    /// Encode a standalone ordinary-path filesystem using this existing
    /// identity pass. This produces bytes, not a CAS admission or inode seal.
    pub fn build_filesystem(
        &mut self,
        entries: &[crate::FilesystemEntry],
        output: &mut File,
        options: crate::FilesystemPackOptions,
    ) -> Result<crate::FilesystemPackResult, Error> {
        // SAFETY: exclusive live verified-source handle; callbacks/context live
        // until this synchronous call finishes, and no raw handle escapes.
        unsafe {
            crate::filesystem_pack::build_identified(self.handle.as_ptr(), entries, output, options)
        }
    }
    pub fn build(
        &mut self,
        output: &mut File,
        options: DirectPackOptions,
    ) -> Result<PackConstruction, Error> {
        self.build_inner(None, output, options)
    }
    /// Reuse the identity pass after a CAS lookup; pack only new source indices.
    pub fn build_selected(
        &mut self,
        indices: &[usize],
        output: &mut File,
        options: DirectPackOptions,
    ) -> Result<PackConstruction, Error> {
        self.build_inner(Some(indices), output, options)
    }
    fn build_inner(
        &mut self,
        indices: Option<&[usize]>,
        output: &mut File,
        options: DirectPackOptions,
    ) -> Result<PackConstruction, Error> {
        let native = sys::pcfs_pack_options_v1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<sys::pcfs_pack_options_v1>() as u32,
            profile: sys::PCFS_EROFS_PROFILE_BUILD_V1,
            compression: match options.compression {
                Compression::None => 0,
                Compression::Lz4 => 1,
            },
            max_output_bytes: options.max_output_bytes,
            max_input_bytes: self.limits.max_input_bytes,
            max_objects: self.limits.max_sources,
        };
        let mut result = std::mem::MaybeUninit::<sys::pcfs_pack_build_result_v2>::uninit();
        // SAFETY: C zeros then fills every field. Success requires the opaque
        // verified source set, not caller-provided ContentRefs or a skip flag.
        let status = unsafe {
            match indices {
                None => sys::pcfs_erofs_build_verified(
                    &native,
                    self.handle.as_ptr(),
                    output.as_raw_fd(),
                    result.as_mut_ptr(),
                ),
                Some(selected) => sys::pcfs_erofs_build_verified_selected(
                    &native,
                    self.handle.as_ptr(),
                    selected.as_ptr(),
                    selected.len(),
                    output.as_raw_fd(),
                    result.as_mut_ptr(),
                ),
            }
        };
        check(status)?;
        let result = unsafe { result.assume_init() };
        let mut reads = self.stats()?;
        // Per-build I/O, rather than totals from previous diagnostic reads.
        reads.identity_bytes = result.identity_bytes;
        reads.encoding_source_bytes = result.encoding_source_bytes;
        reads.delivered_bytes = result.delivered_bytes;
        reads.verified_blocks = result.verified_blocks;
        reads.cache_hits = result.cache_hits;
        #[cfg(feature = "polar")]
        let output = GeneratedIdentity::capture(output)?;
        #[cfg(feature = "polar")]
        if output.size != result.pack.size_bytes {
            return Err(Error {
                status: sys::PCFS_CORRUPT,
            });
        }
        Ok(PackConstruction {
            pack: PackResult {
                sha256: result.pack.sha256,
                size_bytes: result.pack.size_bytes,
                unique_objects: result.pack.unique_objects,
                input_bytes: result.pack.input_bytes,
            },
            members_digest: result.members_digest,
            reads,
            #[cfg(feature = "polar")]
            output,
        })
    }
}
impl Drop for IdentifiedSources<'_> {
    fn drop(&mut self) {
        // SAFETY: unique handle; destroy it BEFORE dropping callback providers.
        unsafe { sys::pcfs_verified_sources_destroy(self.handle.as_ptr()) };
    }
}
