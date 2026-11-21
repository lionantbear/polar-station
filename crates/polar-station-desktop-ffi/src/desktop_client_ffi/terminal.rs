//! Byte-oriented boundary between the Station client and native Termio.

use std::{
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::{Arc, Mutex, Weak},
};

use polar_station_client::desktop_terminal::{DesktopTerminal, DesktopTerminalConfig};
use polar_station_protocol::TerminalSizeV1;

use super::{DesktopClientHandle, DesktopClientWakeFn};
use polar_station_client::desktop_client::DesktopClient;

static PROCESS_CLIENT: Mutex<Weak<Mutex<DesktopClient>>> = Mutex::new(Weak::new());

pub(super) fn register_process_client(client: Weak<Mutex<DesktopClient>>) {
    *PROCESS_CLIENT
        .lock()
        .unwrap_or_else(|value| value.into_inner()) = client;
}

// Drop clears the callback under the same lock used to invoke it. Native
// code can release its context as soon as close returns.
type Wake = Arc<Mutex<Option<(extern "C" fn(*mut c_void), usize)>>>;

pub struct DesktopTerminalHandle {
    terminal: DesktopTerminal,
    wake: Wake,
    pending: Vec<u8>,
    offset: usize,
    output_epoch: u64,
}

/// # Safety
/// Client handle must be live. Config must be NUL-terminated UTF-8. The wake
/// callback may only signal its native event loop and must remain valid until
/// closing the returned handle. It must not call any of these functions.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_open(
    client: *mut DesktopClientHandle,
    config: *const c_char,
    callback: DesktopClientWakeFn,
    context: *mut c_void,
) -> *mut DesktopTerminalHandle {
    catch_unwind(AssertUnwindSafe(|| {
        let client = unsafe { client.as_ref() }?;
        unsafe { open(&client.client, config, None, callback, context) }
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// Config, terminal_type and optional locale are readable NUL-terminated
/// strings during this call. Callback follows the terminal_open contract.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_open_selected(
    config: *const c_char,
    terminal_type: *const c_char,
    locale: *const c_char,
    callback: DesktopClientWakeFn,
    context: *mut c_void,
) -> *mut DesktopTerminalHandle {
    catch_unwind(AssertUnwindSafe(|| {
        let client = PROCESS_CLIENT
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .upgrade()?;
        if terminal_type.is_null() {
            return None;
        }
        let terminal_type = unsafe { CStr::from_ptr(terminal_type) };
        let locale = (!locale.is_null()).then(|| unsafe { CStr::from_ptr(locale) });
        unsafe { open(&client, config, Some((terminal_type, locale)), callback, context) }
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

unsafe fn open(
    client: &Mutex<DesktopClient>,
    config: *const c_char,
    environment: Option<(&CStr, Option<&CStr>)>,
    callback: DesktopClientWakeFn,
    context: *mut c_void,
) -> Option<*mut DesktopTerminalHandle> {
    if config.is_null() {
        return None;
    }
    let json = unsafe { CStr::from_ptr(config) }.to_str().ok()?;
    let mut config: DesktopTerminalConfig = serde_json::from_str(json).ok()?;
    if let Some((terminal_type, locale)) = environment {
        if let Some(terminal_type) = bounded_terminal_value(terminal_type) {
            config.terminal_type = terminal_type.into();
        }
        config.locale = locale.and_then(bounded_terminal_value).map(str::to_owned);
    }
    let wake: Wake = Arc::new(Mutex::new(callback.map(|call| (call, context as usize))));
    let terminal = client
        .lock()
        .unwrap_or_else(|value| value.into_inner())
        .open_terminal(config, {
            let wake = wake.clone();
            move || {
                let guard = wake.lock().unwrap_or_else(|value| value.into_inner());
                if let Some((call, context)) = *guard {
                    call(context as *mut c_void);
                }
            }
        })
        .ok()?;
    Some(Box::into_raw(Box::new(DesktopTerminalHandle {
        terminal,
        wake,
        pending: Vec::new(),
        offset: 0,
        output_epoch: 0,
    })))
}

fn bounded_terminal_value(value: &CStr) -> Option<&str> {
    let value = value.to_str().ok()?;
    (!value.is_empty() && value.len() <= 128 && value == value.trim()
        && !value.chars().any(char::is_control))
        .then_some(value)
}

/// # Safety
/// A non-null handle must be live and calls on it serialized. A non-null
/// destination must point to at least capacity writable bytes.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_read(
    handle: *mut DesktopTerminalHandle,
    destination: *mut u8,
    capacity: usize,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_mut() }) else {
            return 0;
        };
        if destination.is_null() || capacity == 0 {
            return 0;
        }
        let epoch = handle.terminal.output_epoch();
        if epoch != handle.output_epoch {
            handle.pending.clear();
            handle.offset = 0;
            handle.output_epoch = epoch;
        }
        // Consume already queued frames in one native call. The parser
        // takes its renderer lock once per batch and never waits for bytes.
        let mut written = 0;
        while written < capacity {
            if handle.offset == handle.pending.len() {
                let Some(bytes) = handle.terminal.read_output() else {
                    break;
                };
                handle.pending = bytes;
                handle.offset = 0;
            }
            let bytes = &handle.pending[handle.offset..];
            let count = bytes.len().min(capacity - written);
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), destination.add(written), count) };
            handle.offset += count;
            written += count;
        }
        written
    }))
    .unwrap_or(0)
}

/// # Safety
/// A non-null handle must be live and calls on it serialized. Bytes must
/// point to length readable bytes for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_write(
    handle: *const DesktopTerminalHandle,
    bytes: *const u8,
    length: usize,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        if bytes.is_null() || length > isize::MAX as usize {
            return false;
        }
        handle
            .terminal
            .write_input(unsafe { std::slice::from_raw_parts(bytes, length) })
            .is_ok()
    }))
    .unwrap_or(false)
}

/// # Safety
/// A non-null handle must be live and calls on it serialized.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_viewport(
    handle: *const DesktopTerminalHandle,
    columns: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
    focused: bool,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        handle
            .terminal
            .set_viewport(
                TerminalSizeV1 {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                },
                focused,
            )
            .is_ok()
    }))
    .unwrap_or(false)
}

/// # Safety
/// A non-null handle must be live and calls on it serialized. The returned
/// string uses polar_station_desktop_string_free.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_status(
    handle: *const DesktopTerminalHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { handle.as_ref() }?;
        let status = serde_json::to_vec(&serde_json::json!({
            "terminal": handle.terminal.status(),
            "control": handle.terminal.control_status(),
        }))
        .ok()?;
        Some(CString::new(status).ok()?.into_raw())
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// A non-null handle must be live. All other calls on it must have finished.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_terminal_close(handle: *mut DesktopTerminalHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { Box::from_raw(handle) };
        *handle
            .wake
            .lock()
            .unwrap_or_else(|value| value.into_inner()) = None;
        drop(handle);
    }));
}


