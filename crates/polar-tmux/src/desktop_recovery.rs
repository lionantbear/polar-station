//! Control Center recovery policy available without an IPC connection or projection.
//! Native adapters retain editor bytes and revision snapshots; only an accepted
//! revision-checked Host result can turn that draft into applied configuration.

use std::ffi::c_char;

#[repr(C)]
pub struct RecoveryView {
    pub can_submit: bool,
    pub local_enabled: bool,
    pub status: *const c_char,
    pub save_label: *const c_char,
    pub host_label: *const c_char,
    pub local_label: *const c_char,
    pub destination_label: *const c_char,
}

/// All returned strings have static lifetime. Remote transport state is
/// deliberately absent: it cannot gate native Local entry or Host editing.
#[no_mangle]
pub extern "C" fn polar_station_recovery_view(
    connected: bool,
    target_known: bool,
    pending: bool,
) -> RecoveryView {
    let can_submit = connected && target_known && !pending;
    let status = if pending {
        c"Host change pending confirmation; draft is not yet applied."
    } else if !target_known {
        c"Host configuration unavailable. Draft is unapplied and kept in this window. Reopen Host after reconnect to review its target before applying."
    } else if !connected {
        c"Host unavailable. Draft is unapplied and kept in this window. Local terminal remains available."
    } else {
        c"Review the Host target before applying. Draft remains unapplied until Host confirms."
    };
    RecoveryView {
        can_submit,
        local_enabled: true,
        status: status.as_ptr(),
        save_label: if can_submit { c"Apply" } else { c"Keep Draft" }.as_ptr(),
        host_label: c"Host".as_ptr(),
        local_label: c"Local Terminal".as_ptr(),
        destination_label: c"SSH destination".as_ptr(),
    }
}


