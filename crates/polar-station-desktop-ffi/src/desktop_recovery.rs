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
    let view = polar_tmux_workspace::recovery_view(connected, target_known, pending);
    RecoveryView {
        can_submit: view.can_submit, local_enabled: view.local_enabled,
        status: view.status.as_ptr(), save_label: view.save_label.as_ptr(),
        host_label: view.host_label.as_ptr(), local_label: view.local_label.as_ptr(),
        destination_label: view.destination_label.as_ptr(),
    }
}
