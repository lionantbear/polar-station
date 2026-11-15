//! Private output inode binding. This is neither digest verification nor admission.
use crate::Error;
use polar_cas_fs_sys as sys;
use std::{
    ffi::CStr,
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
};

/// Seal a finished job-private 0600 regular file and reopen the same inode
/// readonly. Does not copy or reread payload. The caller must own the directory
/// and exclude all other writers/name mutations, then CLOSE its writer before
/// publishing. Existing writable FDs are not revoked by chmod. No fs-verity
/// claim, content identity, or Catalog state is created by this operation.
pub fn seal_generated_file(
    writer: &File,
    parent: &File,
    leaf: &CStr,
    expected_size: u64,
) -> Result<File, Error> {
    let mut fd = -1;
    // SAFETY: borrowed FDs and NUL-terminated leaf live through this synchronous
    // call. C transfers a new readonly FD only on success and closes on errors.
    let status = unsafe {
        sys::pcfs_generated_seal(
            writer.as_raw_fd(),
            parent.as_raw_fd(),
            leaf.as_ptr(),
            expected_size,
            &mut fd,
        )
    };
    if status != sys::PCFS_OK {
        return Err(Error { status });
    }
    if fd < 0 {
        return Err(Error {
            status: sys::PCFS_IO,
        });
    }
    // SAFETY: success transferred sole ownership of exactly this new FD.
    Ok(unsafe { File::from_raw_fd(fd) })
}
