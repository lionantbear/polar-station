//! Socket publication notification, armed before starting a producer or connecting. No timer scans.
//! The parent always validates the exact private socket after a directory event.
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::{io, path::Path};

pub(crate) struct SocketReadiness {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fd: tokio::io::unix::AsyncFd<OwnedFd>,
    #[cfg(target_os = "macos")]
    _directory: OwnedFd,
}



impl SocketReadiness {
    pub(crate) fn new(socket: &Path) -> io::Result<Self> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let directory = std::ffi::CString::new(
            socket
                .parent()
                .ok_or_else(|| io::Error::other("socket parent missing"))?
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|_| io::Error::other("invalid socket parent"))?;
        #[cfg(target_os = "linux")]
        {
            // SAFETY: creation returns a new owned descriptor; path remains live for add_watch.
            let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            if unsafe {
                libc::inotify_add_watch(
                    fd.as_raw_fd(),
                    directory.as_ptr(),
                    libc::IN_CREATE
                        | libc::IN_MOVED_TO
                        | libc::IN_ATTRIB
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF,
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                // SAFETY: OwnedFd retains this descriptor unchanged until deregistration.
                fd: unsafe { tokio::io::unix::AsyncFd::register(fd) }?,
            })
        }
        #[cfg(target_os = "macos")]
        {
            // SAFETY: path is a valid C string and each successful call transfers one descriptor.
            let raw = unsafe { libc::open(directory.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let directory = unsafe { OwnedFd::from_raw_fd(raw) };
            let raw = unsafe { libc::kqueue() };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            // SAFETY: owned descriptor and writable initialized kevent, no retained pointers.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let change = libc::kevent {
                ident: directory.as_raw_fd() as _,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE | libc::NOTE_RENAME | libc::NOTE_DELETE,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            if unsafe {
                libc::kevent(
                    fd.as_raw_fd(),
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                // A kqueue can be watched for reads, but Darwin rejects
                // EVFILT_WRITE. AsyncFd::register requests both interests.
                // SAFETY: OwnedFd retains the kqueue descriptor unchanged until deregistration.
                fd: unsafe {
                    tokio::io::unix::AsyncFd::register_with_interest(
                        fd,
                        tokio::io::Interest::READABLE,
                    )
                }?,
                _directory: directory,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = socket;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket readiness requires a supported desktop",
            ))
        }
    }

    pub(crate) async fn changed(&self) -> io::Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        loop {
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| {
                #[cfg(target_os = "linux")]
                {
                    let mut buffer = [0_u8; 4096];
                    // SAFETY: buffer is writable for its length and fd is owned for this call.
                    let read = unsafe {
                        libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len())
                    };
                    if read < 0 {
                        Err(io::Error::last_os_error())
                    } else if read == 0 {
                        Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "readiness closed",
                        ))
                    } else {
                        Ok(())
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    // SAFETY: initialized result, zero timeout prevents blocking the async executor.
                    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                    let timeout = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    let count = unsafe {
                        libc::kevent(fd.as_raw_fd(), std::ptr::null(), 0, &mut event, 1, &timeout)
                    };
                    if count < 0 {
                        Err(io::Error::last_os_error())
                    } else if count == 0 {
                        Err(io::Error::from(io::ErrorKind::WouldBlock))
                    } else {
                        Ok(())
                    }
                }
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket readiness unavailable",
        ))
    }
}
