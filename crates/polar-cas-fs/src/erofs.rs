use crate::{Error, PreparedSource};
use polar_cas_fs_sys as sys;
use std::cell::Cell;
use std::ffi::{c_void, CStr};
use std::fs::File;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Cumulative native counters. Hashes include newly finalized images; strict
/// open hashes are separate. Main-device reads include metadata and selected
/// data, exclude external-device direct reads and OS readahead.
pub type NativeIoCounters = sys::pcfs_io_counters_v1;
pub fn native_io_counters() -> Result<NativeIoCounters, Error> {
    let mut out = NativeIoCounters::default();
    // SAFETY: native atomic loads write one fixed record. Quiesce measured jobs
    // before taking exact deltas; concurrently sampled fields are not a transaction.
    check(unsafe { sys::pcfs_io_counters_read(&mut out) })?;
    Ok(out)
}

pub(crate) fn check(status: i32) -> Result<(), Error> {
    if status == sys::PCFS_OK {
        Ok(())
    } else {
        Err(Error { status })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Compression {
    None,
    Lz4,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum PackProfile {
    /// Step 04 format. May inline regular file data; not always projectable.
    #[default]
    LegacyV1,
    /// 4KiB raw/LZ4, no regular inline/tail/fragments or external devices.
    NativeV1,
}
#[derive(Clone, Copy, Debug)]
pub struct PackOptions {
    pub profile: PackProfile,
    pub compression: Compression,
    pub max_objects: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
}
impl Default for PackOptions {
    fn default() -> Self {
        Self {
            profile: PackProfile::LegacyV1,
            compression: Compression::Lz4,
            max_objects: 100_000,
            max_input_bytes: 64 << 30,
            max_output_bytes: 64 << 30,
        }
    }
}
pub struct PackObject<'a> {
    pub sha256: [u8; 32],
    pub source: &'a dyn PreparedSource,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackResult {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub unique_objects: u64,
    pub input_bytes: u64,
}
struct Context<'a, 'b> {
    objects: &'a [PackObject<'b>],
    cancelled: &'a AtomicBool,
}
unsafe extern "C" fn open_input(ctx: *mut c_void, token: u64, fd: *mut i32) -> i32 {
    // SAFETY: Context and descriptors are borrowed synchronously by build().
    let ctx = unsafe { &*ctx.cast::<Context<'_, '_>>() };
    let Some(obj) = usize::try_from(token).ok().and_then(|i| ctx.objects.get(i)) else {
        return sys::PCFS_INVALID;
    };
    match catch_unwind(AssertUnwindSafe(|| {
        if obj.source.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "source cancelled",
            ));
        }
        obj.source.open_readonly_fd()
    })) {
        Ok(Ok(Some(owned))) => {
            unsafe { *fd = owned.into_raw_fd() };
            sys::PCFS_OK
        }
        Ok(Ok(None)) => sys::PCFS_NO_FD,
        Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => sys::PCFS_CANCELLED,
        _ => sys::PCFS_IO,
    }
}
unsafe extern "C" fn read_input(
    ctx: *mut c_void,
    token: u64,
    offset: u64,
    buf: *mut u8,
    capacity: usize,
    got: *mut usize,
) -> i32 {
    // SAFETY: native source supplies valid bounded writable buffers; no pointer
    // is retained and all provider panics are caught before returning to C.
    let ctx = unsafe { &*ctx.cast::<Context<'_, '_>>() };
    let Some(obj) = usize::try_from(token).ok().and_then(|i| ctx.objects.get(i)) else {
        return sys::PCFS_INVALID;
    };
    let bytes = unsafe { std::slice::from_raw_parts_mut(buf, capacity) };
    match catch_unwind(AssertUnwindSafe(|| {
        if obj.source.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "source cancelled",
            ));
        }
        obj.source.read_at(offset, bytes)
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
unsafe extern "C" fn is_cancelled(ctx: *mut c_void) -> i32 {
    // O(1): never scan a 100,000-object provider list for every 64KiB read.
    let ctx = unsafe { &*ctx.cast::<Context<'_, '_>>() };
    i32::from(ctx.cancelled.load(Ordering::Relaxed))
}

pub struct PackBuilder;
impl PackBuilder {
    /// Direct C-library build, no tar, no input-directory materialization and
    /// no encoder subprocess. Execute a complete job on a blocking worker.
    /// On Err, discard the partial output. On Ok, caller still owns CAS sealing,
    /// immutability, publisher transaction and replica registration.
    pub fn build(
        objects: &[PackObject<'_>],
        output: &mut File,
        options: PackOptions,
    ) -> Result<PackResult, Error> {
        Self::build_cancellable(objects, output, options, &AtomicBool::new(false))
    }
    pub fn build_cancellable(
        objects: &[PackObject<'_>],
        output: &mut File,
        options: PackOptions,
        cancelled: &AtomicBool,
    ) -> Result<PackResult, Error> {
        if options.max_objects > 1_000_000 || objects.len() as u64 > options.max_objects {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let mut entries = Vec::with_capacity(objects.len());
        for (i, obj) in objects.iter().enumerate() {
            let size = obj.source.size().map_err(|_| Error {
                status: sys::PCFS_IO,
            })?;
            entries.push(sys::pcfs_pack_entry_v1 {
                sha256: obj.sha256,
                size_bytes: size,
                source_token: i as u64,
            });
        }
        let mut ctx = Context { objects, cancelled };
        let sources = sys::pcfs_sources_v1 {
            abi_version: sys::PCFS_ABI_V1,
            struct_size: std::mem::size_of::<sys::pcfs_sources_v1>() as u32,
            context: (&mut ctx as *mut Context<'_, '_>).cast(),
            open_fd: Some(open_input),
            read_at: Some(read_input),
            is_cancelled: Some(is_cancelled),
        };
        let opts = sys::pcfs_pack_options_v1 {
            abi_version: sys::PCFS_ABI_V1,
            struct_size: std::mem::size_of::<sys::pcfs_pack_options_v1>() as u32,
            profile: match options.profile {
                PackProfile::LegacyV1 => 1,
                PackProfile::NativeV1 => 2,
            },
            compression: match options.compression {
                Compression::None => 0,
                Compression::Lz4 => 1,
            },
            max_output_bytes: options.max_output_bytes,
            max_input_bytes: options.max_input_bytes,
            max_objects: options.max_objects,
        };
        let mut result = sys::pcfs_pack_result_v1::default();
        // SAFETY: callbacks/entries/output remain alive for the full synchronous
        // call; callbacks never unwind. C closes all temporary sources first.
        check(unsafe {
            sys::pcfs_erofs_build(
                &opts,
                &sources,
                entries.as_ptr(),
                entries.len(),
                output.as_raw_fd(),
                &mut result,
            )
        })?;
        Ok(PackResult {
            sha256: result.sha256,
            size_bytes: result.size_bytes,
            unique_objects: result.unique_objects,
            input_bytes: result.input_bytes,
        })
    }
    pub fn fingerprint() -> &'static str {
        // SAFETY: native returns a static NUL-terminated immutable constant.
        unsafe { CStr::from_ptr(sys::pcfs_erofs_fingerprint()) }
            .to_str()
            .unwrap_or("invalid-native-fingerprint")
    }
}

/// Whole file identity at the native boundary, not an fs-verity digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDescriptor {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}

/// A validated, fd-pinned native image. Keep the storage lease immutable for its
/// entire lifetime. Owned handles are Send, not Sync.
pub struct Image {
    pub(crate) handle: NonNull<sys::pcfs_image>,
    descriptor: ImageDescriptor,
    pub(crate) owner: Option<Arc<dyn Send + Sync>>,
    _not_sync: PhantomData<Cell<()>>,
}
// SAFETY: owned image state contains no surviving borrowed callbacks or TLS
// resources. Native references are atomic and image cache mutations are locally
// locked; fixed raw/LZ4 read state is image-owned. Do not add Sync.
unsafe impl Send for Image {}
impl Image {
    pub fn open(file: &File, expected_sha256: &[u8; 32], size_bytes: u64) -> Result<Self, Error> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: C duplicates the borrowed FD and validates digest and format.
        check(unsafe {
            sys::pcfs_image_open(
                file.as_raw_fd(),
                expected_sha256.as_ptr(),
                size_bytes,
                &mut handle,
            )
        })?;
        Ok(Self {
            handle: NonNull::new(handle).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            descriptor: ImageDescriptor {
                sha256: *expected_sha256,
                size_bytes,
            },
            owner: None,
            _not_sync: PhantomData,
        })
    }
    /// Open only metadata from an exact-image lease. Every lease constructor
    /// established the image identity through generation, managed admission,
    /// sealed-snapshot verification or Host preparation. Member-admission trust
    /// is separate: unknown imported member claims still require explicit audit.
    #[cfg(feature = "polar")]
    pub fn open_leased(lease: &crate::CasImageLease) -> polar_cas::Result<Self> {
        Self::open_owned_lease(Arc::new(lease.try_clone()?))
    }
    /// Share an existing owned lease without duplicating its Rust descriptor.
    #[cfg(feature = "polar")]
    pub(crate) fn open_owned_lease(owner: Arc<crate::CasImageLease>) -> polar_cas::Result<Self> {
        let expected = crate::projection::descriptor(owner.content())?;
        owner.revalidate()?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: the typed lease already binds exact immutable bytes and pins
        // its owner for Image and all Member lives. This does not assert members.
        check(unsafe {
            sys::pcfs_image_open_managed(
                owner.file().as_raw_fd(),
                expected.sha256.as_ptr(),
                expected.size_bytes,
                &mut handle,
            )
        })
        .map_err(crate::projection::cas_error)?;
        Ok(Self {
            handle: NonNull::new(handle).ok_or(polar_cas::Error::Integrity)?,
            descriptor: expected,
            owner: Some(owner),
            _not_sync: PhantomData,
        })
    }
    pub fn descriptor(&self) -> ImageDescriptor {
        self.descriptor
    }
    /// Bounded self-description of a construction-profile Pack. Returned JSON
    /// is authenticated as part of this image identity, not as a statement from
    /// a trusted producer. Unknown input must not gain a ready member replica
    /// merely because this metadata is present.
    pub fn pack_build_summary(&self) -> Result<Vec<u8>, Error> {
        let mut data = vec![0u8; 1024];
        let mut length = 0;
        // SAFETY: native image is owned and supplies at most this buffer length.
        check(unsafe {
            sys::pcfs_pack_summary_read(
                self.handle.as_ptr(),
                data.as_mut_ptr().cast(),
                data.len(),
                &mut length,
            )
        })?;
        data.truncate(length);
        Ok(data)
    }
    /// Complete external Pack admission. The opaque image identity alone does
    /// not establish member claims; every canonical member is verified once.
    #[cfg(feature = "polar")]
    pub fn admit_complete_pack(
        &self,
        max_members: usize,
        cancelled: &AtomicBool,
    ) -> polar_cas::Result<polar_cas::shared::CompletePackIndex> {
        if max_members > polar_cas::shared::MAX_PACK_MEMBERS {
            return Err(polar_cas::Error::TooLarge);
        }
        unsafe extern "C" fn cancel(context: *mut c_void) -> i32 {
            // SAFETY: borrowed AtomicBool lives through the synchronous call.
            i32::from(unsafe { &*context.cast::<AtomicBool>() }.load(Ordering::Relaxed))
        }
        let mut rows = vec![sys::pcfs_pack_member_v1::default(); max_members];
        let mut count = 0;
        // SAFETY: bounded writable rows and cancellation reference remain live;
        // native code retains neither pointer and returns at most capacity rows.
        check(unsafe {
            sys::pcfs_pack_members(
                self.handle.as_ptr(),
                rows.as_mut_ptr(),
                rows.len(),
                &mut count,
                Some(cancel),
                (cancelled as *const AtomicBool).cast_mut().cast(),
            )
        })
        .map_err(crate::projection::cas_error)?;
        rows.truncate(count);
        let mut members = Vec::with_capacity(count);
        for row in rows {
            if cancelled.load(Ordering::Relaxed) {
                return Err(polar_cas::Error::Io(io::ErrorKind::Interrupted.into()));
            }
            self.check_native_member(&row.sha256, row.size_bytes)
                .map_err(crate::projection::cas_error)?;
            let member = self
                .member(&row.sha256, row.size_bytes)
                .map_err(crate::projection::cas_error)?;
            // SAFETY: owned member and borrowed cancellation flag survive the call.
            check(unsafe {
                sys::pcfs_verify_member_cancellable(
                    member.handle.as_ptr(),
                    Some(cancel),
                    (cancelled as *const AtomicBool).cast_mut().cast(),
                )
            })
            .map_err(crate::projection::cas_error)?;
            members.push(polar_cas::ContentRef {
                id: polar_cas::ObjectId::from_sha256(row.sha256),
                size_bytes: row.size_bytes,
            });
        }
        polar_cas::shared::CompletePackIndex::from_trusted_adapter(
            polar_cas::ContentRef {
                id: polar_cas::ObjectId::from_sha256(self.descriptor.sha256),
                size_bytes: self.descriptor.size_bytes,
            },
            members,
        )
    }

    /// Derive size inside the native adapter. Outer CAS verification still
    /// checks the requested ContentRef when the stream reaches EOF.
    pub fn member_by_digest(&self, sha256: &[u8; 32]) -> Result<Member, Error> {
        let mut member = std::ptr::null_mut();
        let mut size = 0;
        // SAFETY: C retains an image reference for the returned owned member.
        check(unsafe {
            sys::pcfs_image_member_by_digest(
                self.handle.as_ptr(),
                sha256.as_ptr(),
                &mut size,
                &mut member,
            )
        })?;
        // SAFETY: success transferred one owned member handle.
        unsafe { Member::from_raw(member, size) }
            .map(|member| member.with_owner(self.owner.clone()))
    }
    pub fn check_native_member(&self, sha256: &[u8; 32], size_bytes: u64) -> Result<(), Error> {
        // SAFETY: native image is owned and its backing remains pinned by self.
        check(unsafe {
            sys::pcfs_image_check_native_member(self.handle.as_ptr(), sha256.as_ptr(), size_bytes)
        })
    }
    pub fn member(&self, sha256: &[u8; 32], size_bytes: u64) -> Result<Member, Error> {
        let mut member = std::ptr::null_mut();
        // SAFETY: native member increments the image refcount; it may outlive self.
        check(unsafe {
            sys::pcfs_image_member(
                self.handle.as_ptr(),
                sha256.as_ptr(),
                size_bytes,
                &mut member,
            )
        })?;
        Ok(Member {
            handle: NonNull::new(member).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            size_bytes,
            cursor: 0,
            owner: self.owner.clone(),
            _not_sync: PhantomData,
        })
    }
    /// Bounded multi-member open, retaining one independent native handle for
    /// every successful slot. Per-member errors never discard successful peers.
    pub fn members(
        &self,
        members: &[ImageDescriptor],
    ) -> Result<Vec<Result<Member, Error>>, Error> {
        if members.len() > 1024 {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let rows: Vec<_> = members
            .iter()
            .map(|m| sys::pcfs_pack_member_v1 {
                sha256: m.sha256,
                size_bytes: m.size_bytes,
            })
            .collect();
        let mut handles = vec![std::ptr::null_mut(); rows.len()];
        let mut statuses = vec![sys::PCFS_INVALID; rows.len()];
        // SAFETY: arrays match count and remain live. Native code initializes
        // every output and transfers handles only for PCFS_OK item statuses.
        check(unsafe {
            sys::pcfs_image_members(
                self.handle.as_ptr(),
                rows.as_ptr(),
                rows.len(),
                statuses.as_mut_ptr(),
                handles.as_mut_ptr(),
            )
        })?;
        Ok(handles
            .into_iter()
            .zip(statuses)
            .zip(members)
            .map(|((handle, status), row)| {
                check(status)?;
                // SAFETY: successful slot transfers exactly one owned handle.
                unsafe { Member::from_raw(handle, row.size_bytes) }
                    .map(|m| m.with_owner(self.owner.clone()))
            })
            .collect())
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: one owned image ref is released; members hold their own refs.
        unsafe { sys::pcfs_image_destroy(self.handle.as_ptr()) };
    }
}
pub struct Member {
    handle: NonNull<sys::pcfs_member>,
    size_bytes: u64,
    cursor: u64,
    owner: Option<Arc<dyn Send + Sync>>,
    _not_sync: PhantomData<Cell<()>>,
}
// SAFETY: lazy inode mutations are locked per member and image references are
// atomic. Its Rust cursor changes only via &mut; no borrowed callbacks survive.
unsafe impl Send for Member {}
impl Member {
    /// Reopen parsed metadata with an independent cursor, without filesystem I/O.
    /// The caller retains the same immutable lease as for this member.
    pub fn try_clone(&self) -> Result<Self, Error> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: source stays alive; C copies metadata under its local lock
        // and transfers a new handle with its own retained image reference.
        check(unsafe { sys::pcfs_member_clone(self.handle.as_ptr(), &mut handle) })?;
        // SAFETY: success returned one owned handle with the same logical size.
        unsafe { Self::from_raw(handle, self.size_bytes) }.map(|m| m.with_owner(self.owner.clone()))
    }
    /// Takes one owned opaque handle returned by a successful native call.
    pub(crate) unsafe fn from_raw(
        handle: *mut sys::pcfs_member,
        size_bytes: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            handle: NonNull::new(handle).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            size_bytes,
            cursor: 0,
            owner: None,
            _not_sync: PhantomData,
        })
    }
    pub(crate) fn with_owner(mut self, owner: Option<Arc<dyn Send + Sync>>) -> Self {
        self.owner = owner;
        self
    }
    pub fn len(&self) -> u64 {
        self.size_bytes
    }
    pub fn is_empty(&self) -> bool {
        self.size_bytes == 0
    }
    pub fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, Error> {
        let mut got = 0;
        // SAFETY: native owns the opaque inode; all access is internally locked.
        check(unsafe {
            sys::pcfs_member_pread(
                self.handle.as_ptr(),
                offset,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut got,
            )
        })?;
        Ok(got)
    }
    pub fn verify(&self) -> Result<(), Error> {
        // SAFETY: native owns this inode and compares full decoded SHA256.
        check(unsafe { sys::pcfs_member_verify(self.handle.as_ptr()) })
    }
}
impl Drop for Member {
    fn drop(&mut self) {
        // SAFETY: release the sole member handle and its image ref.
        unsafe { sys::pcfs_member_destroy(self.handle.as_ptr()) };
    }
}
impl io::Read for Member {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(self.cursor, buf).map_err(io::Error::other)?;
        self.cursor += n as u64;
        Ok(n)
    }
}
impl PreparedSource for Member {
    fn size(&self) -> io::Result<u64> {
        Ok(self.size_bytes)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        Member::read_at(self, offset, buf).map_err(io::Error::other)
    }
}
