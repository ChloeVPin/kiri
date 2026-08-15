//! Restricted window-state persistence surface (`kiri.window.state`).
//!
//! This closes the Tauri `window-state` plugin parity gap and converts Tauri's
//! persistence risk into a Kiri strength. Tauri's `window-state` plugin
//! auto-persists window geometry (position/size/maximized) to a JSON file that the
//! frontend can read and write, and applies it on startup without a second
//! capability gate. That is both a tamper surface (a malicious or buggy frontend
//! can force off-screen/zero-size windows, or forge geometry) and a privacy smell
//! (frontend-readable layout history).
//!
//! Kiri requires the `WINDOW_STATE` capability bit for BOTH save and load, and the
//! persistence backend is host-owned: the core writes geometry into a fixed,
//! frontend-unaddressable store namespace (`window.state`) behind the same
//! `StoreBackend` seam used by `kiri.store.*`. The frontend can only request
//! save/load of the current window's own geometry; it cannot choose the namespace,
//! the key, or another window's state, and it can never read the raw persisted blob.
//! A granted capability with no host backend is refused. The runner only ever
//! receives a host-owned, capability-scoped geometry record.

use std::sync::Arc;

use serde_json::Value;

use crate::error::Result;
use crate::limits::Limits;
use crate::store::StoreBackend;

/// Authorizes the `kiri.window.state.*` commands.
pub const WINDOW_STATE_CAPABILITY: u32 = 19;

/// Fixed, frontend-unaddressable namespace for window-state persistence. The
/// `kiri.store.*` surface uses an allowlist of host-owned namespaces; window-state
/// uses its own reserved namespace so the frontend can never reach it via the
/// generic store command.
pub const WINDOW_STATE_NAMESPACE: &str = "window.state";

/// The single key under the reserved namespace. Only one window's geometry is
/// persisted in this build; multi-window state would extend the key scheme here
/// without changing the authority model.
pub const WINDOW_STATE_KEY: &str = "main";

/// Persisted window geometry. Mirrors the observable geometry fields of
/// `WindowState`; kept separate so persistence is decoupled from live window ops.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Geometry {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "x": self.x,
            "y": self.y,
            "width": self.width,
            "height": self.height,
            "maximized": self.maximized,
        })
    }

    fn from_json(v: &Value) -> Result<Self> {
        Ok(Geometry {
            x: v.get("x").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            y: v.get("y").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            width: v.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            height: v.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            maximized: v.get("maximized").and_then(|x| x.as_bool()).unwrap_or(false),
        })
    }
}

/// Transport seam for persisting geometry. The native host injects a real backend
/// (reusing `StoreBackend`); tests provide a stub. Kept trait-based so the logical
/// protocol has zero platform deps.
pub trait WindowStateBackend: Send + Sync {
    /// Persist `geometry` for the main window under the reserved namespace/key.
    fn save(&self, geometry: &Geometry) -> Result<()>;
    /// Load the persisted geometry, or `None` if absent.
    fn load(&self) -> Result<Option<Geometry>>;
}

/// Host-owned `StoreBackend`-backed persistence. The core has already enforced the
/// `WINDOW_STATE` capability before this runs, and the namespace is fixed, so the
/// frontend can never address an arbitrary store location.
pub struct StoreWindowStateBackend {
    backend: Arc<dyn StoreBackend>,
}

impl StoreWindowStateBackend {
    pub fn new(backend: Arc<dyn StoreBackend>) -> Self {
        Self { backend }
    }
}

impl WindowStateBackend for StoreWindowStateBackend {
    fn save(&self, geometry: &Geometry) -> Result<()> {
        self.backend.set(WINDOW_STATE_NAMESPACE, WINDOW_STATE_KEY, geometry.to_json())
    }

    fn load(&self) -> Result<Option<Geometry>> {
        match self.backend.get(WINDOW_STATE_NAMESPACE, WINDOW_STATE_KEY)? {
            Some(v) => Ok(Some(Geometry::from_json(&v)?)),
            None => Ok(None),
        }
    }
}

