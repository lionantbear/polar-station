//! Local-only adapters for shared Station UI semantics. No Host state or geometry.
use polar_tmux_workspace::{STATION_SHORTCUTS, station_shortcut};
use std::{
    ffi::{CString, c_char},
    sync::OnceLock,
};

/// Decode a filesystem image reference through the Resource domain.
/// # Safety
/// `bytes` is readable for `len`; returned text uses the desktop string free ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_image_path(bytes:*const u8,len:usize)->*mut c_char {
    if bytes.is_null()||len==0||len>16384{return std::ptr::null_mut()}
    std::panic::catch_unwind(|| {
        let value=std::str::from_utf8(unsafe{std::slice::from_raw_parts(bytes,len)}).ok()?;
        CString::new(polar_tmux_resources::image_path(value)?).ok().map(CString::into_raw)
    }).ok().flatten().unwrap_or(std::ptr::null_mut())
}

/// Parse a general filesystem reference through the Resource domain.
/// # Safety
/// `bytes` contains `len` readable bytes; returned text uses desktop_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_resource_file_path(bytes:*const u8,len:usize)->*mut c_char {
    if bytes.is_null()||len==0||len>65536{return std::ptr::null_mut()}
    let value=std::str::from_utf8(unsafe {std::slice::from_raw_parts(bytes,len)}).ok();
    value.and_then(|value| polar_tmux_resources::file_open_candidates(value).into_iter().next())
        .and_then(|r|CString::new(r.path).ok()).map_or(std::ptr::null_mut(),CString::into_raw)
}
/// Test whether native clicked text has bounded filesystem candidates. No IO.
/// # Safety
/// `bytes` must contain `len` readable bytes; `hit` is a UTF-8 byte offset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_file_context_candidate(bytes: *const u8, len: usize, hit: usize) -> bool {
    if bytes.is_null() || len == 0 || len > 16384 { return false; }
    let Ok(text) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes,len) }) else { return false; };
    !polar_tmux_resources::click_file_candidates(&polar_tmux_resources::FileClickContext { text: text.into(), hit }).is_empty()
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_resource_file_rule_count()->usize {polar_tmux_resources::file_link_rules().len()}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_resource_file_rule_at(index:usize)->*const c_char {
    static RULES:OnceLock<Vec<CString>>=OnceLock::new();
    RULES.get_or_init(||polar_tmux_resources::file_link_rules().iter().map(|v|CString::new(*v).expect("static file rule")).collect())
        .get(index).map_or(std::ptr::null(),|s|s.as_ptr())
}
pub struct ResourceFileOpenHandle(polar_tmux_resources::FileOpenFlow);
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
    let Ok(input)=serde_json::from_slice::<polar_tmux_resources::FileOpenInput>(unsafe{std::slice::from_raw_parts(bytes,len)}) else {return std::ptr::null_mut()};
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
            .chain(polar_tmux_workspace::shortcuts::shortcut_hints(polar_tmux_workspace::shortcuts::ShortcutHintContext{panes:2,tabs:2,idea:true}))
            .chain(polar_tmux_workspace::shortcuts::shortcut_hints(polar_tmux_workspace::shortcuts::ShortcutHintContext{panes:2,tabs:2,idea:false}))
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

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct ConnectionsHandle(polar_tmux_workspace::ConnectionsEngine);
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
        let input = serde_json::from_slice::<polar_tmux_workspace::ConnectionsInput>(unsafe {
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
pub struct SessionEditorHandle(polar_tmux_workspace::SessionEditorEngine);
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
        let input = serde_json::from_slice::<polar_tmux_workspace::SessionEditorInput>(unsafe {
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
        let input = serde_json::from_slice::<polar_tmux_workspace::AgentCatalogInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        serde_json::to_value(polar_tmux_workspace::project_agent_catalog(input))
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
        let input = serde_json::from_slice::<polar_tmux_workspace::WorkspaceLifecycleInput>(unsafe {
            std::slice::from_raw_parts(bytes, len)
        })
        .map_err(|e| e.to_string())?;
        serde_json::to_value(polar_tmux_workspace::decide_workspace_lifecycle(input))
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

/// Window-local interaction handle; callers serialize dispatch/destruction on the UI thread.
pub struct AttentionHandle(polar_tmux_workspace::AttentionEngine);
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
        let input = serde_json::from_slice::<polar_tmux_workspace::AttentionInput>(unsafe {
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
pub struct StationShortcutGesture {gesture: polar_tmux_workspace::shortcuts::DictateGesture, clock: std::time::Instant}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_gesture_new() -> *mut StationShortcutGesture {
    Box::into_raw(Box::new(StationShortcutGesture{gesture:Default::default(),clock:std::time::Instant::now()}))
}
/// # Safety
/// Handle is live and exclusively borrowed; events: press=1, release=2,
/// discrete nonrepeat activation=3, cancel=4. Returns none=0/start=1/stop=2/use-original=3.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polar_station_shortcut_gesture_event(handle: *mut StationShortcutGesture, event: u32, recording: bool, can_start: bool, can_use_available: bool) -> u32 {
    use polar_tmux_workspace::shortcuts::ShortcutEvent;
    let Some(handle) = (unsafe { handle.as_mut() }) else { return 0; };
    let event = match event { 1 => ShortcutEvent::Press, 2 => ShortcutEvent::Release, 3 => ShortcutEvent::Activate, _ => ShortcutEvent::Cancel };
    handle.gesture.interpret(event, handle.clock.elapsed().as_millis().min(u128::from(u64::MAX)) as u64, recording, can_start, can_use_available) as u32
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
    polar_tmux_workspace::shortcuts::shortcut_hints(polar_tmux_workspace::shortcuts::ShortcutHintContext{panes:context.panes,tabs:context.tabs,idea:context.idea}).count()
}
#[unsafe(no_mangle)]
pub extern "C" fn polar_station_shortcut_hint_at(context:StationShortcutHintContext,index:usize)->StationShortcutView {
    let entries=polar_tmux_workspace::shortcuts::shortcut_hints(polar_tmux_workspace::shortcuts::ShortcutHintContext{panes:context.panes,tabs:context.tabs,idea:context.idea});
    match entries.into_iter().nth(index) {Some(entry)=>StationShortcutView{action:entry.action as u32,number:entry.number,repeatable:entry.repeatable,key:static_c_string(entry.key),label:static_c_string(entry.label)},None=>StationShortcutView{action:0,number:0,repeatable:false,key:std::ptr::null(),label:std::ptr::null()}}
}
