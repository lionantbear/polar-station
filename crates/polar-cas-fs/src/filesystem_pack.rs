//! A standalone EROFS filesystem with explicit ordinary paths.
//!
//! This adapter reuses the pinned importer and identified-source gate. It does
//! not consume host directory paths, assign object UUIDs, register Catalog
//! records, or convert an untrusted hash claim into an admission witness.
use crate::{
    Compression, Error, IdentifiedSources, IdentityLimits, ImageDescriptor, Member, MetaStat,
    NativeMeta, PreparedSource,
};
use polar_cas_fs_sys as sys;
use std::{ffi::CStr, fs::File, os::fd::AsRawFd, sync::atomic::AtomicBool};

#[derive(Clone, Debug)]
pub struct FilesystemEntry {
    /// Root-relative; explicit parent directories required. Root is options.
    pub path: String,
    pub kind: FilesystemKind,
}
#[derive(Clone, Debug)]
pub enum FilesystemKind {
    Directory {
        mode: u32,
    },
    File {
        source: usize,
        content: ImageDescriptor,
        mode: u32,
    },
    Symlink {
        target: String,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct FilesystemPackOptions {
    pub compression: Compression,
    pub root_mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub max_output_bytes: u64,
    pub max_input_bytes: u64,
    pub max_entries: u64,
    pub max_path_bytes: u64,
}
impl Default for FilesystemPackOptions {
    fn default() -> Self {
        Self {
            compression: Compression::Lz4,
            root_mode: 0o555,
            uid: 0,
            gid: 0,
            max_output_bytes: 64 << 30,
            max_input_bytes: 64 << 30,
            max_entries: 100_000,
            max_path_bytes: 64 << 20,
        }
    }
}
impl FilesystemPackOptions {
    fn native(self) -> sys::pcfs_filesystem_options_v1 {
        sys::pcfs_filesystem_options_v1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<sys::pcfs_filesystem_options_v1>() as u32,
            compression: match self.compression {
                Compression::None => 0,
                Compression::Lz4 => 1,
            },
            root_mode: self.root_mode,
            uid: self.uid,
            gid: self.gid,
            max_output_bytes: self.max_output_bytes,
            max_input_bytes: self.max_input_bytes,
            max_entries: self.max_entries,
            max_path_bytes: self.max_path_bytes,
        }
    }
}
#[derive(Clone, Debug)]
pub struct FilesystemPackResult {
    pub image: ImageDescriptor,
    /// Non-root filesystem path count, not inode or unique-content count.
    pub entries: u64,
    pub files: u64,
    pub input_bytes: u64,
}
fn check(status: i32) -> Result<(), Error> {
    if status == sys::PCFS_OK {
        Ok(())
    } else {
        Err(Error { status })
    }
}
fn native_entries(entries: &[FilesystemEntry]) -> Vec<sys::pcfs_filesystem_entry_v1> {
    entries
        .iter()
        .map(|e| {
            let mut result = sys::pcfs_filesystem_entry_v1 {
                abi_version: 1,
                struct_size: std::mem::size_of::<sys::pcfs_filesystem_entry_v1>() as u32,
                path: e.path.as_ptr().cast(),
                path_bytes: e.path.len() as u64,
                symlink_target: std::ptr::null(),
                symlink_bytes: 0,
                sha256: [0; 32],
                size_bytes: 0,
                source_token: 0,
                kind: 0,
                mode: 0,
                reserved: 0,
            };
            match &e.kind {
                FilesystemKind::Directory { mode } => {
                    result.kind = sys::PCFS_META_DIRECTORY;
                    result.mode = *mode;
                }
                FilesystemKind::File {
                    source,
                    content,
                    mode,
                } => {
                    result.kind = sys::PCFS_META_FILE;
                    result.mode = *mode;
                    result.sha256 = content.sha256;
                    result.size_bytes = content.size_bytes;
                    result.source_token = *source as u64;
                }
                FilesystemKind::Symlink { target } => {
                    result.kind = sys::PCFS_META_SYMLINK;
                    result.mode = 0o777;
                    result.symlink_target = target.as_ptr().cast();
                    result.symlink_bytes = target.len() as u64;
                }
            }
            result
        })
        .collect()
}

pub struct FilesystemPackBuilder;
impl FilesystemPackBuilder {
    /// Validate paths, node types, metadata and budgets without source I/O.
    pub fn validate(
        entries: &[FilesystemEntry],
        options: FilesystemPackOptions,
    ) -> Result<(), Error> {
        if entries.len() as u64 > options.max_entries || options.max_entries > 1_000_000 {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let native = native_entries(entries);
        // SAFETY: entry string spans and options stay borrowed throughout call;
        // C copies bounded strings and never retains the Rust input pointers.
        check(unsafe {
            sys::pcfs_filesystem_validate_v1(&options.native(), native.as_ptr(), native.len())
        })
    }

    /// Explicit one-shot source identification + build. Already identified
    /// sources may call IdentifiedSources::build_filesystem to reuse that pass.
    /// All provided sources are identified; callers should omit unused inputs.
    pub fn build(
        entries: &[FilesystemEntry],
        sources: &[&dyn PreparedSource],
        output: &mut File,
        options: FilesystemPackOptions,
        cancel: &AtomicBool,
    ) -> Result<FilesystemPackResult, Error> {
        Self::validate(entries, options)?;
        for e in entries {
            if let FilesystemKind::File { source, .. } = &e.kind {
                if *source >= sources.len() {
                    return Err(Error {
                        status: sys::PCFS_INVALID,
                    });
                }
            }
        }
        let limits = IdentityLimits {
            max_sources: options.max_entries,
            max_input_bytes: options.max_input_bytes,
            ..IdentityLimits::default()
        };
        let mut identified = IdentifiedSources::identify_cancellable(sources, limits, cancel)?;
        identified.build_filesystem(entries, output, options)
    }
    /// Include BOTH this adapter identity and PackBuilder::fingerprint() in a
    /// runtime build key; the latter pins upstream encoder and patches.
    pub fn fingerprint() -> &'static str {
        // SAFETY: returns an immutable static NUL-terminated string.
        unsafe { CStr::from_ptr(sys::pcfs_filesystem_fingerprint_v1()) }
            .to_str()
            .unwrap_or("invalid-filesystem-fingerprint")
    }
}

/// The raw handle is crate-private and borrowed exclusively from IdentifiedSources.
/// Caller must retain its native handle/context throughout this synchronous call.
pub(crate) unsafe fn build_identified(
    sources: *mut sys::pcfs_verified_sources,
    entries: &[FilesystemEntry],
    output: &mut File,
    options: FilesystemPackOptions,
) -> Result<FilesystemPackResult, Error> {
    FilesystemPackBuilder::validate(entries, options)?;
    let native = native_entries(entries);
    let mut result = sys::pcfs_filesystem_result_v1::default();
    // SAFETY: live exclusively borrowed source handle, valid output FD, bounded
    // entry spans. C validates identities and fills result only on success.
    check(unsafe {
        sys::pcfs_erofs_build_tree_verified_v1(
            &options.native(),
            sources,
            native.as_ptr(),
            native.len(),
            output.as_raw_fd(),
            &mut result,
        )
    })?;
    Ok(FilesystemPackResult {
        image: ImageDescriptor {
            sha256: result.sha256,
            size_bytes: result.size_bytes,
        },
        entries: result.entries,
        files: result.files,
        input_bytes: result.input_bytes,
    })
}

/// Strictly opens one standalone image, with ZERO external devices. The caller
/// retains the same immutable-backing obligation as existing NativeMeta readers.
pub struct FilesystemPackReader {
    inner: NativeMeta,
}
impl FilesystemPackReader {
    pub fn open(file: &File, expected: ImageDescriptor) -> Result<Self, Error> {
        Ok(Self {
            inner: NativeMeta::open(file, expected, &[])?,
        })
    }
    #[cfg(feature = "polar")]
    pub fn open_leased(owner: std::sync::Arc<crate::CasImageLease>) -> polar_cas::Result<Self> {
        Ok(Self {
            inner: NativeMeta::open_filesystem_lease(owner)?,
        })
    }
    pub fn stat(&self, path: &str) -> Result<MetaStat, Error> {
        self.inner.stat(path)
    }
    pub fn entry_count(&self) -> Result<u64, Error> {
        self.inner.entry_count()
    }
    pub fn member(&self, path: &str, expected: ImageDescriptor) -> Result<Member, Error> {
        self.inner.member(path, expected)
    }
    pub fn read_link(&self, path: &str) -> Result<String, Error> {
        self.inner.read_link(path)
    }
}
