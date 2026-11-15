//! Native-main-loop readiness, not a frame timer. Worker results stay in their
//! existing owners; the socket transports only readiness and subscription IDs.
use godot::builtin::RustCallable;
use godot::classes::{IRefCounted, RefCounted};
use godot::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

struct Wake {
    reader: Mutex<UnixStream>,
    writer: Mutex<UnixStream>,
    pending: Mutex<HashSet<i64>>,
}
impl Wake {
    fn new() -> std::io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            pending: Mutex::default(),
        })
    }
    fn post(&self, token: i64) {
        let first = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let first = pending.is_empty();
            pending.insert(token);
            first
        };
        if first {
            // A full socket already represents readiness. Never block a worker.
            let _ = self
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .write(&[1]);
        }
    }
    fn take(&self) -> HashSet<i64> {
        let mut reader = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut bytes = [0; 256];
        while matches!(reader.read(&mut bytes), Ok(n) if n > 0) {}
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
static WAKE: LazyLock<Wake> = LazyLock::new(|| Wake::new().expect("graphics event socket"));
static NEXT: AtomicI64 = AtomicI64::new(1);
thread_local! {
    static CALLBACKS: RefCell<HashMap<i64, Callable>> = RefCell::default();
    static DEADLINES: RefCell<HashMap<i64, Instant>> = RefCell::default();
}
pub fn subscribe(callback: Callable) -> i64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    CALLBACKS.with_borrow_mut(|callbacks| callbacks.insert(id, callback));
    id
}
pub fn unsubscribe(id: i64) {
    CALLBACKS.with_borrow_mut(|callbacks| callbacks.remove(&id));
    DEADLINES.with_borrow_mut(|deadlines| deadlines.remove(&id));
}
pub fn post(id: i64) {
    WAKE.post(id);
}

#[derive(PartialEq, Eq, Hash)]
struct WakeToken(i64);
impl std::fmt::Display for WakeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "graphics-wake:{}", self.0)
    }
}
impl RustCallable for WakeToken {
    fn invoke(&mut self, _: &[&Variant]) -> Variant {
        post(self.0);
        Variant::nil()
    }
}

#[derive(GodotClass)]
#[class(base=RefCounted)]
pub struct PolarGraphicsEvents {
    base: Base<RefCounted>,
}
#[godot_api]
impl IRefCounted for PolarGraphicsEvents {
    fn init(base: Base<RefCounted>) -> Self {
        Self { base }
    }
}
#[godot_api]
impl PolarGraphicsEvents {
    #[func]
    fn source_revision(&self, path: GString) -> i64 {
        crate::file_events::revision(&path.to_string())
    }
    #[func]
    fn wake_fd(&self) -> i64 {
        i64::from(
            WAKE.reader
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_raw_fd(),
        )
    }
    #[func]
    fn subscribe(&self, callback: Callable) -> i64 {
        subscribe(callback)
    }
    #[func]
    fn unsubscribe(&self, id: i64) {
        unsubscribe(id);
    }
    #[func]
    fn waker(&self, id: i64) -> Callable {
        Callable::from_custom(WakeToken(id))
    }
    #[func]
    fn schedule(&self, id: i64, milliseconds: i64) {
        DEADLINES.with_borrow_mut(|deadlines| {
            deadlines.insert(
                id,
                Instant::now() + Duration::from_millis(milliseconds.max(0) as u64),
            )
        });
        post(0); // Rearm the native one-shot deadline without requesting a frame.
    }
    #[func]
    fn cancel(&self, id: i64) {
        DEADLINES.with_borrow_mut(|deadlines| deadlines.remove(&id));
    }
    #[func]
    fn next_delay(&self) -> i64 {
        DEADLINES.with_borrow(|deadlines| {
            deadlines.values().min().map_or(-1, |deadline| {
                i64::try_from(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis(),
                )
                .unwrap_or(i64::MAX)
                .max(1)
            })
        })
    }
    #[func]
    fn dispatch(&self) {
        let mut ready = WAKE.take();
        DEADLINES.with_borrow_mut(|deadlines| {
            let now = Instant::now();
            deadlines.retain(|id, deadline| {
                if *deadline <= now {
                    ready.insert(*id);
                    false
                } else {
                    true
                }
            });
        });
        for id in ready {
            let callback = CALLBACKS.with_borrow(|callbacks| callbacks.get(&id).cloned());
            if let Some(callback) = callback {
                if callback.is_valid() {
                    callback.call(&[]);
                } else {
                    unsubscribe(id);
                }
            }
        }
    }
}


