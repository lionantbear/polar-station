use polar_cas_fs_sys as sys;
use std::ffi::c_void;
use std::fmt;
use std::fs::File;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::rc::Rc;

/// A prepared, caller-pinned data source. No resolving, network I/O or async
/// waiting may occur in these methods. This capability is NOT digest evidence.
/// Implementations must keep their declared content stable for the whole job.
pub trait PreparedSource {
    fn size(&self) -> io::Result<u64>;
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize>;
    /// Returning an OwnedFd transfers a NEW readonly regular-file FD to C.
    fn open_readonly_fd(&self) -> io::Result<Option<OwnedFd>> {
        Ok(None)
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl PreparedSource for File {
    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        FileExt::read_at(self, buffer, offset)
    }
    fn open_readonly_fd(&self) -> io::Result<Option<OwnedFd>> {
        // C checks access flags and regular-file type before accepting the FD.
        Ok(Some(self.try_clone()?.into()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub status: i32,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Polar native I/O failed (status {})", self.status)
    }
}
impl std::error::Error for Error {}
fn check(status: i32) -> Result<(), Error> {
    if status == sys::PCFS_OK {
        Ok(())
    } else {
        Err(Error { status })
    }
}
fn callback_status(result: std::thread::Result<io::Result<()>>) -> i32 {
    match result {
        Ok(Ok(())) => sys::PCFS_OK,
        Ok(Err(_)) | Err(_) => sys::PCFS_IO,
    }
}
struct Context<'a> {
    provider: &'a dyn PreparedSource,
}
unsafe extern "C" fn open_callback(context: *mut c_void, _: u64, fd: *mut i32) -> i32 {
    // SAFETY: source owns this boxed Context until the native handle is closed;
    // C passes a valid writable fd pointer and initializes it to -1.
    let context = unsafe { &*context.cast::<Context<'_>>() };
    match catch_unwind(AssertUnwindSafe(|| context.provider.open_readonly_fd())) {
        Ok(Ok(Some(owned))) => {
            // SAFETY: validated callback output pointer; ownership moves to C.
            unsafe { *fd = owned.into_raw_fd() };
            sys::PCFS_OK
        }
        Ok(Ok(None)) => sys::PCFS_NO_FD,
        Ok(Err(_)) | Err(_) => sys::PCFS_IO,
    }
}
unsafe extern "C" fn read_callback(
    context: *mut c_void,
    _: u64,
    offset: u64,
    buffer: *mut u8,
    capacity: usize,
    read_out: *mut usize,
) -> i32 {
    // SAFETY: callback is synchronous; C supplies a writable bounded buffer.
    let context = unsafe { &*context.cast::<Context<'_>>() };
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, capacity) };
    callback_status(catch_unwind(AssertUnwindSafe(|| {
        let got = context.provider.read_at(offset, buffer)?;
        if got > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "provider overread",
            ));
        }
        // SAFETY: native read_out is valid for the duration of this call.
        unsafe { *read_out = got };
        Ok(())
    })))
}
unsafe extern "C" fn cancel_callback(context: *mut c_void) -> i32 {
    // SAFETY: same pinned Context as the other two callbacks.
    let context = unsafe { &*context.cast::<Context<'_>>() };
    match catch_unwind(AssertUnwindSafe(|| context.provider.is_cancelled())) {
        Ok(false) => 0,
        Ok(true) | Err(_) => 1,
    }
}

/// Owns the native source and pins its callback context. Not Send or Sync:
/// native callback/provider concurrency has not been approved in this step.
pub struct Source<'a> {
    handle: NonNull<sys::pcfs_source>,
    _context: Box<Context<'a>>,
    _not_send_sync: PhantomData<Rc<()>>,
}
impl<'a> Source<'a> {
    pub fn new(provider: &'a dyn PreparedSource) -> Result<Self, Error> {
        let size = provider.size().map_err(|_| Error {
            status: sys::PCFS_IO,
        })?;
        let mut context = Box::new(Context { provider });
        let callbacks = sys::pcfs_sources_v1 {
            abi_version: sys::PCFS_ABI_V1,
            struct_size: std::mem::size_of::<sys::pcfs_sources_v1>() as u32,
            context: (&mut *context as *mut Context<'_>).cast(),
            open_fd: Some(open_callback),
            read_at: Some(read_callback),
            is_cancelled: Some(cancel_callback),
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: C copies callbacks; boxed context is not moved in memory and
        // is stored in the resulting Source. On failure C cleans partial state.
        check(unsafe { sys::pcfs_source_open(&callbacks, 0, size, &mut handle) })?;
        let handle = NonNull::new(handle).ok_or(Error {
            status: sys::PCFS_IO,
        })?;
        Ok(Self {
            handle,
            _context: context,
            _not_send_sync: PhantomData,
        })
    }
    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, Error> {
        let mut got = 0;
        // SAFETY: source and buffer live through synchronous native call.
        check(unsafe {
            sys::pcfs_source_pread(
                self.handle.as_ptr(),
                offset,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut got,
            )
        })?;
        Ok(got)
    }
    pub fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
        // SAFETY: same ownership and buffer contracts as read_at.
        check(unsafe {
            sys::pcfs_source_pread_exact(
                self.handle.as_ptr(),
                offset,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        })
    }
    pub fn check_unchanged(&mut self) -> Result<(), Error> {
        // SAFETY: handle owned by self. This is a metadata check, NOT a hash.
        check(unsafe { sys::pcfs_source_check(self.handle.as_ptr()) })
    }
}
impl Drop for Source<'_> {
    fn drop(&mut self) {
        // SAFETY: unique owned handle is closed BEFORE its context is dropped.
        unsafe { sys::pcfs_source_destroy(self.handle.as_ptr()) };
    }
}

/// Bounded seekable staging output. The file must be exclusively controlled by
/// the job; Rust borrowing alone cannot prevent another process/FD from writing.
/// Finishing this handle does NOT verify, publish or make the inode immutable.
pub struct Output<'a> {
    handle: NonNull<sys::pcfs_output>,
    _file: PhantomData<&'a mut File>,
    _not_send_sync: PhantomData<Rc<()>>,
}
impl<'a> Output<'a> {
    pub fn new(file: &'a mut File, max_bytes: u64) -> Result<Self, Error> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: borrowed file stays live. C duplicates, never owns original FD.
        check(unsafe { sys::pcfs_output_new(file.as_raw_fd(), max_bytes, &mut handle) })?;
        let handle = NonNull::new(handle).ok_or(Error {
            status: sys::PCFS_IO,
        })?;
        Ok(Self {
            handle,
            _file: PhantomData,
            _not_send_sync: PhantomData,
        })
    }
    pub fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
        // SAFETY: the borrowed input is valid until synchronous pwrite completes.
        check(unsafe {
            sys::pcfs_output_pwrite(self.handle.as_ptr(), offset, bytes.as_ptr(), bytes.len())
        })
    }
    pub fn set_len(&mut self, size: u64) -> Result<(), Error> {
        // SAFETY: owned live handle; C enforces signed-offset and byte budgets.
        check(unsafe { sys::pcfs_output_truncate(self.handle.as_ptr(), size) })
    }
    pub fn finish(self) -> Result<u64, Error> {
        let mut size = 0;
        // SAFETY: live handle; failure leaves output unpublishable. Drop closes
        // the duplicate on BOTH success and failure, then releases file borrow.
        check(unsafe { sys::pcfs_output_finish(self.handle.as_ptr(), &mut size) })?;
        Ok(size)
    }
}
impl Drop for Output<'_> {
    fn drop(&mut self) {
        // SAFETY: uniquely owned native handle; cannot be used after self drops.
        unsafe { sys::pcfs_output_destroy(self.handle.as_ptr()) };
    }
}

pub fn digest_path(sha256: &[u8; 32]) -> Result<String, Error> {
    let mut path = [0u8; sys::PCFS_SHA256_PATH_CAPACITY];
    // SAFETY: fixed-size digest, writable full capacity including NUL terminator.
    check(unsafe { sys::pcfs_sha256_path(sha256.as_ptr(), path.as_mut_ptr().cast(), path.len()) })?;
    let end = path.iter().position(|b| *b == 0).ok_or(Error {
        status: sys::PCFS_INVALID,
    })?;
    std::str::from_utf8(&path[..end])
        .map(str::to_owned)
        .map_err(|_| Error {
            status: sys::PCFS_INVALID,
        })
}
