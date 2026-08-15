//! Capability-gated clipboard surface (kiri.clipboard).
//!
//! This closes the Tauri clipboard plugin parity gap (G-6) and exceeds it on
//! the security axis: every operation is authorized by the central capability
//! authority (bit CLIPBOARD) and routed through a host-owned
//! ClipboardController. JavaScript can read/write the system clipboard only
//! with the explicit clipboard capability granted by native code; it cannot
//! reach the OS clipboard API directly, and every access is recorded for audit.
//!
//! The controller is a thin trait the native host implements (arboard on
//! macOS/Linux/Windows). The state mirror (ClipboardState) is owned by core so
//! the control plane can be exercised headlessly: tests drive a StubClipboard
//! and assert routing, authorization, and state without touching the OS.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};

/// Authorizes the kiri.clipboard.* commands.
pub const CLIPBOARD_CAPABILITY: u32 = 8;

/// Mirror of the most recently written clipboard text. Kept in core so the
/// control-plane state is authoritative and testable without the OS clipboard.
/// The real system clipboard is the source of truth at read time; this mirror
/// only records what the control plane last wrote through Kiri.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardState {
    pub last_written: String,
}

impl ClipboardState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Host-provided clipboard backend. The native host implements this for the
/// real OS clipboard (arboard on macOS/Linux/Windows). Commands mutate state
/// (the core mirror) and call the corresponding native op.
pub trait ClipboardController: Send + Sync {
    /// Read the current system clipboard text.
    fn read(&self, state: &mut ClipboardState) -> Result<String>;
    /// Write text to the system clipboard.
    fn write(&self, state: &mut ClipboardState, text: &str);
}

/// Build the two kiri.clipboard.* handlers bound to one ClipboardController and
/// its shared ClipboardState mirror. Reused by the router builder and any plugin
/// path so authority is identical either way.
pub fn clipboard_handlers(
    controller: std::sync::Arc<dyn ClipboardController>,
    state: std::sync::Arc<std::sync::Mutex<ClipboardState>>,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(CLIPBOARD_CAPABILITY);

    let read_ctrl = controller.clone();
    let read_state = state.clone();
    let write_ctrl = controller.clone();
    let write_state = state.clone();

    vec![
        (
            command_id::CLIPBOARD_READ,
            required,
            Arc::new(move |_c, _rid, _p: &Value| {
                let text = read_ctrl.read(&mut read_state.lock().unwrap())?;
                Ok(serde_json::json!({ "text": text }))
            }) as Handler,
        ),
        (
            command_id::CLIPBOARD_WRITE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let text = p.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.clipboard.write requires string text")
                })?;
                write_ctrl.write(&mut write_state.lock().unwrap(), text);
                Ok(serde_json::json!({ "written": true }))
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

    struct StubClipboard {
        inner: std::sync::Mutex<String>,
    }
    impl ClipboardController for StubClipboard {
        fn read(&self, _state: &mut ClipboardState) -> Result<String> {
            Ok(self.inner.lock().unwrap().clone())
        }
        fn write(&self, state: &mut ClipboardState, text: &str) {
            *self.inner.lock().unwrap() = text.to_string();
            state.last_written = text.to_string();
        }
    }

    fn router() -> Router {
        let state = std::sync::Arc::new(std::sync::Mutex::new(ClipboardState::new()));
        let ctrl =
            std::sync::Arc::new(StubClipboard { inner: std::sync::Mutex::new(String::new()) });
        Router::new_with_limits(crate::limits::Limits::default()).with_clipboard(ctrl, state)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(CLIPBOARD_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn write_then_read_roundtrips() {
        let r = router();
        let out =
            dispatch(&r, command_id::CLIPBOARD_WRITE, serde_json::json!({ "text": "hi kiri" }));
        assert!(out["error"].is_null());
        assert_eq!(out["payload"]["written"], true);
        let out = dispatch(&r, command_id::CLIPBOARD_READ, serde_json::json!({}));
        assert!(out["error"].is_null());
        assert_eq!(out["payload"]["text"], "hi kiri");
    }

    #[test]
    fn missing_clipboard_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(command_id::CLIPBOARD_READ, 1, 1, serde_json::json!({}));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn write_without_string_is_protocol_error() {
        let r = router();
        let mut granted = CapabilityBits::empty();
        granted.set(CLIPBOARD_CAPABILITY);
        let req =
            WireRequest::new(command_id::CLIPBOARD_WRITE, 1, 1, serde_json::json!({ "text": 5 }));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
    }
}
