//! Local-only adapters for shared Station UI semantics. No Host state or geometry.
use polar_station_workspace::{STATION_SHORTCUTS, station_shortcut};
use std::{
    ffi::{CString, c_char},
    sync::OnceLock,
};

/// Compose supplied observations without contacting the Host or modifying state.
/// # Safety
/// `bytes` contains `len` readable bytes; release the result with
/// `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_diagnostics_project(bytes: *const u8, len: usize) -> *mut c_char {
    if bytes.is_null() || len == 0 || len > 1024 * 1024 { return std::ptr::null_mut(); }
    std::panic::catch_unwind(|| {
        let facts = serde_json::from_slice::<polar_station_workspace::DiagnosticsFacts>(unsafe {std::slice::from_raw_parts(bytes, len)}).ok()?;
        let projection = polar_station_workspace::project_diagnostics(&facts);
        CString::new(serde_json::to_vec(&projection).ok()?).ok().map(CString::into_raw)
    }).ok().flatten().unwrap_or(std::ptr::null_mut())
}

/// Decode a filesystem image reference through the Resource domain.
/// # Safety
/// `bytes` is readable for `len`; returned text uses the desktop string free ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_image_path(bytes:*const u8,len:usize)->*mut c_char {
    if bytes.is_null()||len==0||len>16384{return std::ptr::null_mut()}
    std::panic::catch_unwind(|| {
        let value=std::str::from_utf8(unsafe{std::slice::from_raw_parts(bytes,len)}).ok()?;
        CString::new(polar_station_resources::image_path(value)?).ok().map(CString::into_raw)
    }).ok().flatten().unwrap_or(std::ptr::null_mut())
}

/// Parse a general filesystem reference through the Resource domain.
/// # Safety
/// `bytes` contains `len` readable bytes; returned text uses desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_file_path(bytes:*const u8,len:usize)->*mut c_char {
    if bytes.is_null()||len==0||len>65536{return std::ptr::null_mut()}
    let value=std::str::from_utf8(unsafe {std::slice::from_raw_parts(bytes,len)}).ok();
    value.and_then(|value| polar_station_resources::file_open_candidates(value).into_iter().next())
        .and_then(|r|CString::new(r.path).ok()).map_or(std::ptr::null_mut(),CString::into_raw)
}
/// Capture validation only; filesystem syntax and source resolution run in Host.
/// # Safety
/// `bytes` contains `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_file_input_valid(bytes: *const u8, len: usize) -> bool {
    if bytes.is_null() || len == 0 || len > 65536 { return false; }
    let Ok(text) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }) else { return false; };
    polar_station_resources::FileReferenceInput::from_capture(text.into()).is_ok()
}
/// Recognize once and carry the typed result to Host. The native caller only
/// maps the returned original-text ranges to terminal cells.
/// # Safety
/// `bytes` has `len` readable bytes. Free output with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_file_capture(bytes: *const u8, len: usize) -> *mut c_char {
    if bytes.is_null() || len == 0 || len > 65536 { return std::ptr::null_mut(); }
    std::panic::catch_unwind(|| {
        let input = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, len) }).ok()?;
        let result = polar_station_resources::recognize_file_capture(input, polar_station_resources::PathFlavor::Posix).ok()?;
        CString::new(serde_json::to_vec(&result).ok()?).ok().map(CString::into_raw)
    }).ok().flatten().unwrap_or(std::ptr::null_mut())
}

