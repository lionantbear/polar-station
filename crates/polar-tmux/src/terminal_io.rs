//! Shared cancellable Unix terminal I/O; descriptor ownership remains with callers.
use anyhow::{Context, Result};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};
use tokio::io::unix::AsyncFd;

/// A duplicated terminal descriptor registered with Tokio's reactor.
///
/// Tokio's process-global stdin/stdout wrappers use the blocking thread pool,
/// so aborting a task does not cancel an in-flight terminal read or write.
/// Pane Hosts instead make their duplicated descriptor nonblocking and wait
/// through `AsyncFd`, which makes idle input and backpressured output
/// cancellation-safe. The original file-status flags are restored on drop.
pub(crate) struct AsyncTerminalFd {
    fd: AsyncFd<OwnedFd>,
    original_fd: RawFd,
    original_status_flags: libc::c_int,
}

impl AsyncTerminalFd {
    pub(crate) fn duplicate(fd: RawFd) -> Result<Self> {
        // SAFETY: F_GETFL only inspects the live descriptor supplied by the
        // caller and does not retain it.
        let original_status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if original_status_flags < 0 {
            return Err(io::Error::last_os_error())
                .context("failed to read terminal descriptor flags");
        }
        // SAFETY: F_DUPFD_CLOEXEC returns a new descriptor owned by this
        // function on success.
        let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicated < 0 {
            return Err(io::Error::last_os_error())
                .context("failed to duplicate terminal descriptor");
        }
        // SAFETY: `duplicated` is newly owned by this function.
        let duplicated = unsafe { OwnedFd::from_raw_fd(duplicated) };
        // File-status flags are shared by duplicated descriptors. The Drop
        // implementation restores the original flags before this process can
        // return control to any caller that owns the inherited descriptor.
        if unsafe {
            libc::fcntl(
                duplicated.as_raw_fd(),
                libc::F_SETFL,
                original_status_flags | libc::O_NONBLOCK,
            )
        } < 0
        {
            return Err(io::Error::last_os_error())
                .context("failed to make terminal descriptor nonblocking");
        }
        // SAFETY: the owned duplicate stays open and is never replaced while registered.
        let fd = match unsafe { AsyncFd::register(duplicated) }.map_err(io::Error::from) {
            Ok(fd) => fd,
            Err(error) => {
                // SAFETY: the inherited descriptor remains live here and the
                // saved flags came from that exact open file description.
                let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, original_status_flags) };
                return Err(error).context("failed to register terminal descriptor");
            }
        };
        let registered_fd = fd.as_raw_fd();
        Ok(Self {
            fd,
            original_fd: registered_fd,
            original_status_flags,
        })
    }

    pub(crate) async fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut readiness = self.fd.readable().await?;
            match readiness.try_io(|inner| {
                // SAFETY: the registered descriptor is live and `buffer` is
                // writable for the exact length supplied.
                let read = unsafe {
                    libc::read(
                        inner.get_ref().as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                };
                if read < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(read as usize)
                }
            }) {
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    pub(crate) async fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut readiness = self.fd.writable().await?;
            match readiness.try_io(|inner| {
                // SAFETY: the registered descriptor is live and `bytes` is
                // readable for the exact length supplied.
                let written = unsafe {
                    libc::write(
                        inner.get_ref().as_raw_fd(),
                        bytes.as_ptr().cast(),
                        bytes.len(),
                    )
                };
                if written < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(written as usize)
                }
            }) {
                Ok(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(Ok(written)) => bytes = &bytes[written..],
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => continue,
            }
        }
        Ok(())
    }
}

impl Drop for AsyncTerminalFd {
    fn drop(&mut self) {
        // SAFETY: the registered duplicate is still live during Drop and the
        // saved flags came from this exact shared open file description.
        let _ = unsafe { libc::fcntl(self.original_fd, libc::F_SETFL, self.original_status_flags) };
    }
}
