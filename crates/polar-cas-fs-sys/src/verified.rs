//! Raw ABI for construction-verified inputs. See native/pcfs_verified_sources.h.
use crate::{pcfs_image, pcfs_pack_options_v1, pcfs_pack_result_v1, pcfs_sources_v1};
use std::ffi::c_char;

pub const PCFS_EROFS_PROFILE_BUILD_V1: u32 = 3;
#[repr(C)]
pub struct pcfs_verified_sources {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_source_spec_v1 {
    pub source_token: u64,
    pub size_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_verify_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub block_bytes: u32,
    pub reserved: u32,
    pub max_sources: u64,
    pub max_input_bytes: u64,
    pub max_index_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_source_identity_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pcfs_verified_stats_v1 {
    pub identity_bytes: u64,
    pub encoding_source_bytes: u64,
    pub delivered_bytes: u64,
    pub verified_blocks: u64,
    pub cache_hits: u64,
    pub source_opens: u64,
    pub peak_open_sources: u64,
    pub index_bytes: u64,
    pub cache_bytes: u64,
}
#[repr(C)]
pub struct pcfs_pack_build_result_v2 {
    pub pack: pcfs_pack_result_v1,
    pub members_digest: [u8; 32],
    pub identity_bytes: u64,
    pub encoding_source_bytes: u64,
    pub delivered_bytes: u64,
    pub verified_blocks: u64,
    pub cache_hits: u64,
}
extern "C" {
    pub fn pcfs_verified_sources_identify(
        options: *const pcfs_verify_options_v1,
        sources: *const pcfs_sources_v1,
        specs: *const pcfs_source_spec_v1,
        count: usize,
        out: *mut *mut pcfs_verified_sources,
    ) -> i32;
    pub fn pcfs_verified_sources_identity(
        sources: *const pcfs_verified_sources,
        index: usize,
        out: *mut pcfs_source_identity_v1,
    ) -> i32;
    pub fn pcfs_verified_sources_pread(
        sources: *mut pcfs_verified_sources,
        index: usize,
        offset: u64,
        buffer: *mut u8,
        capacity: usize,
        read_out: *mut usize,
    ) -> i32;
    pub fn pcfs_verified_sources_stats(
        sources: *const pcfs_verified_sources,
        out: *mut pcfs_verified_stats_v1,
    ) -> i32;
    pub fn pcfs_verified_sources_destroy(sources: *mut pcfs_verified_sources);
    pub fn pcfs_erofs_build_verified(
        options: *const pcfs_pack_options_v1,
        sources: *mut pcfs_verified_sources,
        output_fd: i32,
        result: *mut pcfs_pack_build_result_v2,
    ) -> i32;
    pub fn pcfs_erofs_build_verified_selected(
        options: *const pcfs_pack_options_v1,
        sources: *mut pcfs_verified_sources,
        indices: *const usize,
        count: usize,
        output_fd: i32,
        result: *mut pcfs_pack_build_result_v2,
    ) -> i32;
    pub fn pcfs_pack_summary_read(
        image: *mut pcfs_image,
        buffer: *mut c_char,
        capacity: usize,
        length: *mut usize,
    ) -> i32;
}
