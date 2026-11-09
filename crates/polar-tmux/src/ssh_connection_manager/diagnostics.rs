//! Bounded compatibility recovery snapshot; writes run on the existing writer thread.
use super::*;

pub(super) fn record_startup_failure(
    inner: &ManagerInner,
    target_id: &str,
    stage: &'static str,
    errno: Option<i32>,
) {
    let failure = StartupFailure {
        stage,
        errno,
        elapsed_ms: inner
            .trace_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    };
    inner
        .latest_startup_failures
        .lock()
        .expect("SSH startup failure map poisoned")
        .insert(target_id.to_owned(), failure);
    tracing::warn!(target_id, stage, errno, "SSH startup failed");
    record_recovery_trace(inner, "ssh_startup_failed", None, None, None);
}

pub(super) fn record_recovery_trace(
    inner: &ManagerInner,
    stage: &'static str,
    status: Option<SshConnectionStatus>,
    generation: Option<u64>,
    route: Option<SshRouteKind>,
) {
    let status = status.map(|status| match status {
        SshConnectionStatus::Connecting => "connecting",
        SshConnectionStatus::Connected => "connected",
        SshConnectionStatus::Recovering => "recovering",
        SshConnectionStatus::Offline => "offline",
    });
    let entry = RecoveryTraceEntry {
        sequence: inner.trace_sequence.fetch_add(1, Ordering::Relaxed),
        elapsed_ms: inner
            .trace_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        stage,
        status,
        generation,
        route,
        child_exit_code: None,
        child_signal: None,
        target_id: None,
        master_exit: None,
    };
    append_recovery_trace(inner, entry);
}

pub(super) fn append_recovery_trace(inner: &ManagerInner, entry: RecoveryTraceEntry) {
    let mut entries = inner
        .trace_entries
        .lock()
        .expect("SSH recovery trace poisoned");
    if entries.len() == RECOVERY_TRACE_CAPACITY {
        entries.pop_front();
    }
    entries.push_back(entry);
    drop(entries);
    // Bounded notification only: serialization, fsync and rename run off the
    // transport/terminal task. A full slot already represents the latest buffer.
    let outcome = if inner.trace_flush.try_send(()).is_ok() {
        "queued"
    } else {
        "coalesced"
    };
    tracing::info!(target: "polar_station_operation", component="Diagnostics", event="recovery_trace_write_requested", outcome);
}

pub(super) fn flush_recovery_trace(inner: &ManagerInner) {
    tracing::info!(target: "polar_station_operation", component="Diagnostics", event="recovery_trace_write_execute");
    let entries = inner
        .trace_entries
        .lock()
        .expect("SSH recovery trace poisoned")
        .clone();
    let latest_startup_failures = inner
        .latest_startup_failures
        .lock()
        .expect("SSH startup failure map poisoned")
        .clone();
    if create_private_directory(&inner.runtime_root).is_err() {
        return;
    }
    let trace = match serde_json::to_vec_pretty(&RecoveryTraceFile {
        schema_version: "1",
        attempts: &entries,
        latest_startup_failures: &latest_startup_failures,
    }) {
        Ok(trace) => trace,
        Err(_) => return,
    };
    let sequence = inner.trace_sequence.load(Ordering::Relaxed);
    let temporary = inner
        .runtime_root
        .join(format!(".{RECOVERY_TRACE_NAME}.{sequence}.tmp"));
    let final_path = inner.runtime_root.join(RECOVERY_TRACE_NAME);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let Ok(mut file) = options.open(&temporary) else {
        return;
    };
    if file.write_all(&trace).is_err() || file.sync_all().is_err() {
        let _ = fs::remove_file(&temporary);
        return;
    }
    drop(file);
    if fs::rename(&temporary, &final_path).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}
