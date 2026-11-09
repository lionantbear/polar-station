//! Arbitrary logical tree -> native EROFS metadata + ordered external packs.
//! This is a synchronous adapter. It performs no Catalog queries, network IO,
//! filesystem mount, fs-verity enablement, GC or publication.
use crate::erofs::check;
use crate::{Error, Image, ImageDescriptor, Member};
use polar_cas_fs_sys as sys;
use std::ffi::{c_void, CStr, CString};
use std::fs::File;
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug)]
pub enum MetaKind<'a> {
    Directory {
        mode: u32,
    },
    File {
        content: ImageDescriptor,
        pack_index: u32,
        mode: u32,
    },
    Redirect {
        content: ImageDescriptor,
        mode: u32,
    },
    Symlink {
        target: &'a str,
    },
}
#[derive(Clone, Debug)]
pub struct MetaEntry<'a> {
    pub path: &'a str,
    pub kind: MetaKind<'a>,
}
#[derive(Clone, Copy, Debug)]
pub struct MetaOptions {
    pub root_mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub max_entries: u64,
    pub max_output_bytes: u64,
    pub max_path_bytes: u64,
    pub max_content_bytes: u64,
}
impl Default for MetaOptions {
    fn default() -> Self {
        Self {
            root_mode: 0o755,
            uid: 0,
            gid: 0,
            max_entries: 100_000,
            max_output_bytes: 1 << 30,
            max_path_bytes: 64 << 20,
            max_content_bytes: 64 << 30,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMetaResult {
    pub meta: ImageDescriptor,
    /// The exact order is part of the native image ABI. Do not sort/re-resolve.
    pub packs: Vec<ImageDescriptor>,
    pub entries: u64,
    pub files: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct MetaStat {
    pub size_bytes: u64,
    /// Native constants: directory=1, regular=2, symlink=3.
    pub kind: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}
fn cstring(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error {
        status: sys::PCFS_INVALID,
    })
}
unsafe extern "C" fn cancelled(context: *mut c_void) -> i32 {
    // SAFETY: points to a borrowed AtomicBool for the synchronous call only.
    i32::from(unsafe { &*context.cast::<AtomicBool>() }.load(Ordering::Relaxed))
}
pub struct NativeMetaCompiler;
impl NativeMetaCompiler {
    pub fn build(
        entries: &[MetaEntry<'_>],
        packs: &[&Image],
        output: &mut File,
        options: MetaOptions,
    ) -> Result<NativeMetaResult, Error> {
        Self::build_cancellable(entries, packs, output, options, &AtomicBool::new(false))
    }
    pub fn build_cancellable(
        entries: &[MetaEntry<'_>],
        packs: &[&Image],
        output: &mut File,
        options: MetaOptions,
        cancel: &AtomicBool,
    ) -> Result<NativeMetaResult, Error> {
        Self::build_profile(
            entries,
            packs,
            output,
            options,
            cancel,
            sys::PCFS_NATIVE_META_PROFILE_V1,
            false,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn build_profile(
        entries: &[MetaEntry<'_>],
        packs: &[&Image],
        output: &mut File,
        options: MetaOptions,
        cancel: &AtomicBool,
        profile: u32,
        trusted: bool,
    ) -> Result<NativeMetaResult, Error> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error {
                status: sys::PCFS_CANCELLED,
            });
        }
        if packs.len() > sys::PCFS_META_MAX_PACKS
            || entries.len() as u64 > options.max_entries
            || options.max_entries > 1_000_000
        {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        // Heap allocations containing C strings remain fixed until native returns.
        let mut names = Vec::with_capacity(entries.len());
        let mut links = Vec::with_capacity(entries.len());
        let mut string_bytes = 0u64;
        for e in entries {
            if e.path.len() > 4095 {
                return Err(Error {
                    status: sys::PCFS_INVALID,
                });
            }
            let link = match &e.kind {
                MetaKind::Symlink { target } => Some(*target),
                _ => None,
            };
            if link.is_some_and(|s| s.len() > 4095) {
                return Err(Error {
                    status: sys::PCFS_INVALID,
                });
            }
            string_bytes = string_bytes
                .checked_add((e.path.len() + 1 + link.map_or(0, |s| s.len() + 1)) as u64)
                .ok_or(Error {
                    status: sys::PCFS_LIMIT,
                })?;
            if string_bytes > options.max_path_bytes {
                return Err(Error {
                    status: sys::PCFS_LIMIT,
                });
            }
            names.push(cstring(e.path)?);
            links.push(link.map(cstring).transpose()?);
        }
        let native: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (kind, digest, size_bytes, pack_index, mode) = match &e.kind {
                    MetaKind::Directory { mode } => {
                        (sys::PCFS_META_DIRECTORY, [0; 32], 0, 0, *mode)
                    }
                    MetaKind::File {
                        content,
                        pack_index,
                        mode,
                    } => (
                        sys::PCFS_META_FILE,
                        content.sha256,
                        content.size_bytes,
                        *pack_index,
                        *mode,
                    ),
                    MetaKind::Redirect { content, mode } => (
                        sys::PCFS_META_REDIRECT,
                        content.sha256,
                        content.size_bytes,
                        0,
                        *mode,
                    ),
                    MetaKind::Symlink { .. } => (sys::PCFS_META_SYMLINK, [0; 32], 0, 0, 0o777),
                };
                sys::pcfs_meta_entry_v1 {
                    path: names[i].as_ptr(),
                    symlink_target: links[i].as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                    sha256: digest,
                    size_bytes,
                    kind,
                    pack_index,
                    mode,
                    reserved: 0,
                }
            })
            .collect();
        let handles: Vec<_> = packs.iter().map(|p| p.handle.as_ptr()).collect();
        let opts = sys::pcfs_meta_options_v1 {
            abi_version: sys::PCFS_ABI_V1,
            struct_size: std::mem::size_of::<sys::pcfs_meta_options_v1>() as u32,
            profile,
            root_mode: options.root_mode,
            uid: options.uid,
            gid: options.gid,
            max_entries: options.max_entries,
            max_output_bytes: options.max_output_bytes,
            max_path_bytes: options.max_path_bytes,
            max_content_bytes: options.max_content_bytes,
        };
        let mut result = sys::pcfs_meta_result_v1::default();
        // SAFETY: all strings, entries, images, callback context and fd remain
        // alive for this synchronous call. Cancellation uses only an atomic read.
        let build = if trusted {
            sys::pcfs_meta_build_from_trusted_members
        } else {
            sys::pcfs_native_meta_build
        };
        check(unsafe {
            build(
                &opts,
                handles.as_ptr(),
                handles.len(),
                native.as_ptr(),
                native.len(),
                output.as_raw_fd(),
                Some(cancelled),
                (cancel as *const AtomicBool).cast_mut().cast(),
                &mut result,
            )
        })?;
        Ok(NativeMetaResult {
            meta: ImageDescriptor {
                sha256: result.sha256,
                size_bytes: result.size_bytes,
            },
            packs: packs.iter().map(|p| p.descriptor()).collect(),
            entries: result.entries,
            files: result.files,
        })
    }
    pub fn fingerprint() -> &'static str {
        // SAFETY: the native function returns a static immutable NUL string.
        unsafe { CStr::from_ptr(sys::pcfs_native_meta_fingerprint()) }
            .to_str()
            .unwrap_or("invalid-native-fingerprint")
    }
}
/// Readable native meta image. C retains M and all ordered pack FDs. The storage
/// owner must still provide immutability/GC leases; fd pins alone are not verity.
pub struct NativeMeta {
    handle: NonNull<sys::pcfs_image>,
    owners: Arc<Vec<Arc<dyn Send + Sync>>>,
    _not_send_sync: PhantomData<Rc<()>>,
}
impl NativeMeta {
    pub fn open(file: &File, expected: ImageDescriptor, packs: &[&Image]) -> Result<Self, Error> {
        if packs.len() > sys::PCFS_META_MAX_PACKS {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let handles: Vec<_> = packs.iter().map(|p| p.handle.as_ptr()).collect();
        let mut out = std::ptr::null_mut();
        // SAFETY: C validates M and device tags and retains each pack on success.
        check(unsafe {
            sys::pcfs_native_meta_open(
                file.as_raw_fd(),
                expected.sha256.as_ptr(),
                expected.size_bytes,
                handles.as_ptr(),
                handles.len(),
                &mut out,
            )
        })?;
        Ok(Self {
            handle: NonNull::new(out).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            owners: Arc::new(packs.iter().filter_map(|p| p.owner.clone()).collect()),
            _not_send_sync: PhantomData,
        })
    }
    #[cfg(feature = "polar")]
    pub(crate) fn open_filesystem_lease(
        owner: Arc<crate::CasImageLease>,
    ) -> polar_cas::Result<Self> {
        owner.revalidate()?;
        let expected = crate::projection::descriptor(owner.content())?;
        let mut out = std::ptr::null_mut();
        // SAFETY: exact admitted immutable lease supplies descriptor and ownership.
        // C accepts zero external devices; retained owners follow each Member.
        check(unsafe {
            sys::pcfs_filesystem_open_managed(
                owner.file().as_raw_fd(),
                expected.sha256.as_ptr(),
                expected.size_bytes,
                &mut out,
            )
        })
        .map_err(crate::projection::cas_error)?;
        Ok(Self {
            handle: NonNull::new(out).ok_or(polar_cas::Error::Integrity)?,
            owners: Arc::new(vec![owner]),
            _not_send_sync: PhantomData,
        })
    }
    pub fn member(&self, path: &str, expected: ImageDescriptor) -> Result<Member, Error> {
        let path = cstring(path)?;
        let mut out = std::ptr::null_mut();
        // SAFETY: image and C string are borrowed; returned member owns a ref.
        check(unsafe {
            sys::pcfs_native_meta_member(
                self.handle.as_ptr(),
                path.as_ptr(),
                expected.sha256.as_ptr(),
                expected.size_bytes,
                &mut out,
            )
        })?;
        // SAFETY: a successful native call returned exactly one owned member.
        unsafe { Member::from_raw(out, expected.size_bytes) }
            .map(|member| member.with_owner(Some(self.owners.clone())))
    }
    pub fn entry_count(&self) -> Result<u64, Error> {
        let mut count = 0;
        // SAFETY: valid owned image and an output integer; C bounds traversal.
        check(unsafe {
            sys::pcfs_native_meta_count_entries(self.handle.as_ptr(), 1_000_001, &mut count)
        })?;
        Ok(count)
    }
    pub fn stat(&self, path: &str) -> Result<MetaStat, Error> {
        let path = cstring(path)?;
        let mut st = sys::pcfs_meta_stat_v1::default();
        // SAFETY: owned image, valid NUL string and writable output record.
        check(unsafe { sys::pcfs_native_meta_stat(self.handle.as_ptr(), path.as_ptr(), &mut st) })?;
        Ok(MetaStat {
            size_bytes: st.size_bytes,
            kind: st.kind,
            mode: st.mode,
            uid: st.uid,
            gid: st.gid,
        })
    }
    pub fn read_link(&self, path: &str) -> Result<String, Error> {
        let path = cstring(path)?;
        let mut bytes = [0u8; 4096];
        let mut len = 0;
        // SAFETY: the native function bounds writes to this fixed buffer.
        check(unsafe {
            sys::pcfs_native_meta_readlink(
                self.handle.as_ptr(),
                path.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut len,
            )
        })?;
        String::from_utf8(bytes[..len].to_vec()).map_err(|_| Error {
            status: sys::PCFS_CORRUPT,
        })
    }
}
impl Drop for NativeMeta {
    fn drop(&mut self) {
        // SAFETY: release one image ref; live members independently pin it.
        unsafe { sys::pcfs_image_destroy(self.handle.as_ptr()) };
    }
}

/// Explicit opt-in to the Hybrid ABI; native v1 behavior is unchanged.
pub struct HybridMetaCompiler;
impl HybridMetaCompiler {
    pub fn build(
        entries: &[MetaEntry<'_>],
        packs: &[&Image],
        output: &mut File,
        options: MetaOptions,
        cancel: &AtomicBool,
    ) -> Result<NativeMetaResult, Error> {
        NativeMetaCompiler::build_profile(
            entries,
            packs,
            output,
            options,
            cancel,
            sys::PCFS_HYBRID_META_PROFILE_V2,
            false,
        )
    }
    /// Internal only: caller has a typed managed/generated lease or explicitly
    /// audited every selected member before reaching this constructor.
    #[cfg(feature = "polar")]
    pub(crate) fn build_trusted(
        entries: &[MetaEntry<'_>],
        packs: &[&Image],
        output: &mut File,
        options: MetaOptions,
        cancel: &AtomicBool,
    ) -> Result<NativeMetaResult, Error> {
        NativeMetaCompiler::build_profile(
            entries,
            packs,
            output,
            options,
            cancel,
            sys::PCFS_HYBRID_META_PROFILE_V2,
            true,
        )
    }
    pub fn fingerprint() -> &'static str {
        // SAFETY: native returns a static immutable NUL-terminated string.
        unsafe { CStr::from_ptr(sys::pcfs_hybrid_meta_fingerprint()) }
            .to_str()
            .unwrap_or("invalid-hybrid-fingerprint")
    }
}
pub struct HybridMeta(NativeMeta);
impl HybridMeta {
    pub fn open(file: &File, expected: ImageDescriptor, packs: &[&Image]) -> Result<Self, Error> {
        Self::open_inner(file, expected, packs, false)
    }
    /// Internal only: exact output of the native constructor, already hashed.
    #[cfg(feature = "polar")]
    pub(crate) fn open_generated(
        file: &File,
        expected: ImageDescriptor,
        packs: &[&Image],
    ) -> Result<Self, Error> {
        Self::open_inner(file, expected, packs, true)
    }
    fn open_inner(
        file: &File,
        expected: ImageDescriptor,
        packs: &[&Image],
        generated: bool,
    ) -> Result<Self, Error> {
        if packs.len() > sys::PCFS_META_MAX_PACKS {
            return Err(Error {
                status: sys::PCFS_LIMIT,
            });
        }
        let handles: Vec<_> = packs.iter().map(|p| p.handle.as_ptr()).collect();
        let mut out = std::ptr::null_mut();
        // SAFETY: arguments live for the synchronous call. C retains all inputs.
        let open = if generated {
            sys::pcfs_hybrid_meta_open_managed
        } else {
            sys::pcfs_hybrid_meta_open
        };
        check(unsafe {
            open(
                file.as_raw_fd(),
                expected.sha256.as_ptr(),
                expected.size_bytes,
                handles.as_ptr(),
                handles.len(),
                &mut out,
            )
        })?;
        Ok(Self(NativeMeta {
            handle: NonNull::new(out).ok_or(Error {
                status: sys::PCFS_IO,
            })?,
            owners: Arc::new(packs.iter().filter_map(|p| p.owner.clone()).collect()),
            _not_send_sync: PhantomData,
        }))
    }
    pub fn stat(&self, path: &str) -> Result<MetaStat, Error> {
        self.0.stat(path)
    }
    pub fn entry_count(&self) -> Result<u64, Error> {
        self.0.entry_count()
    }
    pub fn read_link(&self, path: &str) -> Result<String, Error> {
        self.0.read_link(path)
    }
    /// Never returns the zero-filled body of a redirect placeholder.
    pub fn member(&self, path: &str, expected: ImageDescriptor) -> Result<Member, Error> {
        self.0.member(path, expected)
    }
    pub fn redirect(&self, path: &str) -> Result<ImageDescriptor, Error> {
        let name = cstring(path)?;
        let mut sha256 = [0; 32];
        let mut size_bytes = 0;
        // SAFETY: valid borrowed image/path and exclusively borrowed outputs.
        check(unsafe {
            sys::pcfs_hybrid_meta_redirect(
                self.0.handle.as_ptr(),
                name.as_ptr(),
                sha256.as_mut_ptr(),
                &mut size_bytes,
            )
        })?;
        Ok(ImageDescriptor { sha256, size_bytes })
    }
    /// Adapter-only slot; None for empty native files, which have no extents.
    pub fn native_pack_slot(&self, path: &str) -> Result<Option<u32>, Error> {
        let name = cstring(path)?;
        let mut slot = u32::MAX;
        // SAFETY: C checks inode mappings with a retained image closure.
        check(unsafe {
            sys::pcfs_native_meta_source_pack(self.0.handle.as_ptr(), name.as_ptr(), &mut slot)
        })?;
        Ok((slot != u32::MAX).then_some(slot))
    }
}
