//! Kernel evidence only. Never reinterpret a verity digest as ContentRef.
use polar_cas_fs_sys as sys;
use std::{fs::File, os::fd::AsRawFd};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerityPolicy {
    RequireEnabled,
    EnableIfMissing,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerityDigest {
    pub algorithm: u32,
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub struct VerityError {
    pub status: i32,
    pub os_error: i32,
}
impl std::fmt::Display for VerityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fs-verity status {}: {}",
            self.status,
            std::io::Error::from_raw_os_error(self.os_error)
        )
    }
}
impl std::error::Error for VerityError {}
pub fn require(file: &File, policy: VerityPolicy) -> Result<VerityDigest, VerityError> {
    let mut result = sys::pcfs_verity_digest_v1 {
        algorithm: 0,
        digest_size: 0,
        digest: [0; 64],
    };
    let mut errno = 0;
    // SAFETY: a borrowed live fd, nonoverlapping owned result buffers; C never
    // retains them and bounds its variable-length kernel digest to 64 bytes.
    let status = unsafe {
        sys::pcfs_verity_require(
            file.as_raw_fd(),
            u32::from(policy == VerityPolicy::EnableIfMissing),
            &mut result,
            &mut errno,
        )
    };
    if status != sys::PCFS_OK {
        return Err(VerityError {
            status,
            os_error: errno,
        });
    }
    if result.digest_size > 64 {
        return Err(VerityError {
            status: sys::PCFS_CORRUPT,
            os_error: 0,
        });
    }
    Ok(VerityDigest {
        algorithm: result.algorithm,
        bytes: result.digest[..result.digest_size as usize].to_vec(),
    })
}
