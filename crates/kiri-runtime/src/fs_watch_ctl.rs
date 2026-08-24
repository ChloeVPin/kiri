//! Native filesystem-watch transport for the capability-scoped watch service.
//!
//! The logical service owns capability and path authorization. This module
//! only turns an already-authorized target into a bounded host-assigned watch
//! and queues coarse event names. Raw paths from the operating system are
//! intentionally discarded so a watcher cannot disclose paths outside the
//! host-owned target.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use kiri_core::error::{Error, Result};
use kiri_core::fs_watch::{FsWatchBackend, WatchEvent, WatchKind, WatchTarget};

struct ActiveWatch {
    _watcher: RecommendedWatcher,
    events: Receiver<WatchEvent>,
}

pub struct NativeFsWatchBackend {
    next_id: AtomicU64,
    active: Mutex<HashMap<u64, ActiveWatch>>,
}

impl NativeFsWatchBackend {
    pub fn new() -> Self {
        Self { next_id: AtomicU64::new(1), active: Mutex::new(HashMap::new()) }
    }

    fn event_name(kind: &EventKind) -> &'static str {
        match kind {
            EventKind::Create(_) => "create",
            EventKind::Modify(_) => "modify",
            EventKind::Remove(_) => "remove",
            EventKind::Access(_) => "access",
            EventKind::Other => "other",
            EventKind::Any => "any",
        }
    }
}

impl Default for NativeFsWatchBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FsWatchBackend for NativeFsWatchBackend {
    fn watch(&self, target: &WatchTarget) -> Result<u64> {
        let (sender, receiver): (Sender<WatchEvent>, Receiver<WatchEvent>) = mpsc::channel();
        let event_path = target.path.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                if let Ok(event) = result {
                    let _ = sender.send(WatchEvent {
                        path: event_path.clone(),
                        event: Self::event_name(&event.kind).to_string(),
                    });
                }
            },
            Config::default(),
        )
        .map_err(|e| Error::service_unavailable(format!("kiri.fs.watch init failed: {e}")))?;
        let mode = match target.kind {
            WatchKind::All => RecursiveMode::Recursive,
            WatchKind::Modify => RecursiveMode::NonRecursive,
        };
        watcher
            .watch(Path::new(&target.path), mode)
            .map_err(|e| Error::service_unavailable(format!("kiri.fs.watch failed: {e}")))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.active
            .lock()
            .map_err(|_| Error::command_error("kiri.fs.watch state poisoned"))?
            .insert(id, ActiveWatch { _watcher: watcher, events: receiver });
        Ok(id)
    }

    fn unwatch(&self, watch_id: u64) -> Result<()> {
        let removed = self
            .active
            .lock()
            .map_err(|_| Error::command_error("kiri.fs.unwatch state poisoned"))?
            .remove(&watch_id);
        if removed.is_some() {
            Ok(())
        } else {
            Err(Error::resource_not_found(format!("kiri.fs.unwatch unknown watch id {watch_id}")))
        }
    }

    fn drain(&self, watch_id: u64) -> Vec<WatchEvent> {
        let Ok(active) = self.active.lock() else {
            return Vec::new();
        };
        let Some(watch) = active.get(&watch_id) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(event) = watch.events.try_recv() {
            events.push(event);
            if events.len() >= 256 {
                break;
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_unregisters_a_native_watch() {
        let root = std::env::temp_dir().join(format!("kiri-watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create watch root");
        let backend = NativeFsWatchBackend::new();
        let target =
            WatchTarget { path: root.to_string_lossy().into_owned(), kind: WatchKind::All };
        let id = backend.watch(&target).expect("native watch registration");
        assert!(backend.drain(id).is_empty());
        backend.unwatch(id).expect("native watch removal");
        assert!(backend.drain(id).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
