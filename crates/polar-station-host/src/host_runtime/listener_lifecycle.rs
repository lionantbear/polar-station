//! Listener availability follows the owned accept task, including cancellation/panic.
use std::{io, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

pub(super) struct ListenerReadiness(Arc<AtomicBool>);
impl ListenerReadiness {
    pub(super) fn new(ready: Arc<AtomicBool>) -> Self {
        ready.store(true, Ordering::Release);
        Self(ready)
    }
}
impl Drop for ListenerReadiness {
    fn drop(&mut self) { self.0.store(false, Ordering::Release); }
}

fn recoverable(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionAborted)
        || matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM))
}

pub(super) async fn accept(listener: &tokio::net::UnixListener, ready: &AtomicBool)
    -> io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)>
{
    let mut delay = Duration::from_millis(100);
    let mut deadline = None;
    loop {
        match listener.accept().await {
            Ok(value) => { ready.store(true, Ordering::Release); return Ok(value); }
            Err(error) => {
                ready.store(false, Ordering::Release);
                tracing::error!(errno=error.raw_os_error(), error=%error, "Station IPC accept failed");
                let deadline = *deadline.get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_secs(10));
                if !recoverable(&error) || tokio::time::Instant::now() >= deadline { return Err(error); }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
    }
}




