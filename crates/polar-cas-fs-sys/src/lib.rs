//! Raw ABI for the owned positional-I/O adapter needed by the EROFS importer.
//!
//! Enable `erofs` for the direct, pinned liberofs build/read API.
//! Native integration is separate from the portable I/O-boundary tests.
//! All calls require the lifetime, ownership and buffer contracts in pcfs_io.h.
#![allow(non_camel_case_types)]
#![cfg(target_os = "linux")]

use std::ffi::{c_char, c_void};

pub const PCFS_ABI_V1: u32 = 1;
pub const PCFS_NO_FD: i32 = 1;
pub const PCFS_OK: i32 = 0;
pub const PCFS_INVALID: i32 = -1;
pub const PCFS_IO: i32 = -2;
pub const PCFS_CANCELLED: i32 = -3;
pub const PCFS_LIMIT: i32 = -4;
pub const PCFS_UNSUPPORTED: i32 = -5;
pub const PCFS_CORRUPT: i32 = -6;
pub const PCFS_NOT_FOUND: i32 = -7;
pub const PCFS_IO_MAX_TRANSFER: usize = 1024 * 1024;
pub const PCFS_SHA256_PATH_CAPACITY: usize = 83;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_sources_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub context: *mut c_void,
    pub open_fd: Option<unsafe extern "C" fn(*mut c_void, u64, *mut i32) -> i32>,
    pub read_at:
        Option<unsafe extern "C" fn(*mut c_void, u64, u64, *mut u8, usize, *mut usize) -> i32>,
    pub is_cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}
#[repr(C)]
pub struct pcfs_source {
    _private: [u8; 0],
}
#[repr(C)]
pub struct pcfs_output {
    _private: [u8; 0],
}

extern "C" {
    pub fn pcfs_source_open(
        callbacks: *const pcfs_sources_v1,
        token: u64,
        size: u64,
        out: *mut *mut pcfs_source,
    ) -> i32;
    pub fn pcfs_source_pread(
        source: *mut pcfs_source,
        offset: u64,
        buffer: *mut u8,
        capacity: usize,
        read_out: *mut usize,
    ) -> i32;
    pub fn pcfs_source_pread_exact(
        source: *mut pcfs_source,
        offset: u64,
        buffer: *mut u8,
        length: usize,
    ) -> i32;
    pub fn pcfs_source_check(source: *mut pcfs_source) -> i32;
    pub fn pcfs_source_destroy(source: *mut pcfs_source);
    pub fn pcfs_output_new(fd: i32, max_bytes: u64, out: *mut *mut pcfs_output) -> i32;
    pub fn pcfs_output_pwrite(
        output: *mut pcfs_output,
        offset: u64,
        buffer: *const u8,
        length: usize,
    ) -> i32;
    pub fn pcfs_output_truncate(output: *mut pcfs_output, size: u64) -> i32;
    pub fn pcfs_output_finish(output: *mut pcfs_output, final_size: *mut u64) -> i32;
    pub fn pcfs_output_destroy(output: *mut pcfs_output);
    pub fn pcfs_sha256_path(digest: *const u8, output: *mut c_char, capacity: usize) -> i32;
}

#[cfg(feature = "erofs")]
mod erofs;
#[cfg(feature = "erofs")]
pub use erofs::*;

#[cfg(feature = "erofs")]
mod native_meta;
#[cfg(feature = "erofs")]
pub use native_meta::*;

pub const PCFS_VERITY_NOT_ENABLED: i32 = -8;
pub const PCFS_VERITY_DENIED: i32 = -9;
pub const PCFS_VERITY_BUSY: i32 = -10;
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct pcfs_verity_digest_v1 {
    pub algorithm: u32,
    pub digest_size: u32,
    pub digest: [u8; 64],
}
extern "C" {
    pub fn pcfs_verity_require(
        fd: i32,
        enable_if_missing: u32,
        out: *mut pcfs_verity_digest_v1,
        os_error: *mut i32,
    ) -> i32;
}

extern "C" {
    pub fn pcfs_native_mount_detached(
        meta_fd: i32,
        pack_fds: *const i32,
        pack_count: usize,
        allow_exec: u32,
        mount_fd: *mut i32,
        os_error: *mut i32,
    ) -> i32;
}

extern "C" {
    pub fn pcfs_overlay_mount_detached(
        meta_root: i32,
        data_fds: *const i32,
        data_count: usize,
        upper: i32,
        work: i32,
        allow_exec: u32,
        mount_fd: *mut i32,
        os_error: *mut i32,
    ) -> i32;
    pub fn pcfs_overlay_readonly_v1(mount_fd: i32, os_error: *mut i32) -> i32;
    pub fn pcfs_frozen_link_content(
        root: i32,
        source: i32,
        sha256: *const u8,
        size: u64,
        os_error: *mut i32,
    ) -> i32;
}

#[cfg(feature = "erofs")]
mod coverage;
#[cfg(feature = "erofs")]
pub use coverage::*;

#[cfg(feature = "erofs")]
mod verified;
#[cfg(feature = "erofs")]
pub use verified::*;

mod tree_observer;
pub use tree_observer::*;

extern "C" {
    pub fn pcfs_generated_seal(
        writer_fd: i32,
        directory_fd: i32,
        leaf: *const std::ffi::c_char,
        expected_size: u64,
        read_only_fd: *mut i32,
    ) -> i32;
}

#[cfg(feature = "erofs")]
mod filesystem_pack;
#[cfg(feature = "erofs")]
pub use filesystem_pack::*;
