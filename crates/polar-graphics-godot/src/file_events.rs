//! OS notifications for local preview sources. No document I/O or Godot objects
//! on the notify worker. The engine thread dispatches pending events as signals.
use godot::classes::{INode, Node};
use godot::prelude::*;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

#[derive(Default)]
struct Pending {
    changed: AtomicBool,
    error: Mutex<Option<String>>,
}

#[derive(GodotClass)]
#[class(base=Node)]
pub struct PolarFileEvents {
    base: Base<Node>,
    pending: Arc<Pending>,
    subscription: i64,
}

#[godot_api]
impl INode for PolarFileEvents {
    fn init(base: Base<Node>) -> Self {
        Self {
            base,
            pending: Arc::default(),
            subscription: 0,
        }
    }
    fn exit_tree(&mut self) {
        unregister(self.subscription);
        crate::events::unsubscribe(self.subscription);
    }
}

// Canonicalize the directory, keeping the filename as a replaceable entry.
// FSEvents returns physical paths even when a native chooser used an alias.
fn source_entry(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing source directory"))?;
    Ok(parent.canonicalize()?.join(
        path.file_name()
            .ok_or_else(|| std::io::Error::other("Missing source filename"))?,
    ))
}

fn affects(event: &Event, paths: &HashSet<PathBuf>) -> bool {
    !event.kind.is_access()
        && (event.need_rescan() || event.paths.iter().any(|p| paths.contains(p)))
}

#[godot_api]
impl PolarFileEvents {
    #[func]
    fn dispatch_pending(&mut self) {
        // Dispatch only OS notifications; never stat/read files on frame ticks.
        let error = self
            .pending
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(error) = error {
            self.base_mut()
                .emit_signal("watch_failed", &[error.to_variant()]);
        }
        if self.pending.changed.swap(false, Ordering::AcqRel) {
            self.base_mut().emit_signal("files_changed", &[]);
        }
    }

    #[signal]
    fn files_changed();
    #[signal]
    fn watch_failed(message: GString);

    #[func]
    fn source_revision(&self, path: GString) -> i64 {
        revision(&path.to_string())
    }
    #[func]
    fn watch_files(&mut self, files: PackedStringArray) -> GString {
        unregister(self.subscription);
        self.pending = Arc::default();
        crate::events::unsubscribe(self.subscription);
        self.subscription = crate::events::subscribe(self.base().callable("dispatch_pending"));
        let mut paths = HashSet::new();
        let mut parents = HashSet::new();
        for name in files.as_slice() {
            let path = PathBuf::from(name.to_string());
            if !path.is_absolute() {
                return "File watches require absolute paths".into();
            }
            let Some(_parent) = path.parent() else {
                return "Missing source directory".into();
            };
            // Watch the directory entry, not its inode: editor rename-over saves
            // and later recreation must continue notifying the same view.
            let entry = match source_entry(&path) {
                Ok(entry) => entry,
                Err(error) => return GString::from(error.to_string().as_str()),
            };
            parents.insert(
                entry
                    .parent()
                    .expect("source entry has a directory")
                    .to_path_buf(),
            );
            paths.insert(path);
            paths.insert(entry);
        }
        if paths.is_empty() {
            return GString::new();
        }
        if let Err(error) = register(
            self.subscription,
            paths,
            parents,
            Arc::downgrade(&self.pending),
        ) {
            return error.to_string().as_str().into();
        }
        // Close the load/subscribe race with one initial refresh, not a timer.
        self.pending.changed.store(true, Ordering::Release);
        crate::events::post(self.subscription);
        GString::new()
    }
}

