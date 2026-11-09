//! Local Dictate policy ABI. Native adapters serialize calls on one background
//! queue and deliver returned effects to their UI thread in order.
//!
//! Persist `snapshot` before executing effects. If persisting an Insert fails,
//! report `delivery: unchanged` without touching the editor. Never replay saved
//! effects; restore the snapshot through this ABI instead.
use crate::dictate_speech::DictateSpeech;
use polar_tmux_workspace::dictate::{Input, application::MAX_INPUT};
use serde_json::{Value, json};
use std::ffi::{CString, c_char};


/// Static product shortcut declaration, shared by both native registrations.
#[repr(C)]
pub struct StationDictateShortcut {
    pub mac_keycode: u32,
    pub linux_keyval: u32,
    pub label: *const c_char,
    pub mac_label: *const c_char,
    pub linux_label: *const c_char,
    pub xdg_trigger: *const c_char,
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_dictate_shortcut() -> StationDictateShortcut {
    StationDictateShortcut {
        mac_keycode: 63,
        linux_keyval: 0xffeb,
        label: polar_tmux_workspace::dictate::SHORTCUT_LABEL.as_ptr(),
        mac_label: polar_tmux_workspace::dictate::SHORTCUT_MAC_LABEL.as_ptr(),
        linux_label: polar_tmux_workspace::dictate::SHORTCUT_LINUX_LABEL.as_ptr(),
        xdg_trigger: polar_tmux_workspace::dictate::SHORTCUT_TRIGGER.as_ptr(),
    }
}

pub struct DictateHandle {
    application: polar_tmux_workspace::dictate::application::Application<crate::dictate_adapter::StationDictateIo>,
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_dictate_new() -> *mut DictateHandle {
    Box::into_raw(Box::new(DictateHandle {
        application: polar_tmux_workspace::dictate::application::Application::new(crate::dictate_adapter::StationDictateIo::default()),
    }))
}
/// # Safety
/// `handle` is live and exclusively borrowed. `bytes` spans `length` readable
/// bytes. Free returned strings with `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_dispatch(
    handle: *mut DictateHandle,
    bytes: *const u8,
    length: usize,
) -> *mut c_char {
    let result: Result<Value, String> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || length == 0 || length > MAX_INPUT {
            return Err("Invalid Dictate input".into());
        }
        let value: Value = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, length) })
            .map_err(|e| e.to_string())?;
        // Observe native receipts only; no transcript, context or audio is logged.
        if let (Some(stage @ ("capture_ready" | "stop" | "transcribed" | "rewritten" | "delivery")), Some(id)) =
            (value["type"].as_str(), value["operation_id"].as_str()) {
            tracing::info!(target: "polar_dictate::latency", operation_id=id, event=stage,
                outcome=value["outcome"].as_str().unwrap_or(""), "Dictate native stage");
        }
        let application = &mut unsafe { &mut *handle }.application;
        Ok(application.dispatch(value).unwrap_or_else(|error| application.failure_response(error)))
    }))
    .unwrap_or_else(|_| Err("Dictate state failed".into()));
    let value = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The handle is null or owned and live; no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_free(handle: *mut DictateHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn polar_station_dictate_speech_new() -> *mut DictateSpeech {
    std::panic::catch_unwind(DictateSpeech::local)
        .ok()
        .and_then(Result::ok)
        .map(|speech| Box::into_raw(Box::new(speech)))
        .unwrap_or(std::ptr::null_mut())
}

/// Reserve on the UI thread before scheduling a worker. Consume each successful
/// reservation with one transcription call, even after cancellation.
/// # Safety
/// Handle is live and ID is readable for its length; free must not race.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_reserve(
    handle: *const DictateSpeech,
    id: *const u8,
    length: usize,
) -> bool {
    if handle.is_null() || id.is_null() || length == 0 || length > 128 {
        return false;
    }
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(id, length) })
        .is_ok_and(|id| unsafe { &*handle }.reserve(id).is_ok())
}

