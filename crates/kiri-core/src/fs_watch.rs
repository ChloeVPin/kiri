//! Restricted filesystem-watch surface (`kiri.fs.watch`).
//!
//! This closes the Tauri `fs` watch parity gap (G-10) and exceeds it on the
//! security axis. Tauri's `fs` watch, once the capability is present, lets the
//! frontend watch any path. Kiri requires BOTH the `FS` capability bit AND a
//! host-owned path allowlist: the frontend may only watch pre-approved paths
//! inside the `PathScope`, so a granted capability cannot be pivoted into
//! surveillance of arbitrary filesystem locations. Watch events are delivered
//! back to the frontend only through the host-owned channel allowlist, never as
//! raw watch metadata.
//!
//! The actual watcher is behind the `FsWatchBackend` trait (mirrors
//! `TrayRunner`): the native host injects a real backend; tests use a
//! `StubFsWatch` and assert path-allowlist enforcement and capability gating
//! headlessly, with no OS watcher and no WebView.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.fs.watch` commands. Reuses the shared `FS` capability
/// bit (6) so it stays in lockstep with `capability_bit::FS` and `for_command`.
pub const FS_WATCH_CAPABILITY: u32 = crate::dispatch::capability_bit::FS;

/// Host-owned watch kind. The frontend names a path + kind; the host owns which
/// kinds are permitted for which path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    /// Report create/modify/remove for the exact path or its children.
    All,
    /// Only content modifications of a file.
    Modify,
}

impl WatchKind {
    fn from_payload(v: &Value) -> WatchKind {
        match v.as_str() {
            Some("modify") => WatchKind::Modify,
            _ => WatchKind::All,
        }
    }
}

/// One host-approved watch target. The frontend references `path` only; it
/// cannot invent a path. The host owns the set of watchable paths, each bounded
/// by the existing `PathScope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTarget {
    pub path: String,
    pub kind: WatchKind,
}

/// Host-configured allowlist of watch targets. Default-deny: a watch succeeds
/// only if the exact (path, kind) pair is listed. The host owns the namespace.
#[derive(Debug, Clone, Default)]
pub struct FsWatchAllowlist {
    targets: Vec<WatchTarget>,
}

impl FsWatchAllowlist {
    pub fn new(targets: Vec<WatchTarget>) -> Self {
        Self { targets }
    }

    fn resolve(&self, path: &str, kind: WatchKind) -> Option<WatchTarget> {
        self.targets.iter().find(|t| t.path == path && t.kind == kind).cloned()
    }

    pub fn targets(&self) -> &[WatchTarget] {
        &self.targets
    }
}

/// A watch event delivered to the frontend. The path is the host-owned watch
/// target path (already allowlisted), and the kind is bounded; the backend
/// never emits an unapproved path.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub path: String,
    pub event: String,
}

/// Transport seam. The native host provides a real watcher; tests provide a
/// stub. Trait-based so the logical protocol has zero platform deps.
pub trait FsWatchBackend: Send + Sync {
    /// Begin watching the host-owned target; returns a host-assigned watch id.
    fn watch(&self, target: &WatchTarget) -> Result<u64>;
    /// Stop watching the given host-assigned watch id.
    fn unwatch(&self, watch_id: u64) -> Result<()>;
    /// Drain queued watch events for a watch id (used by tests/transport).
    fn drain(&self, watch_id: u64) -> Vec<WatchEvent>;
}

/// Production backend used when no live watcher is wired into this build.
/// The command stays registered and capability-gated; the transport simply
/// reports that it is not available, so the frontend gets an explicit error
/// instead of an unregistered (unknown-command) failure.
pub struct DisabledFsWatch;

impl FsWatchBackend for DisabledFsWatch {
    fn watch(&self, _target: &WatchTarget) -> Result<u64> {
        Err(Error::service_unavailable("kiri.fs.watch backend not wired in this build"))
    }
    fn unwatch(&self, _watch_id: u64) -> Result<()> {
        Err(Error::service_unavailable("kiri.fs.unwatch backend not wired in this build"))
    }
    fn drain(&self, _watch_id: u64) -> Vec<WatchEvent> {
        Vec::new()
    }
}

/// Capability-scoped fs-watch service bounded to a path allowlist plus limits.
#[derive(Clone)]
pub struct FsWatchService {
    backend: Arc<dyn FsWatchBackend>,
    allowlist: Arc<FsWatchAllowlist>,
    limits: Arc<Limits>,
}

