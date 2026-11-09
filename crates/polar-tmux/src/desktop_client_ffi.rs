//! Narrow C ABI for native Polar Station presentation adapters.
//!
//! The ABI carries typed JSON control frames and client-state events only. It
//! intentionally has no terminal-byte function: Pane Host remains the sole
//! terminal bridge.

use std::{
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::Mutex,
};

use polar_tmux_protocol::LocalClientFrame;

use crate::desktop_client::{
    DesktopClient, DesktopClientConfig, DesktopContentRequest, DesktopNativeStateRequest,
};

pub type DesktopClientWakeFn = Option<extern "C" fn(*mut c_void)>;

/// Opt-in native invocation boundaries. Sequence correlates concurrent calls,
/// including calls before a Surface has a content trace key (generation zero).
#[no_mangle]
pub extern "C" fn polar_station_trace_native_call(stage: u32, generation: u64, operation: u64) {
    if operation == 0 {
        return;
    }
    let event = match stage {
        26 => "native_process_output_begin",
        27 => "native_process_output_end",
        28 => "native_rebuild_cells_begin",
        29 => "native_rebuild_cells_end",
        30 => "native_draw_frame_begin",
        31 => "native_draw_frame_end",
        32 => "native_draw_lock_acquired",
        33 => "native_output_lock_acquired",
        34 => "native_cursor_timer_fired",
        35 => "native_redraw_requested",
        36 => "native_redraw_invisible",
        37 => "native_redraw_vsync",
        38 => "native_redraw_enqueued",
        39 => "native_redraw_received",
        40 => "native_redraw_dispatched",
        41 => "native_gtk_queue_begin",
        42 => "native_gtk_queue_end",
        43 => "native_gtk_render_begin",
        44 => "native_gtk_render_end",
        45 => "native_redraw_queue_full",
        61 => "native_controller_render_begin",
        62 => "native_controller_render_end",
        63 => "native_gtk_size_begin",
        64 => "native_gtk_size_end",
        65 => "native_resize_coalesce_begin",
        66 => "native_resize_admitted",
        67 => "native_resize_coalesce_flush",
        68 => "native_pty_resize_begin",
        69 => "native_pty_resize_end",
        _ => return,
    };
    tracing::info!(target: "polar_station_operation", component="Native", event, generation, sequence = operation);
}

/// Numeric geometry only; shared diagnostic key identifies the existing Surface.
#[no_mangle]
pub extern "C" fn polar_station_trace_native_grid(
    stage: u32,
    key: u64,
    columns: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
) {
    let event = match stage {
        1 => "native_surface_grid",
        2 => "native_terminal_grid",
        3 => "native_draw_grid",
        _ => return,
    };
    crate::operation_trace::init();
    tracing::info!(target: "polar_station_operation", component="Native", event, generation=key, columns, rows, pixel_width, pixel_height);
}

