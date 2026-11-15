//! Additive standalone path-tree ABI; existing pack/meta profiles are unchanged.
use crate::{pcfs_sources_v1, pcfs_verified_sources};
use std::ffi::c_char;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_filesystem_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub compression: u32,
    pub root_mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub max_output_bytes: u64,
    pub max_input_bytes: u64,
    pub max_entries: u64,
    pub max_path_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_filesystem_entry_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub path: *const c_char,
    pub path_bytes: u64,
    pub symlink_target: *const c_char,
    pub symlink_bytes: u64,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub source_token: u64,
    pub kind: u32,
    pub mode: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_filesystem_result_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub entries: u64,
    pub files: u64,
    pub input_bytes: u64,
}
extern "C" {
    pub fn pcfs_erofs_build_tree_v1(
        options: *const pcfs_filesystem_options_v1,
        sources: *const pcfs_sources_v1,
        entries: *const pcfs_filesystem_entry_v1,
        count: usize,
        output_fd: i32,
        result: *mut pcfs_filesystem_result_v1,
    ) -> i32;
    pub fn pcfs_erofs_build_tree_verified_v1(
        options: *const pcfs_filesystem_options_v1,
        sources: *mut pcfs_verified_sources,
        entries: *const pcfs_filesystem_entry_v1,
        count: usize,
        output_fd: i32,
        result: *mut pcfs_filesystem_result_v1,
    ) -> i32;
    pub fn pcfs_filesystem_validate_v1(
        options: *const pcfs_filesystem_options_v1,
        entries: *const pcfs_filesystem_entry_v1,
        count: usize,
    ) -> i32;
    pub fn pcfs_filesystem_fingerprint_v1() -> *const c_char;
}
