use crate::{pcfs_image, pcfs_member};
use std::ffi::{c_char, c_void};

pub const PCFS_NATIVE_META_PROFILE_V1: u32 = 1;
pub const PCFS_HYBRID_META_PROFILE_V2: u32 = 2;
pub const PCFS_META_REDIRECT: u32 = 4;
pub const PCFS_META_DIRECTORY: u32 = 1;
pub const PCFS_META_FILE: u32 = 2;
pub const PCFS_META_SYMLINK: u32 = 3;
pub const PCFS_META_MAX_PACKS: usize = 64;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_meta_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub profile: u32,
    pub root_mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub max_output_bytes: u64,
    pub max_entries: u64,
    pub max_path_bytes: u64,
    pub max_content_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_meta_entry_v1 {
    pub path: *const c_char,
    pub symlink_target: *const c_char,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub kind: u32,
    pub pack_index: u32,
    pub mode: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct pcfs_meta_result_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub entries: u64,
    pub files: u64,
    pub dependencies: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_meta_stat_v1 {
    pub size_bytes: u64,
    pub kind: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}
extern "C" {
    pub fn pcfs_image_check_native_member(
        image: *mut pcfs_image,
        sha256: *const u8,
        size: u64,
    ) -> i32;
    pub fn pcfs_native_meta_build(
        options: *const pcfs_meta_options_v1,
        packs: *const *mut pcfs_image,
        pack_count: usize,
        entries: *const pcfs_meta_entry_v1,
        count: usize,
        output_fd: i32,
        is_cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        context: *mut c_void,
        result: *mut pcfs_meta_result_v1,
    ) -> i32;
    pub fn pcfs_meta_build_from_trusted_members(
        options: *const pcfs_meta_options_v1,
        packs: *const *mut pcfs_image,
        pack_count: usize,
        entries: *const pcfs_meta_entry_v1,
        count: usize,
        output_fd: i32,
        is_cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        context: *mut c_void,
        result: *mut pcfs_meta_result_v1,
    ) -> i32;
    pub fn pcfs_native_meta_open(
        fd: i32,
        sha256: *const u8,
        size: u64,
        packs: *const *mut pcfs_image,
        count: usize,
        out: *mut *mut pcfs_image,
    ) -> i32;
    pub fn pcfs_native_meta_member(
        image: *mut pcfs_image,
        path: *const c_char,
        sha256: *const u8,
        size: u64,
        out: *mut *mut pcfs_member,
    ) -> i32;
    pub fn pcfs_native_meta_stat(
        image: *mut pcfs_image,
        path: *const c_char,
        out: *mut pcfs_meta_stat_v1,
    ) -> i32;
    pub fn pcfs_native_meta_readlink(
        image: *mut pcfs_image,
        path: *const c_char,
        buffer: *mut c_char,
        capacity: usize,
        length: *mut usize,
    ) -> i32;
    pub fn pcfs_native_meta_count_entries(
        image: *mut pcfs_image,
        maximum: u64,
        out: *mut u64,
    ) -> i32;
    pub fn pcfs_native_meta_fingerprint() -> *const c_char;
}

extern "C" {
    pub fn pcfs_hybrid_meta_open(
        fd: i32,
        sha256: *const u8,
        size: u64,
        packs: *const *mut pcfs_image,
        count: usize,
        out: *mut *mut pcfs_image,
    ) -> i32;
    pub fn pcfs_hybrid_meta_open_managed(
        fd: i32,
        sha256: *const u8,
        size: u64,
        packs: *const *mut pcfs_image,
        count: usize,
        out: *mut *mut pcfs_image,
    ) -> i32;
    pub fn pcfs_hybrid_meta_redirect(
        image: *mut pcfs_image,
        path: *const c_char,
        sha256: *mut u8,
        size: *mut u64,
    ) -> i32;
    pub fn pcfs_native_meta_source_pack(
        image: *mut pcfs_image,
        path: *const c_char,
        pack_index: *mut u32,
    ) -> i32;
    pub fn pcfs_hybrid_meta_fingerprint() -> *const c_char;
}

extern "C" {
    pub fn pcfs_filesystem_open_managed(
        fd: i32,
        sha256: *const u8,
        size: u64,
        out: *mut *mut pcfs_image,
    ) -> i32;
}
