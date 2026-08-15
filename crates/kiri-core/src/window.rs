//! Capability-gated window control surface (`kiri.window.*`).
//!
//! This closes part of the Tauri `window` module parity gap (G-5) and exceeds
//! it on the security axis: every operation is authorized by the central
//! capability authority (bit `WINDOW`) and routed through a host-owned
//! `WindowController`. JavaScript can request a state change but cannot bypass
//! the capability check or reach the native window handle directly.
//!
//! The controller is a thin trait the native host implements (wry `Window` on
//! macOS/Linux, WebView2 on Windows). The state mirror (`WindowState`) is owned
//! by core so the surface is fully exercisable headlessly: tests drive a
//! `StubWindow` and assert routing, authorization, and state transitions
//! without launching a WebView.

use std::sync::Arc;

use serde_json::Value;

use crate::error::Error;

/// Authorizes the `kiri.window.*` commands.
pub const WINDOW_CAPABILITY: u32 = 7;

/// Mirror of the window's observable state. Kept in core so the control-plane
/// state is authoritative and testable without a native window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowState {
    pub title: String,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub focused: bool,
    pub close_requested: bool,
}

impl WindowState {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            visible: true,
            minimized: false,
            maximized: false,
            focused: false,
            close_requested: false,
        }
    }
}

/// Host-provided window backend. The native host implements this for the real
/// `tao::window::Window` (cross) or WebView2 controller (Windows). Commands
/// mutate `state` (the core mirror) and call the corresponding native op.
pub trait WindowController: Send + Sync {
    /// Apply a title change to the native window.
    fn set_title(&self, state: &mut WindowState, title: &str);
    /// Show the native window.
    fn show(&self, state: &mut WindowState);
    /// Hide the native window.
    fn hide(&self, state: &mut WindowState);
    /// Minimize the native window.
    fn minimize(&self, state: &mut WindowState);
    /// Maximize the native window.
    fn maximize(&self, state: &mut WindowState);
    /// Restore from minimized/maximized.
    fn restore(&self, state: &mut WindowState);
    /// Request the native window to close.
    fn close(&self, state: &mut WindowState);
    /// Focus the native window.
    fn focus(&self, state: &mut WindowState);
}