struct Subscription {
    paths: HashSet<PathBuf>,
    parents: HashSet<PathBuf>,
    pending: Weak<Pending>,
}
#[derive(Default)]
struct Hub {
    subscriptions: HashMap<i64, Subscription>,
    parents: HashMap<PathBuf, usize>,
    revisions: HashMap<PathBuf, i64>,
}
static HUB: LazyLock<Mutex<Hub>> = LazyLock::new(|| Mutex::new(Hub::default()));
static WATCHER: LazyLock<Mutex<Option<RecommendedWatcher>>> = LazyLock::new(|| Mutex::new(None));
static REVISION: AtomicI64 = AtomicI64::new(1);
fn register(
    id: i64,
    paths: HashSet<PathBuf>,
    parents: HashSet<PathBuf>,
    pending: Weak<Pending>,
) -> notify::Result<()> {
    let mut watcher = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    if watcher.is_none() {
        *watcher = Some(notify::recommended_watcher(
            |result: notify::Result<Event>| {
                let mut hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(event) = &result {
                    if event.kind.is_access() {
                        return;
                    }
                    let revision = REVISION.fetch_add(1, Ordering::Relaxed);
                    for (path, current) in &mut hub.revisions {
                        if event.need_rescan() || event.paths.contains(path) {
                            *current = revision;
                        }
                    }
                }
                if result.is_err() {
                    let revision = REVISION.fetch_add(1, Ordering::Relaxed);
                    for current in hub.revisions.values_mut() {
                        *current = revision;
                    }
                }
                for (id, subscription) in &hub.subscriptions {
                    let Some(pending) = subscription.pending.upgrade() else {
                        continue;
                    };
                    match &result {
                        Ok(event) if affects(event, &subscription.paths) => {
                            pending.changed.store(true, Ordering::Release);
                        }
                        Err(error) => {
                            *pending.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error.to_string());
                        }
                        _ => continue,
                    }
                    crate::events::post(*id);
                }
            },
        )?);
    }
    let new_parents: Vec<_> = {
        let hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
        parents
            .iter()
            .filter(|path| !hub.parents.contains_key(*path))
            .cloned()
            .collect()
    };
    // Backend operations never hold the lock used by notification callbacks.
    let mut added: Vec<PathBuf> = Vec::new();
    for parent in &new_parents {
        {
            if let Err(error) = watcher
                .as_mut()
                .expect("watcher exists")
                .watch(parent, RecursiveMode::NonRecursive)
            {
                for path in added {
                    let _ = watcher.as_mut().expect("watcher exists").unwatch(&path);
                }
                return Err(error);
            }
            added.push(parent.clone());
        }
    }
    let mut hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
    for parent in &parents {
        *hub.parents.entry(parent.clone()).or_default() += 1;
    }
    for path in &paths {
        hub.revisions
            .entry(path.clone())
            .or_insert_with(|| REVISION.fetch_add(1, Ordering::Relaxed));
    }
    hub.subscriptions.insert(
        id,
        Subscription {
            paths,
            parents,
            pending,
        },
    );
    Ok(())
}
fn unregister(id: i64) {
    let mut watcher = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    let mut removed = Vec::new();
    let mut hub = HUB.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(subscription) = hub.subscriptions.remove(&id) {
        for parent in subscription.parents {
            if let Some(count) = hub.parents.get_mut(&parent) {
                *count -= 1;
                if *count == 0 {
                    hub.parents.remove(&parent);
                    removed.push(parent);
                }
            }
        }
        let retained: HashSet<PathBuf> = hub
            .subscriptions
            .values()
            .flat_map(|s| s.paths.iter().cloned())
            .collect();
        hub.revisions.retain(|path, _| retained.contains(path));
    }
    drop(hub);
    if let Some(watcher) = watcher.as_mut() {
        for parent in removed {
            let _ = watcher.unwatch(&parent);
        }
    }
}
pub fn revision(path: &str) -> i64 {
    let Ok(path) = source_entry(std::path::Path::new(path)) else {
        return -1;
    };
    HUB.lock()
        .unwrap_or_else(|e| e.into_inner())
        .revisions
        .get(&path)
        .copied()
        .unwrap_or(-1)
}


