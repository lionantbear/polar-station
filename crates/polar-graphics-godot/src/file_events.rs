//! OS notifications for local preview sources. No document I/O or Godot objects
//! on the notify worker. The engine thread dispatches pending events as signals.
use godot::classes::{INode, Node};
use godot::prelude::*;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Pending {
    changed: AtomicBool,
    error: Mutex<Option<String>>,
}

#[derive(GodotClass)]
#[class(base=Node)]
pub struct PolarFileEvents {
    base: Base<Node>,
    watcher: Option<RecommendedWatcher>,
    pending: Arc<Pending>,
}

#[godot_api]
impl INode for PolarFileEvents {
    fn init(base: Base<Node>) -> Self {
        Self {
            base,
            watcher: None,
            pending: Arc::default(),
        }
    }
    fn process(&mut self, _delta: f64) {
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
    fn exit_tree(&mut self) {
        self.watcher = None;
    }
}

// Canonicalize the directory, keeping the filename as a replaceable entry.
// FSEvents returns physical paths even when a native chooser used an alias.
fn source_entry(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| std::io::Error::other("Missing source directory"))?;
    Ok(parent.canonicalize()?.join(path.file_name().ok_or_else(|| std::io::Error::other("Missing source filename"))?))
}

fn affects(event: &Event, paths: &HashSet<PathBuf>) -> bool {
    !event.kind.is_access()
        && (event.need_rescan() || event.paths.iter().any(|p| paths.contains(p)))
}

#[godot_api]
impl PolarFileEvents {
    #[signal]
    fn files_changed();
    #[signal]
    fn watch_failed(message: GString);

    #[func]
    fn watch_files(&mut self, files: PackedStringArray) -> GString {
        self.watcher = None;
        self.pending = Arc::default();
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
            parents.insert(entry.parent().expect("source entry has a directory").to_path_buf());
            paths.insert(path);
            paths.insert(entry);
        }
        if paths.is_empty() {
            return GString::new();
        }
        let pending = Arc::clone(&self.pending);
        let watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) if affects(&event, &paths) => {
                    pending.changed.store(true, Ordering::Release);
                }
                Err(error) => {
                    *pending.error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(error.to_string());
                }
                _ => {}
            });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => return GString::from(error.to_string().as_str()),
        };
        for parent in parents {
            if let Err(error) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
                return GString::from(error.to_string().as_str());
            }
        }
        self.watcher = Some(watcher);
        // Close the load/subscribe race with one initial refresh, not a timer.
        self.pending.changed.store(true, Ordering::Release);
        GString::new()
    }
}


