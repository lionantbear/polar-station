use crate::pcfs_sources_v1;
use std::ffi::c_char;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_pack_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub profile: u32,
    pub compression: u32,
    pub max_output_bytes: u64,
    pub max_input_bytes: u64,
    pub max_objects: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct pcfs_pack_entry_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub source_token: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct pcfs_pack_result_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub unique_objects: u64,
    pub input_bytes: u64,
}
#[repr(C)]
pub struct pcfs_image {
    _private: [u8; 0],
}
#[repr(C)]
pub struct pcfs_member {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct pcfs_io_counters_v1 {
    pub whole_image_hash_calls: u64,
    pub whole_image_hash_bytes: u64,
    pub image_open_hash_calls: u64,
    pub image_open_hash_bytes: u64,
    pub member_verify_calls: u64,
    pub member_verify_bytes: u64,
    pub member_read_calls: u64,
    pub member_read_bytes: u64,
    pub image_pread_calls: u64,
    pub image_pread_bytes: u64,
}
extern "C" {
    pub fn pcfs_erofs_build(
        options: *const pcfs_pack_options_v1,
        sources: *const pcfs_sources_v1,
        entries: *const pcfs_pack_entry_v1,
        count: usize,
        output_fd: i32,
        result: *mut pcfs_pack_result_v1,
    ) -> i32;
    pub fn pcfs_image_open(
        fd: i32,
        expected_sha256: *const u8,
        size: u64,
        out: *mut *mut pcfs_image,
    ) -> i32;
    pub fn pcfs_image_open_managed(
        fd: i32,
        expected_sha256: *const u8,
        size: u64,
        out: *mut *mut pcfs_image,
    ) -> i32;
    pub fn pcfs_io_counters_read(out: *mut pcfs_io_counters_v1) -> i32;
    pub fn pcfs_image_destroy(image: *mut pcfs_image);
    pub fn pcfs_image_member(
        image: *mut pcfs_image,
        sha256: *const u8,
        size: u64,
        out: *mut *mut pcfs_member,
    ) -> i32;
    pub fn pcfs_image_member_by_digest(
        image: *mut pcfs_image,
        sha256: *const u8,
        size_out: *mut u64,
        out: *mut *mut pcfs_member,
    ) -> i32;
    pub fn pcfs_member_pread(
        member: *mut pcfs_member,
        offset: u64,
        buffer: *mut u8,
        capacity: usize,
        read_out: *mut usize,
    ) -> i32;
    pub fn pcfs_member_verify(member: *mut pcfs_member) -> i32;
    pub fn pcfs_member_destroy(member: *mut pcfs_member);
    pub fn pcfs_member_clone(member: *mut pcfs_member, out: *mut *mut pcfs_member) -> i32;
    pub fn pcfs_erofs_fingerprint() -> *const c_char;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct pcfs_pack_member_v1 {
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}
extern "C" {
    pub fn pcfs_image_members(
        image: *mut pcfs_image,
        rows: *const pcfs_pack_member_v1,
        count: usize,
        statuses: *mut i32,
        out: *mut *mut pcfs_member,
    ) -> i32;
    pub fn pcfs_verify_member_cancellable(
        member: *mut pcfs_member,
        cancel: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
        context: *mut std::ffi::c_void,
    ) -> i32;
    pub fn pcfs_pack_members(
        image: *mut pcfs_image,
        out: *mut pcfs_pack_member_v1,
        capacity: usize,
        count: *mut usize,
        cancel: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
        context: *mut std::ffi::c_void,
    ) -> i32;
}