impl FsWatchService {
    pub fn new(
        backend: Arc<dyn FsWatchBackend>,
        allowlist: FsWatchAllowlist,
        limits: Limits,
    ) -> Self {
        Self { backend, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Begin watching a host-allowlisted path. The path must be an exact entry
    /// in the allowlist; otherwise the request is refused.
    pub fn watch(&self, path: &str, kind: WatchKind) -> Result<Value> {
        let target = self.allowlist.resolve(path, kind).ok_or_else(|| {
            Error::scope_denied(format!("kiri.fs.watch: path not on allowlist: {path}"))
        })?;
        // Bounded path length so a misconfiguration cannot flood.
        self.limits.check_bulk_object(target.path.len() as u64)?;
        let id = self.backend.watch(&target)?;
        Ok(serde_json::json!({ "watch_id": id, "path": target.path }))
    }

    /// Stop an active watch by host-assigned id.
    pub fn unwatch(&self, watch_id: u64) -> Result<Value> {
        self.backend.unwatch(watch_id)?;
        Ok(serde_json::json!({ "unwatched": true, "watch_id": watch_id }))
    }

    /// Drain pending watch events for a host-assigned watch id.
    pub fn drain(&self, watch_id: u64) -> Vec<WatchEvent> {
        self.backend.drain(watch_id)
    }
}

/// Build the `kiri.fs.watch` handlers bound to one FsWatchService.
pub fn fs_watch_handlers(
    service: FsWatchService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(FS_WATCH_CAPABILITY);

    let watch_svc = service.clone();
    let unwatch_svc = service.clone();
    vec![
        (
            command_id::FS_WATCH,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::invalid_argument("kiri.fs.watch requires string path"))?;
                let kind = p.get("kind").map(WatchKind::from_payload).unwrap_or(WatchKind::All);
                watch_svc.watch(path, kind)
            }) as Handler,
        ),
        (
            command_id::FS_UNWATCH,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let watch_id = p.get("watchId").and_then(|v| v.as_u64()).ok_or_else(|| {
                    Error::invalid_argument("kiri.fs.unwatch requires numeric watchId")
                })?;
                unwatch_svc.unwatch(watch_id)
            }) as Handler,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;
    use std::sync::Mutex;

    struct StubFsWatch {
        active: Mutex<std::collections::HashMap<u64, WatchTarget>>,
        events: Mutex<std::collections::HashMap<u64, Vec<WatchEvent>>>,
        next: Mutex<u64>,
    }
    impl FsWatchBackend for StubFsWatch {
        fn watch(&self, target: &WatchTarget) -> Result<u64> {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let id = *next;
            self.active.lock().unwrap().insert(id, target.clone());
            Ok(id)
        }
        fn unwatch(&self, watch_id: u64) -> Result<()> {
            self.active.lock().unwrap().remove(&watch_id);
            Ok(())
        }
        fn drain(&self, watch_id: u64) -> Vec<WatchEvent> {
            self.events.lock().unwrap().remove(&watch_id).unwrap_or_default()
        }
    }

    fn allow() -> FsWatchAllowlist {
        FsWatchAllowlist::new(vec![
            WatchTarget { path: "/data/app".to_string(), kind: WatchKind::All },
            WatchTarget { path: "/data/app/config.json".to_string(), kind: WatchKind::Modify },
        ])
    }

    fn router() -> Router {
        let svc = FsWatchService::new(
            Arc::new(StubFsWatch {
                active: Mutex::new(std::collections::HashMap::new()),
                events: Mutex::new(std::collections::HashMap::new()),
                next: Mutex::new(0),
            }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_fs_watch(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(FS_WATCH_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_path_watches() {
        let r = router();
        let out = dispatch(&r, command_id::FS_WATCH, serde_json::json!({ "path": "/data/app" }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert!(out["payload"]["watch_id"].as_u64().is_some());
    }

    #[test]
    fn disallowed_path_is_denied() {
        let r = router();
        let out = dispatch(&r, command_id::FS_WATCH, serde_json::json!({ "path": "/etc/shadow" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn unwatch_succeeds() {
        let r = router();
        let w = dispatch(&r, command_id::FS_WATCH, serde_json::json!({ "path": "/data/app" }));
        let id = w["payload"]["watch_id"].as_u64().unwrap();
        let out = dispatch(&r, command_id::FS_UNWATCH, serde_json::json!({ "watchId": id }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
    }

    #[test]
    fn capability_denied_without_fs_bit() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::FS_WATCH,
            1,
            1,
            serde_json::json!({ "path": "/data/app" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