/// Capability-scoped window-state service bounded to a host-owned backend plus
/// limits.
#[derive(Clone)]
pub struct WindowStateService {
    backend: Arc<dyn WindowStateBackend>,
    limits: Arc<Limits>,
}

impl WindowStateService {
    pub fn new(backend: Arc<dyn WindowStateBackend>, limits: Limits) -> Self {
        Self { backend, limits: Arc::new(limits) }
    }

    /// Save the supplied geometry. Returns the saved geometry for audit/trace.
    pub fn save(&self, geometry: Geometry) -> Result<Value> {
        self.limits.check_bulk_object(256)?;
        self.backend.save(&geometry)?;
        Ok(geometry.to_json())
    }

    /// Load persisted geometry. Returns `null` when nothing is saved (caller must
    /// fall back to defaults).
    pub fn load(&self) -> Result<Value> {
        self.limits.check_bulk_object(256)?;
        match self.backend.load()? {
            Some(g) => Ok(g.to_json()),
            None => Ok(Value::Null),
        }
    }
}

/// Build the `kiri.window.state.*` handlers bound to one WindowStateService.
pub fn window_state_handlers(
    service: WindowStateService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(WINDOW_STATE_CAPABILITY);

    let save_svc = service.clone();
    let load_svc = service.clone();
    vec![
        (
            command_id::WINDOW_STATE_SAVE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let geometry = Geometry {
                    x: p.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    y: p.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    width: p.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    height: p.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    maximized: p.get("maximized").and_then(|v| v.as_bool()).unwrap_or(false),
                };
                save_svc.save(geometry)
            }) as Handler,
        ),
        (
            command_id::WINDOW_STATE_LOAD,
            required,
            Arc::new(move |_c, _rid, _p: &Value| load_svc.load()) as Handler,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::store::StoreBackend;
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;
    use std::sync::Mutex;

    struct StubBackend {
        data: Mutex<Option<Value>>,
    }
    impl StoreBackend for StubBackend {
        fn get(&self, _ns: &str, _key: &str) -> Result<Option<Value>> {
            Ok(self.data.lock().unwrap().clone())
        }
        fn set(&self, _ns: &str, _key: &str, value: Value) -> Result<()> {
            *self.data.lock().unwrap() = Some(value);
            Ok(())
        }
    }

    fn router() -> Router {
        let backend = Arc::new(StubBackend { data: Mutex::new(None) });
        let svc = WindowStateService::new(
            Arc::new(StoreWindowStateBackend::new(backend)),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_window_state(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(WINDOW_STATE_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn save_then_load_roundtrip() {
        let r = router();
        let geo = serde_json::json!({ "x": 10, "y": 20, "width": 800, "height": 600, "maximized": false });
        let out = dispatch(&r, command_id::WINDOW_STATE_SAVE, geo.clone());
        assert!(out["error"].is_null(), "save error: {out}");
        let loaded = dispatch(&r, command_id::WINDOW_STATE_LOAD, serde_json::json!({}));
        assert!(loaded["error"].is_null(), "load error: {loaded}");
        assert_eq!(loaded["payload"]["x"], 10);
        assert_eq!(loaded["payload"]["width"], 800);
    }

    #[test]
    fn load_without_save_returns_null() {
        let r = router();
        let out = dispatch(&r, command_id::WINDOW_STATE_LOAD, serde_json::json!({}));
        assert!(out["error"].is_null());
        assert!(out["payload"].is_null());
    }

    #[test]
    fn save_denied_without_capability() {
        let r = router();
        let granted = CapabilityBits::empty(); // no WINDOW_STATE bit
        let req = WireRequest::new(
            command_id::WINDOW_STATE_SAVE,
            1,
            1,
            serde_json::json!({ "x": 1, "y": 1, "width": 1, "height": 1, "maximized": false }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
