//! Same boxed-buffer ABI as the existing edge presentation module.
use std::cell::RefCell;
thread_local! { static READER: RefCell<crate::bridge::Reader> = RefCell::default(); }
const MAX_INPUT: usize = 32 * 1024 * 1024;

#[no_mangle]
pub extern "C" fn lgp_reader_alloc(length: u32) -> *mut u8 {
    if length as usize > MAX_INPUT { return std::ptr::null_mut(); }
    Box::into_raw(vec![0u8;length as usize].into_boxed_slice()).cast::<u8>()
}

/// # Safety
/// The allocation must have been returned by this module and freed only once.
#[no_mangle]
pub unsafe extern "C" fn lgp_reader_free(pointer: *mut u8, length: u32) {
    if !pointer.is_null() { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(pointer,length as usize))); }
}

/// # Safety
/// `pointer..pointer+length` must be readable Wasm memory for this call.
#[no_mangle]
pub unsafe extern "C" fn lgp_reader_dispatch(pointer: *const u8, length: u32) -> u64 {
    if pointer.is_null() || length as usize > MAX_INPUT { return 0; }
    let input=std::slice::from_raw_parts(pointer,length as usize);
    let result=serde_json::from_slice(input).map_err(|error|error.to_string())
        .and_then(|input|READER.with(|reader|reader.borrow_mut().dispatch(input)));
    let output=match result {
        Ok(value)=>serde_json::json!({"ok":true,"value":value}),
        Err(error)=>serde_json::json!({"ok":false,"error":error}),
    };
    let Ok(bytes)=serde_json::to_vec(&output) else {return 0};
    let length=bytes.len() as u32;
    let pointer=Box::into_raw(bytes.into_boxed_slice()).cast::<u8>() as u32;
    (u64::from(length)<<32)|u64::from(pointer)
}