/// Blocking transcription of a reserved operation; call from a native worker,
/// never the UI thread. A cancelled reservation is consumed without uploading.
/// # Safety
/// Handle stays live throughout the call. ID and PCM pointers are readable for
/// their lengths. Cancellation may run concurrently; free must not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_transcribe(
    handle: *const DictateSpeech,
    id: *const u8,
    id_length: usize,
    pcm: *const u8,
    pcm_length: usize,
) -> *mut c_char {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value, String> {
            if handle.is_null() || id.is_null() || id_length == 0 || id_length > 128 {
                return Err("Invalid Dictate audio request".into());
            }
            let operation_id =
                std::str::from_utf8(unsafe { std::slice::from_raw_parts(id, id_length) })
                    .map_err(|_| "Invalid Dictate operation")?
                    .to_owned();
            // Invalid PCM still consumes this operation's reservation, so an
            // adapter error cannot leave the application permanently busy.
            let audio = if !pcm.is_null()
                && (0..=19_200_000).contains(&pcm_length)
                && pcm_length.is_multiple_of(2)
            {
                unsafe { std::slice::from_raw_parts(pcm, pcm_length) }.to_vec()
            } else {
                vec![]
            };
            let input = match unsafe { &*handle }.transcribe_reserved(operation_id.clone(), audio) {
                Ok(text) => Input::Transcribed { operation_id, text },
                Err(error) => Input::Failed {
                    operation_id,
                    error,
                },
            };
            Ok(json!({"ok":true,"input":input}))
        }))
        .unwrap_or_else(|_| Err("Dictate worker failed".into()));
    let value = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// Handle is live; ID is readable for its length. May race transcription.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_cancel(
    handle: *const DictateSpeech,
    id: *const u8,
    length: usize,
) {
    if handle.is_null() || id.is_null() || length == 0 || length > 128 {
        return;
    }
    if let Ok(id) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(id, length) }) {
        unsafe { &*handle }.cancel(id);
    }
}

/// # Safety
/// Handle is null or uniquely owned; all worker calls have returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_free(handle: *mut DictateSpeech) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// A non-null callback context remains live until unregister returns. Callbacks
/// may run on the capture writer thread and must marshal UI work to its owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_set_progress(
    handle: *const DictateSpeech,
    callback: Option<crate::dictate_speech::ProgressCallback>,
    context: *mut std::ffi::c_void,
) {
    if !handle.is_null() {
        unsafe { &*handle }.set_progress(callback, context as usize);
    }
}

/// Shared Voice settings and capture command adapter. Blocking service requests
/// run on a native background worker; capture_begin only schedules the writer.
/// # Safety
/// Handle and bytes remain valid for the call, free cannot race.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_service_request(
    handle: *const DictateSpeech,
    bytes: *const u8,
    length: usize,
) -> *mut c_char {
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value,String> {
        if handle.is_null() || bytes.is_null() || length==0 || length>512*1024 {return Err("Invalid Voice request".into());}
        let value:Value=serde_json::from_slice(unsafe {std::slice::from_raw_parts(bytes,length)}).map_err(|_|"Invalid Voice request")?;
        let service=unsafe {&*handle};
        polar_tmux_workspace::dictate::application::service_request(service, value)
    })).unwrap_or_else(|_|Err("Voice client failed".into()));
    let value = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// Nonblocking bounded canonical PCM ingestion. False means stop capture and
/// retain the Host's acknowledged prefix; no unbounded native queue is created.
/// # Safety
/// Handle, ID and PCM are readable for their lengths. Free cannot race.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_speech_append(
    handle: *const DictateSpeech,
    id: *const u8,
    id_length: usize,
    pcm: *const u8,
    pcm_length: usize,
) -> bool {
    if handle.is_null()
        || id.is_null()
        || pcm.is_null()
        || id_length == 0
        || id_length > 128
        || pcm_length > 32000
    {
        return false;
    }
    let Ok(id) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(id, id_length) }) else {
        return false;
    };
    unsafe { &*handle }
        .append(id, unsafe { std::slice::from_raw_parts(pcm, pcm_length) })
        .is_ok()
}

/// Local microphone-test playback only. Does not contact a Host or provider.
/// # Safety
/// Handle and PCM are readable and remain live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_dictate_local_wav(
    handle: *const DictateSpeech,
    pcm: *const u8,
    length: usize,
) -> *mut c_char {
    if handle.is_null() || pcm.is_null() || length > 320000 {
        return std::ptr::null_mut();
    }
    let result =
        unsafe { &*handle }.local_wav(unsafe { std::slice::from_raw_parts(pcm, length) }, None);
    let value = match result {
        Ok(payload) => json!({"ok":true,"payload":payload}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|v| CString::new(v).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}


