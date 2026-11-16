//! Host recovery presentation shared by native adapters; no connection or I/O.
use std::ffi::CStr;

pub struct RecoveryPresentation {
    pub can_submit: bool,
    pub local_enabled: bool,
    pub status: &'static CStr,
    pub save_label: &'static CStr,
    pub host_label: &'static CStr,
    pub local_label: &'static CStr,
    pub destination_label: &'static CStr,
}

pub fn recovery_view(connected: bool, target_known: bool, pending: bool) -> RecoveryPresentation {
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
    RecoveryPresentation {
        can_submit,
        local_enabled: true,
        status,
        save_label: if can_submit { c"Apply" } else { c"Keep Draft" },
        host_label: c"Host",
        local_label: c"Local Terminal",
        destination_label: c"SSH destination",
    }
}


