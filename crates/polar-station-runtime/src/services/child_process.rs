//! Lifetime protection for Host-owned helper processes, never tmux servers.

#[cfg(target_os = "linux")]
pub(super) fn configure_parent_lease(command: &mut tokio::process::Command) -> std::io::Result<()> {
    let expected_parent = unsafe { libc::getpid() };
    // SAFETY: only async-signal-safe syscalls run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn configure_parent_lease(
    _command: &mut tokio::process::Command,
) -> std::io::Result<()> {
    Ok(())
}
