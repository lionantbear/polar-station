//! Linux directory observation; opaque handles own FDs and copied names.
use crate::pcfs_sources_v1;
use std::ffi::{c_char, c_void};

pub const PCFS_TREE_DIRECTORY: u32 = 1;
pub const PCFS_TREE_FILE: u32 = 2;
pub const PCFS_TREE_SYMLINK: u32 = 3;
pub const PCFS_TREE_NO_SOURCE: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_tree_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub max_depth: u32,
    pub max_directory_fds: u32,
    pub max_entries: u64,
    pub max_sources: u64,
    pub max_input_bytes: u64,
    pub max_path_bytes: u64,
}
#[repr(C)]
pub struct pcfs_tree_cancel_v1 {
    pub context: *mut c_void,
    pub is_cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_tree_entry_v1 {
    pub path: *const c_char,
    pub symlink_target: *const c_char,
    pub size_bytes: u64,
    pub source_token: u64,
    pub kind: u32,
    pub mode: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_tree_stats_v1 {
    pub entry_count: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub symlink_count: u64,
    pub input_bytes: u64,
    pub path_bytes: u64,
    pub directory_walks: u64,
    pub source_opens: u64,
    pub peak_directory_fds: u32,
    pub reserved: u32,
}
#[repr(C)]
pub struct pcfs_observed_tree {
    _private: [u8; 0],
}
extern "C" {
    pub fn pcfs_tree_observe(
        root_fd: i32,
        options: *const pcfs_tree_options_v1,
        cancel: *const pcfs_tree_cancel_v1,
        out: *mut *mut pcfs_observed_tree,
    ) -> i32;
    pub fn pcfs_tree_entry(
        tree: *const pcfs_observed_tree,
        index: usize,
        out: *mut pcfs_tree_entry_v1,
    ) -> i32;
    pub fn pcfs_tree_stats(tree: *const pcfs_observed_tree, out: *mut pcfs_tree_stats_v1) -> i32;
    pub fn pcfs_tree_source_node(
        tree: *const pcfs_observed_tree,
        token: u64,
        entry_index: *mut usize,
    ) -> i32;
    pub fn pcfs_tree_open_source(tree: *mut pcfs_observed_tree, token: u64, out: *mut i32) -> i32;
    pub fn pcfs_tree_revalidate(tree: *mut pcfs_observed_tree) -> i32;
    pub fn pcfs_tree_source_provider(
        tree: *mut pcfs_observed_tree,
        out: *mut pcfs_sources_v1,
    ) -> i32;
    pub fn pcfs_tree_destroy(tree: *mut pcfs_observed_tree);
}

/// Strict directory sealing result; no content digest or admission claim.
pub const PCFS_TREE_LOCK_REQUIRED: i32 = -11;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_tree_lock_report_v1 {
    pub checked_nodes: u64,
    pub changed_nodes: u64,
    pub os_error: i32,
    pub reserved: u32,
}
extern "C" {
    pub fn pcfs_tree_lock_v1(
        tree: *mut pcfs_observed_tree,
        enable: u32,
        report: *mut pcfs_tree_lock_report_v1,
    ) -> i32;
}

/// Live native observation. Serialize explicit fields only, never this ABI layout.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct pcfs_tree_seal_entry_v1 {
    pub device: u64,
    pub inode: u64,
    pub link_count: u64,
    pub size_bytes: u64,
    pub mtime_seconds: i64,
    pub ctime_seconds: i64,
    pub kind: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_nanos: u32,
    pub ctime_nanos: u32,
    pub verity: crate::pcfs_verity_digest_v1,
}
extern "C" {
    pub fn pcfs_tree_seal_entry_v1(
        tree: *mut pcfs_observed_tree,
        index: usize,
        out: *mut pcfs_tree_seal_entry_v1,
    ) -> i32;
}
