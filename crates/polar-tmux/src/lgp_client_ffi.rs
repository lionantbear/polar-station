//! Narrow native ABI for the shared LGP client state machine. Only the native
//! owner may free a handle; dispatch and free must not race. State contains no
//! Kernel or network client and cannot execute commands by itself.
use std::{ffi::CString,panic::{catch_unwind,AssertUnwindSafe},sync::Mutex};
use polar_station_input::input::Input;
use polar_station_input::InputEngine;

pub struct LgpClientHandle{state:Mutex<InputEngine>}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_lgp_client_new()->*mut LgpClientHandle{
    catch_unwind(||Box::into_raw(Box::new(LgpClientHandle{state:Mutex::new(InputEngine::new())}))).unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// `handle` is an allocated live handle. `bytes` points to `length` readable
/// bytes for the duration of this call. Native adapters hold the owner on their
/// UI thread; do not race destruction. The returned JSON uses the existing
/// `polar_station_desktop_string_free` allocator pairing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_lgp_client_dispatch(handle:*mut LgpClientHandle,bytes:*const u8,length:usize)->*mut std::ffi::c_char{
    let result=catch_unwind(AssertUnwindSafe(||->Result<serde_json::Value,String>{
        if handle.is_null()||bytes.is_null()||length==0||length>1024*1024{return Err("invalid LGP dispatch input".into());}
        let input:Input=serde_json::from_slice(unsafe{std::slice::from_raw_parts(bytes,length)}).map_err(|e|e.to_string())?;
        let mut engine=unsafe{&*handle}.state.lock().map_err(|_|"LGP state unavailable")?;
        let effects=engine.dispatch(input)?;
        serde_json::to_value(effects).map_err(|e|e.to_string())
    })).unwrap_or_else(|_|Err("LGP state panicked; reconnect required".into()));
    let value=match result{Ok(effects)=>serde_json::json!({"ok":true,"effects":effects}),Err(error)=>serde_json::json!({"ok":false,"error":error})};
    serde_json::to_string(&value).ok().and_then(|s|CString::new(s).ok()).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or was returned by `new` and not already freed. All
/// native callbacks and dispatches must be retired before freeing it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_lgp_client_free(handle:*mut LgpClientHandle){
    if !handle.is_null(){let _=catch_unwind(AssertUnwindSafe(||drop(unsafe{Box::from_raw(handle)})));}
}
/// Shared input ABI. The legacy name delegates to the same owner during migration.
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_input_new() -> *mut LgpClientHandle { polar_station_lgp_client_new() }
/// # Safety
/// Same live-handle and readable-buffer requirements as the compatibility dispatch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_input_dispatch(handle:*mut LgpClientHandle, bytes:*const u8, length:usize) -> *mut std::ffi::c_char {
    unsafe { polar_station_lgp_client_dispatch(handle,bytes,length) }
}
/// # Safety
/// The handle must be live, exclusively owned, and never freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_input_free(handle:*mut LgpClientHandle) { unsafe { polar_station_lgp_client_free(handle) } }


