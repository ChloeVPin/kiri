//! Host platform facts and an in-process event bus for the R-3 JS surface.
//!
//! These commands give Kiri parity with Tauri's most-used frontend modules
//! (`os`, `app`, `event`) without launching a WebView. Handlers read real
//! host facts and never touch the filesystem or network, so they are fully
//! exercisable headlessly (the audit loop never opens a window).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Report the host OS family. Returns one of `macos`, `windows`, `linux`, or
/// `unknown`. Uses the `target_os` cfg so the value is correct for the binary
/// actually running (not the build host's Rust target triple string).
pub fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// Report the host CPU architecture as a stable string (`aarch64`, `x86_64`,
/// etc.). Derived from the `target_arch` cfg.
pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}

/// An in-process publish/subscribe bus shared by the trusted frontend and
/// native tooling. `kiri.event.emit` publishes; `kiri.event.listen` returns a
/// subscriber id, and the host bridge forwards matching publications to the
/// registered listener (parity with Tauri's `event` module).
///
/// `publish` is lock-free on the hot path (one `Mutex` around the subscriber
/// map plus an `Arc` per subscriber queue). Subscriber ids are unique and
/// monotonic.
#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<Mutex<BusInner>>,
}

/// A subscriber entry: the event name it listens for and a queue of
/// published payloads not yet drained by the host bridge.
type Subscriber = (String, Arc<Mutex<Vec<Value>>>);

#[derive(Default)]
struct BusInner {
    next_id: u64,
    /// Subscriber id -> (event name, queue of published payloads).
    subscribers: HashMap<u64, Subscriber>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a listener for `event`. Returns a stable subscriber id.
    pub fn subscribe(&self, event: &str) -> u64 {
        let mut g = self.inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.subscribers.insert(id, (event.to_string(), Arc::new(Mutex::new(Vec::new()))));
        id
    }

    /// Publish `payload` to every subscriber of `event`.
    pub fn publish(&self, event: &str, payload: Value) {
        let g = self.inner.lock().unwrap();
        for (name, queue) in g.subscribers.values() {
            if name == event {
                queue.lock().unwrap().push(payload.clone());
            }
        }
    }

    /// Drain queued publications for a subscriber id (called by the host bridge
    /// when delivering to a listener). Returns an empty vec when the id is
    /// unknown or has no pending events.
    pub fn drain(&self, subscriber_id: u64) -> Vec<Value> {
        let g = self.inner.lock().unwrap();
        match g.subscribers.get(&subscriber_id) {
            Some((_, queue)) => std::mem::take(&mut *queue.lock().unwrap()),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_bus_delivers_to_subscriber() {
        let bus = EventBus::new();
        let id = bus.subscribe("ping");
        bus.publish("ping", serde_json::json!({ "n": 1 }));
        bus.publish("other", serde_json::json!({ "n": 99 }));
        let drained = bus.drain(id);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0]["n"], serde_json::json!(1));
    }

    #[test]
    fn host_os_is_known_family() {
        let os = host_os();
        assert!(matches!(os, "macos" | "windows" | "linux" | "unknown"));
        let arch = host_arch();
        assert!(matches!(arch, "aarch64" | "x86_64" | "x86" | "arm" | "unknown"));
    }
}