/// Whether a native URL match belongs to the external URI handler.
/// # Safety
/// `bytes` has `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_external_link(bytes: *const u8, len: usize) -> bool {
    if bytes.is_null() || len == 0 || len > 16384 { return false; }
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }).is_ok_and(polar_station_resources::is_external_file_link)
}
pub struct ResourceFileOpenHandle(polar_station_resources::FileOpenFlow);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_resource_file_open_new()->*mut ResourceFileOpenHandle {
    Box::into_raw(Box::new(ResourceFileOpenHandle(Default::default())))
}
/// # Safety
/// Handle is live, exclusively borrowed, and serialized on its owning UI thread.
/// `bytes` contains `len` readable bytes; free output with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_file_open_dispatch(handle:*mut ResourceFileOpenHandle,bytes:*const u8,len:usize)->*mut c_char {
    if handle.is_null()||bytes.is_null()||len>128*1024{return std::ptr::null_mut()}
    let Ok(input)=serde_json::from_slice::<polar_station_resources::FileOpenInput>(unsafe{std::slice::from_raw_parts(bytes,len)}) else {return std::ptr::null_mut()};
    let flow=&mut unsafe{&mut *handle}.0;
    let effects=flow.dispatch(input);
    serde_json::to_string(&serde_json::json!({"pending":flow.is_pending(),"effects":effects})).ok()
        .and_then(|v|CString::new(v).ok()).map_or(std::ptr::null_mut(),CString::into_raw)
}
/// # Safety
/// Handle came from file_open_new and is freed once without a concurrent call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_file_open_free(handle:*mut ResourceFileOpenHandle) {
    if !handle.is_null(){drop(unsafe{Box::from_raw(handle)});}
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StationShortcutView {
    pub action: u32,
    pub number: u8,
    pub repeatable: bool,
    pub key: *const c_char,
    pub label: *const c_char,
}
fn strings() -> &'static Vec<(CString, CString)> {
    static STRINGS: OnceLock<Vec<(CString, CString)>> = OnceLock::new();
    STRINGS.get_or_init(|| {
        STATION_SHORTCUTS.iter().copied()
            .chain(polar_station_workspace::shortcuts::shortcut_hints(polar_station_workspace::shortcuts::ShortcutHintContext{panes:2,tabs:2,idea:true}))
            .chain(polar_station_workspace::shortcuts::shortcut_hints(polar_station_workspace::shortcuts::ShortcutHintContext{panes:2,tabs:2,idea:false}))
            .map(|e| {
                (
                    CString::new(e.key).expect("static key"),
                    CString::new(e.label).expect("static label"),
                )
            })
            .collect()
    })
}
fn static_c_string(value:&str)->*const c_char {
    strings().iter().flat_map(|(key,label)|[key,label]).find(|s|s.as_bytes()==value.as_bytes()).map_or(std::ptr::null(),|s|s.as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_count() -> usize {
    STATION_SHORTCUTS.len()
}
/// Render the shared editor projection from selected fields and optional Session facts.
/// # Safety
/// The input points to `len` readable bytes. Free the result with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_tmux_status_bar_view(bytes: *const u8, len: usize) -> *mut c_char {
    if bytes.is_null() || len == 0 || len > 32 * 1024 {
        return std::ptr::null_mut();
    }
    let input = unsafe { std::slice::from_raw_parts(bytes, len) };
    serde_json::from_slice::<polar_station_workspace::TmuxStatusBarPreview>(input)
        .or_else(|_| serde_json::from_slice(input).map(|fields| polar_station_workspace::TmuxStatusBarPreview { fields, facts: None, purpose: None }))
        .ok()
        .and_then(|value| serde_json::to_vec(&value.view()).ok())
        .and_then(|value| CString::new(value).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// Returns 1-based catalog index, or zero for a key not owned by Station.
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_lookup(platform: u32, key: u32) -> u32 {
    station_shortcut(platform, key)
        .and_then(|entry| {
            STATION_SHORTCUTS
                .iter()
                .position(|e| e.action == entry.action && e.number == entry.number)
        })
        .map_or(0, |i| i as u32 + 1)
}
/// All returned strings have static lifetime, including the invalid-index view.
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_at(index: usize) -> StationShortcutView {
    let Some(entry) = STATION_SHORTCUTS.get(index) else {
        return StationShortcutView {
            action: 0,
            number: 0,
            repeatable: false,
            key: c"".as_ptr(),
            label: c"".as_ptr(),
        };
    };
    let text = &strings()[index];
    StationShortcutView {
        action: entry.action as u32,
        number: entry.number,
        repeatable: entry.repeatable,
        key: text.0.as_ptr(),
        label: text.1.as_ptr(),
    }
}

/// Process-wide Resource editor handle; callers serialize dispatch/destruction on the UI thread.
pub struct ResourceEditorHandle(polar_station_resources::ResourceEditor);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_resource_editor_new() -> *mut ResourceEditorHandle {
    Box::into_raw(Box::new(ResourceEditorHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_editor_dispatch(
    handle: *mut ResourceEditorHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 {
            return Err("Invalid Resource editor input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_resources::ResourceEditorInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        let (view, effects) = engine.dispatch(input);
        Ok(serde_json::json!({"view":view,"effects":effects}))
    }))
    .unwrap_or_else(|_| Err("Resource editor interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_editor_free(handle: *mut ResourceEditorHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct SettingsTextHandle(polar_station_workspace::SettingsTextEngine);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_settings_text_new() -> *mut SettingsTextHandle {
    Box::into_raw(Box::new(SettingsTextHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_text_dispatch(
    handle: *mut SettingsTextHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 {
            return Err("Invalid Settings text input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::SettingsTextInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        let (view,effects)=engine.dispatch(input);
        Ok(serde_json::json!({"view":view,"effects":effects}))
    }))
    .unwrap_or_else(|_| Err("Settings text interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_text_free(handle: *mut SettingsTextHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct SettingsNavigationHandle(polar_station_workspace::SettingsNavigation);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_settings_navigation_new() -> *mut SettingsNavigationHandle {
    Box::into_raw(Box::new(SettingsNavigationHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_navigation_dispatch(
    handle: *mut SettingsNavigationHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 || len > 1024 * 1024 {
            return Err("Invalid Settings navigation input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::SettingsNavigationInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        serde_json::to_value(engine.dispatch(input)).map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("Settings navigation interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_navigation_free(handle: *mut SettingsNavigationHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct DiagnosticsHandle(polar_station_workspace::DiagnosticsEngine);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_diagnostics_new() -> *mut DiagnosticsHandle {
    Box::into_raw(Box::new(DiagnosticsHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_diagnostics_dispatch(
    handle: *mut DiagnosticsHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 || len > 1024 * 1024 {
            return Err("Invalid Diagnostics input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::DiagnosticsInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        serde_json::to_value(engine.dispatch(input)).map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err("Diagnostics interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_diagnostics_free(handle: *mut DiagnosticsHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct ConnectionsHandle(polar_station_workspace::ConnectionsEngine);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_connections_new() -> *mut ConnectionsHandle {
    Box::into_raw(Box::new(ConnectionsHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_connections_dispatch(
    handle: *mut ConnectionsHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 || len > 1024 * 1024 {
            return Err("Invalid Connections input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::ConnectionsInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        let effects = engine.dispatch(input);
        Ok(serde_json::json!({"view":engine.view(),"effects":effects}))
    }))
    .unwrap_or_else(|_| Err("Connections interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_connections_free(handle: *mut ConnectionsHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct SessionEditorHandle(polar_station_workspace::SessionEditorEngine);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_session_editor_new() -> *mut SessionEditorHandle {
    Box::into_raw(Box::new(SessionEditorHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_session_editor_dispatch(
    handle: *mut SessionEditorHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 || len > 1024 * 1024 {
            return Err("Invalid SessionEditor input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::SessionEditorInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        let effects = engine.dispatch(input);
        Ok(serde_json::json!({"view":engine.view(),"effects":effects}))
    }))
    .unwrap_or_else(|_| Err("SessionEditor interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_session_editor_free(handle: *mut SessionEditorHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Projects catalog semantics without owning native layout or UI state.
/// # Safety
/// `bytes` points to `len` readable bytes. Free result with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_agent_catalog_project(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if bytes.is_null() || len == 0 || len > 16 * 1024 * 1024 {
            return Err("Invalid catalog input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::AgentCatalogInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        serde_json::to_value(polar_station_workspace::project_agent_catalog(input))
            .map_err(|e| e.to_string())
    })
    .unwrap_or_else(|_| Err("Catalog projection failed".into()));
    let value = result.unwrap_or_else(|error| serde_json::json!({"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Evaluates native window lifecycle semantics without owning native layout or UI state.
/// # Safety
/// `bytes` points to `len` readable bytes. Free result with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_workspace_lifecycle(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if bytes.is_null() || len == 0 || len > 16 * 1024 * 1024 {
            return Err("Invalid catalog input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::WorkspaceLifecycleInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        serde_json::to_value(polar_station_workspace::decide_workspace_lifecycle(input))
            .map_err(|e| e.to_string())
    })
    .unwrap_or_else(|_| Err("Catalog projection failed".into()));
    let value = result.unwrap_or_else(|error| serde_json::json!({"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Projects Settings metadata without owning the native Workspace catalog.
/// # Safety
/// `bytes` points to `len` readable bytes. Free result with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_workspace_manager_view(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if bytes.is_null() || len == 0 || len > 16 * 1024 * 1024 {
            return Err("Invalid catalog input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::workspace_manager::Input>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        serde_json::to_value(polar_station_workspace::workspace_manager::project(input))
            .map_err(|e| e.to_string())
    })
    .unwrap_or_else(|_| Err("Catalog projection failed".into()));
    let value = result.unwrap_or_else(|error| serde_json::json!({"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Transforms local Workspace exchange files; never contacts the Host.
/// # Safety
/// `bytes` points to `len` readable bytes. Free result with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_workspace_transfer(
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if bytes.is_null() || len == 0 || len > 16 * 1024 * 1024 {
            return Err("Invalid catalog input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::portable_workspace::native::Request>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        polar_station_workspace::portable_workspace::native::dispatch(input)
    })
    .unwrap_or_else(|_| Err("Catalog projection failed".into()));
    let value = result.unwrap_or_else(|error| serde_json::json!({"error":error}));
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct AttentionHandle(polar_station_workspace::AttentionEngine);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_attention_new() -> *mut AttentionHandle {
    Box::into_raw(Box::new(AttentionHandle(Default::default())))
}
/// # Safety
/// `handle` is live and exclusively borrowed for this call; `bytes` contains `len`
/// readable bytes. Returned strings use `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_attention_dispatch(
    handle: *mut AttentionHandle,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if handle.is_null() || bytes.is_null() || len == 0 || len > 1024 * 1024 {
            return Err("Invalid Attention input".to_string());
        }
        let input = serde_json::from_slice::<polar_station_workspace::AttentionInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        let engine = &mut unsafe { &mut *handle }.0;
        let effects = engine.dispatch(input);
        Ok(serde_json::json!(effects))
    }))
    .unwrap_or_else(|_| Err("Attention interaction failed".into()));
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    serde_json::to_string(&value)
        .ok()
        .and_then(|s| CString::new(s).ok())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// The pointer is null or live and no dispatch may race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_attention_free(handle: *mut AttentionHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Main-thread shortcut input handle, independent of background Dictate IO.
pub struct StationShortcutGesture {gesture: polar_station_workspace::shortcuts::DictateGesture, clock: std::time::Instant}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_gesture_new() -> *mut StationShortcutGesture {
    Box::into_raw(Box::new(StationShortcutGesture{gesture:Default::default(),clock:std::time::Instant::now()}))
}
/// # Safety
/// Handle is live and exclusively borrowed; events: press=1, release=2,
/// discrete nonrepeat activation=3, cancel=4. Returns none=0/start=1/stop=2/use-original=3.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_shortcut_gesture_event(handle: *mut StationShortcutGesture, event: u32, recording: bool, can_start: bool, can_use_available: bool) -> u32 {
    let Some(handle) = (unsafe { handle.as_mut() }) else { return 0; };
    let event_ms = handle.clock.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    unsafe { polar_station_shortcut_gesture_event_at(handle, event, event_ms, recording, can_start, can_use_available) }
}
/// Uses the physical event's monotonic timestamp, not its callback arrival time.
/// Use one clock epoch per handle; do not mix timed and untimed calls.
/// # Safety
/// Handle is live and exclusively borrowed, as for the untimed entrypoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_shortcut_gesture_event_at(handle: *mut StationShortcutGesture, event: u32, event_ms: u64, recording: bool, can_start: bool, can_use_available: bool) -> u32 {
    use polar_station_workspace::shortcuts::ShortcutEvent;
    let Some(handle) = (unsafe { handle.as_mut() }) else { return 0; };
    let event = match event { 1 => ShortcutEvent::Press, 2 => ShortcutEvent::Release, 3 => ShortcutEvent::Activate, _ => ShortcutEvent::Cancel };
    handle.gesture.interpret(event, event_ms, recording, can_start, can_use_available) as u32
}
/// # Safety
/// Handle is null or uniquely owned and live; no calls race destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_shortcut_gesture_free(handle: *mut StationShortcutGesture) {
    if !handle.is_null() { drop(unsafe { Box::from_raw(handle) }); }
}



/// Shared contextual hint catalog; the native shell owns measurements and drawing.
#[repr(C)]
#[derive(Clone,Copy,Default)]
pub struct StationShortcutHintContext {pub panes:u32,pub tabs:u32,pub idea:bool}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_hint_count(context:StationShortcutHintContext)->usize {
    polar_station_workspace::shortcuts::shortcut_hints(polar_station_workspace::shortcuts::ShortcutHintContext{panes:context.panes,tabs:context.tabs,idea:context.idea}).count()
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_hint_at(context:StationShortcutHintContext,index:usize)->StationShortcutView {
    let entries=polar_station_workspace::shortcuts::shortcut_hints(polar_station_workspace::shortcuts::ShortcutHintContext{panes:context.panes,tabs:context.tabs,idea:context.idea});
    match entries.into_iter().nth(index) {Some(entry)=>StationShortcutView{action:entry.action as u32,number:entry.number,repeatable:entry.repeatable,key:static_c_string(entry.key),label:static_c_string(entry.label)},None=>StationShortcutView{action:0,number:0,repeatable:false,key:std::ptr::null(),label:std::ptr::null()}}
}

/// Shared native setup copy; free with `polar_station_desktop_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_tool_setup_copy() -> *mut c_char {
    serde_json::to_vec(&polar_station_workspace::tool_setup_copy()).ok()
        .and_then(|bytes|CString::new(bytes).ok())
        .map_or(std::ptr::null_mut(),CString::into_raw)
}

/// Transient edit lease; the original Settings repository remains authoritative.
pub struct SettingsEditHandle(polar_station_workspace::SettingsEditLease);
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_settings_edit_new()->*mut SettingsEditHandle {
    Box::into_raw(Box::new(SettingsEditHandle(Default::default())))
}
/// # Safety
/// Handle is live and exclusively used on its UI thread; bytes is readable for len.
/// Release output with polar_station_desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_edit_dispatch(handle:*mut SettingsEditHandle,bytes:*const u8,len:usize)->*mut c_char {
    if handle.is_null()||bytes.is_null()||len>16384{return std::ptr::null_mut()}
    let Ok(input)=serde_json::from_slice::<polar_station_workspace::SettingsEditInput>(unsafe{std::slice::from_raw_parts(bytes,len)}) else {return std::ptr::null_mut()};
    let view=unsafe{&mut *handle}.0.dispatch(input);
    serde_json::to_string(&view).ok().and_then(|v|CString::new(v).ok()).map_or(std::ptr::null_mut(),CString::into_raw)
}
/// # Safety
/// Handle came from settings_edit_new and is freed once without concurrent calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_settings_edit_free(handle:*mut SettingsEditHandle) {
    if !handle.is_null(){drop(unsafe{Box::from_raw(handle)});}
}



/// Shared local backend preview presentation, without filesystem or network IO.
/// # Safety
/// Bytes must be readable for len. Free returned text with desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_backend_view(bytes:*const u8,len:usize)->*mut c_char {
    if bytes.is_null()||len>128*1024{return std::ptr::null_mut();}
    std::panic::catch_unwind(||{
        let input=serde_json::from_slice(unsafe{std::slice::from_raw_parts(bytes,len)}).ok()?;
        let view=polar_station_workspace::backend_selection::project(input);
        CString::new(serde_json::to_vec(&view).ok()?).ok().map(CString::into_raw)
    }).ok().flatten().unwrap_or(std::ptr::null_mut())
}