/// Native presentation milestones. Only bounded identity bytes are accepted;
/// arbitrary text, terminal contents, destinations and titles are excluded.
/// # Safety
/// `identity` must be readable for `length` bytes, or null with length zero.
#[no_mangle]
pub unsafe extern "C" fn polar_station_trace_native(
    stage: u32,
    identity: *const u8,
    length: usize,
    generation: u64,
) {
    if length > 128 || (identity.is_null() && length != 0) {
        return;
    }
    let event = match stage {
        1 => "native_entry",
        2 => "native_window_bound",
        3 => "focus_requested",
        4 => "focus_acquired",
        5 => "surface_frame_presented",
        6 => "native_terminal_read",
        7 => "native_terminal_parsed",
        8 => "native_render_cells_ready",
        9 => "native_draw_submitted",
        10 => "native_surface_trace_identity",
        11 => "native_process_entry",
        12 => "native_global_initialized",
        13 => "native_app_created",
        14 => "native_runtime_initialized",
        15 => "native_layout_apply_begin",
        16 => "native_layout_apply_end",
        17 => "native_surface_create_begin",
        18 => "native_surface_create_end",
        19 => "native_saved_layout_received",
        20 => "native_terminal_init_begin",
        21 => "native_terminal_init_end",
        22 => "native_event_drain",
        23 => "native_window_attach_requested",
        24 => "native_window_attach_execute",
        25 => "native_host_tick",
        46 => "session_create_native_submit",
        47 => "session_create_native_response",
        48 => "session_create_native_projection_applied",
        49 => "session_list_first_paint",
        50 => "native_tab_selection_observed",
        51 => "native_tab_selection_adopted",
        52 => "native_pane_focus_submitted",
        53 => "native_pane_blur_submitted",
        54 => "native_session_assignment_target",
        55 => "native_session_located",
        56 => "native_session_locate_painted",
        57 => "native_pane_controller_enter",
        58 => "native_pane_controller_leave",
        59 => "native_pane_root_match",
        60 => "native_pane_window_active",
        70 => "native_pane_clicked",
        71 => "native_pane_ready_observed",
        72 => "native_pane_ready_synced",
        73 => "native_pending_focus_requested",
        74 => "native_pending_focus_applied",
        75 => "native_surface_focus_observed",
        76 => "native_automatic_focus_suppressed",
        _ => return,
    };
    let bytes = if length == 0 {
        &[][..]
    } else {
        // SAFETY: caller guarantees the bounded slice is readable for this call.
        unsafe { std::slice::from_raw_parts(identity, length) }
    };
    let Ok(identity) = std::str::from_utf8(bytes) else {
        return;
    };
    if !identity
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
    {
        return;
    }
    crate::operation_trace::init();
    if matches!(stage, 52 | 53 | 70) {
        tracing::info!(target: "polar_station_operation", component="Native", event,
            consumer_id=identity, sequence=generation, operation_id=format!("{identity}-{generation}"));
    } else {
        tracing::info!(target: "polar_station_operation", component="Native", event,
            consumer_id=identity, generation);
    }
}

/// GDK presentation feedback uses GLib's monotonic microsecond clock. Zero
/// means unavailable; the event timestamp remains feedback observation time.
/// GTK may return an estimate. Only a matching wp_presentation event can confirm
/// an actual presentation; this API deliberately emits a reported timing.
#[no_mangle]
pub extern "C" fn polar_station_trace_presentation(
    key: u64,
    frame_counter: i64,
    presentation_us: i64,
) {
    if key == 0 || frame_counter < 0 || presentation_us < 0 {
        return;
    }
    crate::operation_trace::init();
    let event = if presentation_us == 0 {
        "surface_presentation_unavailable"
    } else {
        "surface_frame_timing_reported"
    };
    tracing::info!(target: "polar_station_operation", component="Native", event,
        generation=key, frame_counter, presentation_ns=(presentation_us as u64).saturating_mul(1000));
}

pub struct DesktopClientHandle {
    client: Mutex<DesktopClient>,
}

/// Initial terminal projection before an observer is available. Release with
/// `polar_station_desktop_string_free`, like other owned JSON from this ABI.
#[no_mangle]
pub extern "C" fn polar_station_terminal_status_unavailable_json() -> *mut c_char {
    catch_unwind(|| {
        let view = polar_tmux_core::terminal_feedback(
            polar_tmux_protocol::TerminalRecoveryPhaseV1::StatusUnavailable,
            None,
        );
        CString::new(serde_json::to_string(&view).ok()?)
            .ok()
            .map(CString::into_raw)
    })
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// Creates a stopped client from a UTF-8 `DesktopClientConfig` JSON string.
/// Returns null for invalid pointers, JSON, registration, or configuration.
///
/// # Safety
///
/// `config_json` must be a readable NUL-terminated string for this call. The
/// optional callback must remain callable until the returned handle is freed;
/// its context is opaque and is never dereferenced by Rust.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_new(
    config_json: *const c_char,
    wake: DesktopClientWakeFn,
    wake_context: *mut c_void,
) -> *mut DesktopClientHandle {
    catch_unwind(AssertUnwindSafe(|| {
        if config_json.is_null() {
            return None;
        }
        // SAFETY: the caller contract requires a NUL-terminated input pointer
        // that remains valid for this call; the bytes are copied immediately.
        let config = unsafe { CStr::from_ptr(config_json) }.to_str().ok()?;
        let config: DesktopClientConfig = serde_json::from_str(config).ok()?;
        let context = wake_context as usize;
        let client = DesktopClient::new(config, move || {
            if let Some(wake) = wake {
                wake(context as *mut c_void);
            }
        })
        .ok()?;
        Some(Box::into_raw(Box::new(DesktopClientHandle {
            client: Mutex::new(client),
        })))
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// Starts the handle's one background worker; repeated starts are idempotent.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by
/// `polar_station_desktop_client_new` that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_start(
    handle: *mut DesktopClientHandle,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .start()
            .is_ok()
    }))
    .unwrap_or(false)
}