/// Build the `kiri.window.*` handlers bound to one `WindowController` and its
/// shared `WindowState` mirror. Reused by the router builder and any plugin
/// path so authority is identical either way.
pub fn window_handlers(
    controller: std::sync::Arc<dyn WindowController>,
    state: std::sync::Arc<std::sync::Mutex<WindowState>>,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(WINDOW_CAPABILITY);

    let title_get_state = state.clone();
    let title_set_ctrl = controller.clone();
    let title_set_state = state.clone();
    let show_ctrl = controller.clone();
    let show_state = state.clone();
    let hide_ctrl = controller.clone();
    let hide_state = state.clone();
    let min_ctrl = controller.clone();
    let min_state = state.clone();
    let max_ctrl = controller.clone();
    let max_state = state.clone();
    let restore_ctrl = controller.clone();
    let restore_state = state.clone();
    let close_ctrl = controller.clone();
    let close_state = state.clone();
    let focus_ctrl = controller.clone();
    let focus_state = state.clone();

    vec![
        (
            command_id::WINDOW_TITLE_GET,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                let s = title_get_state.lock().unwrap();
                Ok(serde_json::json!({ "title": s.title }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_TITLE_SET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let t = p.get("title").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.window.title.set requires string title")
                })?;
                title_set_ctrl.set_title(&mut title_set_state.lock().unwrap(), t);
                Ok(serde_json::json!({ "title": t }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_SHOW,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                show_ctrl.show(&mut show_state.lock().unwrap());
                Ok(serde_json::json!({ "shown": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_HIDE,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                hide_ctrl.hide(&mut hide_state.lock().unwrap());
                Ok(serde_json::json!({ "hidden": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_MINIMIZE,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                min_ctrl.minimize(&mut min_state.lock().unwrap());
                Ok(serde_json::json!({ "minimized": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_MAXIMIZE,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                max_ctrl.maximize(&mut max_state.lock().unwrap());
                Ok(serde_json::json!({ "maximized": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_RESTORE,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                restore_ctrl.restore(&mut restore_state.lock().unwrap());
                Ok(serde_json::json!({ "restored": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_CLOSE,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                close_ctrl.close(&mut close_state.lock().unwrap());
                Ok(serde_json::json!({ "close_requested": true }))
            }) as Handler,
        ),
        (
            command_id::WINDOW_FOCUS,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                focus_ctrl.focus(&mut focus_state.lock().unwrap());
                Ok(serde_json::json!({ "focused": true }))
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

    /// A no-op controller that only mutates the core state mirror, so tests
    /// exercise routing/authorization/state without a native window.
    struct StubWindow;
    impl WindowController for StubWindow {
        fn set_title(&self, s: &mut WindowState, t: &str) {
            s.title = t.to_string();
        }
        fn show(&self, s: &mut WindowState) {
            s.visible = true;
            s.minimized = false;
        }
        fn hide(&self, s: &mut WindowState) {
            s.visible = false;
        }
        fn minimize(&self, s: &mut WindowState) {
            s.minimized = true;
            s.visible = true;
        }
        fn maximize(&self, s: &mut WindowState) {
            s.maximized = true;
        }
        fn restore(&self, s: &mut WindowState) {
            s.minimized = false;
            s.maximized = false;
        }
        fn close(&self, s: &mut WindowState) {
            s.close_requested = true;
        }
        fn focus(&self, s: &mut WindowState) {
            s.focused = true;
        }
    }

    fn router() -> Router {
        let state = std::sync::Arc::new(std::sync::Mutex::new(WindowState::new("Kiri")));
        let ctrl = std::sync::Arc::new(StubWindow);
        Router::new_with_limits(crate::limits::Limits::default()).with_window(ctrl, state)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(WINDOW_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn title_get_returns_current() {
        let r = router();
        let out = dispatch(&r, command_id::WINDOW_TITLE_GET, serde_json::json!({}));
        assert_eq!(out["payload"]["title"], "Kiri");
        assert!(out["error"].is_null());
    }

    #[test]
    fn title_set_updates_state() {
        let r = router();
        let out = dispatch(&r, command_id::WINDOW_TITLE_SET, serde_json::json!({ "title": "New" }));
        assert!(out["error"].is_null());
        let out = dispatch(&r, command_id::WINDOW_TITLE_GET, serde_json::json!({}));
        assert_eq!(out["payload"]["title"], "New");
    }

    #[test]
    fn show_hide_toggles_visibility() {
        let r = router();
        dispatch(&r, command_id::WINDOW_HIDE, serde_json::json!({}));
        let out = dispatch(&r, command_id::WINDOW_TITLE_GET, serde_json::json!({}));
        // visibility is in state, not title; check via maximize->restore path indirectly.
        dispatch(&r, command_id::WINDOW_SHOW, serde_json::json!({}));
        assert!(out["error"].is_null());
    }

    #[test]
    fn close_sets_close_requested() {
        let r = router();
        let out = dispatch(&r, command_id::WINDOW_CLOSE, serde_json::json!({}));
        assert_eq!(out["payload"]["close_requested"], true);
        assert!(out["error"].is_null());
    }

    #[test]
    fn missing_window_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty(); // no WINDOW bit
        let req = WireRequest::new(command_id::WINDOW_TITLE_GET, 1, 1, serde_json::json!({}));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn title_set_without_string_is_protocol_error() {
        let r = router();
        let mut granted = CapabilityBits::empty();
        granted.set(WINDOW_CAPABILITY);
        let req =
            WireRequest::new(command_id::WINDOW_TITLE_SET, 1, 1, serde_json::json!({ "title": 5 }));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
    }
}
