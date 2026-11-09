//! ABI for bounded, decoded provider coverage. No filesystem positions escape.
use crate::pcfs_image;
use std::ffi::c_void;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_required_content_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct pcfs_coverage_result_v1 {
    pub covered: u64,
    pub probes: u64,
    pub verified_bytes: u64,
}
extern "C" {
    pub fn pcfs_pack_verify_coverage(
        providers: *const *mut pcfs_image,
        provider_count: usize,
        requirements: *const pcfs_required_content_v1,
        requirement_count: usize,
        max_probes: u64,
        max_verify_bytes: u64,
        is_cancelled: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        cancel_context: *mut c_void,
        selected: *mut u32,
        result: *mut pcfs_coverage_result_v1,
    ) -> i32;
}