/// Sends one NUL-terminated typed local-client JSON frame without replay.
///
/// # Safety
///
/// `handle` must be null or live, and `frame_json` must be null or a readable
/// NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_send_json(
    handle: *mut DesktopClientHandle, frame_json: *const c_char,
) -> bool {
    // SAFETY: the compatibility wrapper preserves the caller's pointer contract.
    unsafe { polar_station_desktop_client_send_json_status(handle, frame_json) == 0 }
}

/// Returns zero on queue admission, otherwise DesktopSendError's stable code.
/// # Safety
/// Same live-handle and NUL-terminated input contract as send_json.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_send_json_status(
    handle: *mut DesktopClientHandle,
    frame_json: *const c_char,
) -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return polar_tmux_protocol::DesktopSendError::Unavailable as u32;
        };
        if frame_json.is_null() {
            return polar_tmux_protocol::DesktopSendError::InvalidRequest as u32;
        }
        // SAFETY: see `polar_station_desktop_client_new`; this call also
        // copies and parses the bytes before returning.
        let Ok(frame_json) = unsafe { CStr::from_ptr(frame_json) }.to_str() else {
            return polar_tmux_protocol::DesktopSendError::InvalidRequest as u32;
        };
        let Ok(frame) = serde_json::from_str::<LocalClientFrame>(frame_json) else {
            return polar_tmux_protocol::DesktopSendError::InvalidRequest as u32;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .try_send_detailed(frame)
            .map_or_else(|error| error as u32, |()| 0)
    }))
    .unwrap_or(polar_tmux_protocol::DesktopSendError::Unavailable as u32)
}

/// Detach one native waiter; an admitted Host operation is never replayed.
/// # Safety
/// The handle must be live and request_id a readable NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_cancel_request(
    handle: *mut DesktopClientHandle, generation: u64, request_id: *const c_char,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else { return; };
        if request_id.is_null() { return; }
        let Ok(id) = (unsafe { CStr::from_ptr(request_id) }).to_str() else { return; };
        handle.client.lock().unwrap_or_else(|v| v.into_inner()).cancel_request(generation, id);
    }));
}

/// Admits one content request to the existing desktop worker without I/O here.
/// This path is independent of Host control registration.
///
/// # Safety
/// `handle` must be null or live for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_desktop_client_generation(handle: *mut DesktopClientHandle) -> u64 {
    if handle.is_null() { return 0; }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { &*handle }.client.lock().unwrap_or_else(|v| v.into_inner()).generation())).unwrap_or(0)
}

/// # Safety
/// `handle` must be null or live; `request_json` must be null or readable and
/// NUL-terminated for this call. All inputs are copied before returning.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_send_content_json(
    handle: *mut DesktopClientHandle,
    request_json: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        if request_json.is_null() {
            return false;
        }
        let Ok(json) = unsafe { CStr::from_ptr(request_json) }.to_str() else {
            return false;
        };
        let Ok(request) = serde_json::from_str::<DesktopContentRequest>(json) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .try_send_content(request)
            .is_ok()
    }))
    .unwrap_or(false)
}

