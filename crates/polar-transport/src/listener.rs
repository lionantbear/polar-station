//! Runtime-only listener policy. Install configuration never enables fallback.
use std::{
    io,
    net::{SocketAddr, TcpListener},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListenerBindPolicy {
    #[default]
    Strict,
    BootstrapAuto,
}

impl ListenerBindPolicy {
    /// Return the owned socket, not a port discovered by a disposable probe.
    pub fn bind_tcp(self, preferred: SocketAddr) -> io::Result<TcpListener> {
        match TcpListener::bind(preferred) {
            Err(error)
                if self == Self::BootstrapAuto && error.kind() == io::ErrorKind::AddrInUse =>
            {
                TcpListener::bind(SocketAddr::new(preferred.ip(), 0))
            }
            result => result,
        }
    }
}

/// Inspect structured causes, never localized error messages.
pub fn is_address_in_use(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::AddrInUse)
        {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}