/// Enqueue native-only state I/O on the existing desktop worker.
///
/// # Safety
/// The handle must be live and request_json must be a readable NUL-terminated
/// UTF-8 string for this call. The request is copied before returning.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_native_state_json(
    handle: *mut DesktopClientHandle,
    request_json: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        if request_json.is_null() {
            return false;
        }
        let Ok(json) = unsafe { CStr::from_ptr(request_json) }.to_str() else {
            return false;
        };
        let Ok(request) = serde_json::from_str::<DesktopNativeStateRequest>(json) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .try_native_state(request)
            .is_ok()
    }))
    .unwrap_or(false)
}

/// Pops one owned UTF-8 JSON event. The caller must release it with
/// `polar_station_desktop_string_free`.
///
/// # Safety
///
/// `handle` must be null or a live, not-yet-freed client pointer.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_pop_event_json(
    handle: *mut DesktopClientHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { handle.as_ref() }?;
        let event = handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .pop_event()?;
        let json = serde_json::to_string(&event).ok()?;
        CString::new(json).ok().map(CString::into_raw)
    }))
    .ok()
    .flatten()
    .unwrap_or(ptr::null_mut())
}

/// Returns whether the current connection generation completed registration.
///
/// # Safety
///
/// `handle` must be null or a live, not-yet-freed client pointer.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_is_ready(
    handle: *mut DesktopClientHandle,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .is_ready()
    }))
    .unwrap_or(false)
}

/// Returns whether a native adapter has an event available to drain.
///
/// # Safety
///
/// `handle` must be null or a live, not-yet-freed client pointer.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_has_pending(
    handle: *mut DesktopClientHandle,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .has_pending()
    }))
    .unwrap_or(false)
}

/// Interrupts the current reconnect backoff or replaces the active generation.
/// Repeated requests coalesce and never replay an admitted client frame.
///
/// # Safety
///
/// `handle` must be null or a live, not-yet-freed client pointer.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_retry_now(
    handle: *mut DesktopClientHandle,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        handle
            .client
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .retry_now()
            .is_ok()
    }))
    .unwrap_or(false)
}

/// Stops and joins the background worker while retaining the handle.
///
/// # Safety
///
/// `handle` must be null or a live, not-yet-freed client pointer.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_stop(handle: *mut DesktopClientHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(handle) = unsafe { handle.as_ref() } {
            handle
                .client
                .lock()
                .unwrap_or_else(|value| value.into_inner())
                .stop();
        }
    }));
}

/// Frees one stopped or running client handle exactly once.
///
/// # Safety
///
/// `handle` must be null or a pointer returned by `new` that has not already
/// been freed. No other thread may use the pointer after this call begins.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_free(handle: *mut DesktopClientHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !handle.is_null() {
            // SAFETY: ownership is returned exactly once by the caller that
            // received this pointer from `polar_station_desktop_client_new`.
            drop(unsafe { Box::from_raw(handle) });
        }
    }));
}

/// Releases one event string returned by `pop_event_json` exactly once.
///
/// # Safety
///
/// `value` must be null or an owned pointer returned by
/// `polar_station_desktop_client_pop_event_json` that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_string_free(value: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !value.is_null() {
            // SAFETY: this pointer must come from the pop function above and
            // may be returned exactly once.
            drop(unsafe { CString::from_raw(value) });
        }
    }));
}

/// Submit a typed Session operation to the existing desktop worker.
/// # Safety
/// Handle is live and request_json is readable NUL-terminated UTF-8 for this call.
#[no_mangle]
pub unsafe extern "C" fn polar_station_desktop_client_session_json(handle: *mut DesktopClientHandle, request_json: *const c_char) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = (unsafe {handle.as_ref()}) else {return false};
        if request_json.is_null() {return false;}
        let Ok(bytes) = unsafe {CStr::from_ptr(request_json)}.to_str() else {return false};
        if bytes.len() > 1024 * 1024 {return false;}
        let Ok(request) = serde_json::from_str::<crate::station_input_client::DesktopSessionRequest>(bytes) else {return false};
        handle.client.lock().unwrap_or_else(|value|value.into_inner()).try_session(request).is_ok()
    })).unwrap_or(false)
}
