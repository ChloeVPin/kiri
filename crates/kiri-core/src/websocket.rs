//! Restricted WebSocket surface (`kiri.ws`).
//!
//! This closes the Tauri `websocket` parity gap (G-11) and exceeds it on the
//! security axis. Tauri's `websocket` plugin, once the capability is present,
//! lets the frontend open a socket to any URL and send any frame. Kiri requires
//! BOTH the `WS` capability bit AND a host-owned URL allowlist: the frontend may
//! only connect to pre-approved origins, so a granted capability cannot be
//! pivoted into an exfiltration or SSRF channel. Outbound frames are bounded by
//! the shared bulk-object ceiling, so a hostile frontend cannot stream
//! unbounded bytes. Inbound frames are delivered back to the frontend only
//! through the host-owned channel allowlist, never as raw socket metadata.
//!
//! The actual socket is behind the `WsBackend` trait (mirrors `TrayRunner`):
//! the native host injects a real backend; tests use a `StubWs` and assert
//! URL-allowlist enforcement and capability gating headlessly, with no socket.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.ws.*` commands.
pub const WS_CAPABILITY: u32 = 25;

/// A WebSocket message delivered to the frontend. Direction and payload are
/// bounded; the backend never emits a frame for an unapproved connection.
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub direction: String,
    pub payload: String,
}

/// Host-configured set of permitted connection URLs. Default-deny: a URL is
/// reachable only if it is an exact entry (scheme + authority). This is the
/// concrete mechanism that turns Tauri's "arbitrary connect when granted" into
/// Kiri's "explicit allowlist only".
#[derive(Debug, Clone, Default)]
pub struct WsAllowlist {
    urls: Vec<String>,
}

impl WsAllowlist {
    pub fn new(urls: Vec<String>) -> Self {
        Self { urls }
    }

    pub fn allows(&self, url: &str) -> bool {
        self.urls.iter().any(|u| u == url)
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }
}

/// Transport seam. The native host provides a real socket client; tests provide
/// a stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait WsBackend: Send + Sync {
    /// Open a connection to the host-approved url; returns a host-assigned
    /// connection id.
    fn open(&self, url: &str) -> Result<u64>;
    /// Send a text frame on the given connection id.
    fn send(&self, conn_id: u64, message: &str) -> Result<()>;
    /// Close the given connection id.
    fn close(&self, conn_id: u64) -> Result<()>;
    /// Drain queued inbound messages for a connection id.
    fn drain(&self, conn_id: u64) -> Vec<WsMessage>;
}

/// Production backend used when no live socket client is wired into this build.
/// The command stays registered and capability-gated; the transport simply
/// reports that it is not available, so the frontend gets an explicit error
/// instead of an unregistered (unknown-command) failure.
pub struct DisabledWs;

impl WsBackend for DisabledWs {
    fn open(&self, _url: &str) -> Result<u64> {
        Err(Error::service_unavailable("kiri.ws.connect backend not wired in this build"))
    }
    fn send(&self, _conn_id: u64, _message: &str) -> Result<()> {
        Err(Error::service_unavailable("kiri.ws.send backend not wired in this build"))
    }
    fn close(&self, _conn_id: u64) -> Result<()> {
        Err(Error::service_unavailable("kiri.ws.close backend not wired in this build"))
    }
    fn drain(&self, _conn_id: u64) -> Vec<WsMessage> {
        Vec::new()
    }
}

/// Capability-scoped WebSocket service bounded to a URL allowlist plus limits.
#[derive(Clone)]
pub struct WsService {
    backend: Arc<dyn WsBackend>,
    allowlist: Arc<WsAllowlist>,
    limits: Arc<Limits>,
}

impl WsService {
    pub fn new(backend: Arc<dyn WsBackend>, allowlist: WsAllowlist, limits: Limits) -> Self {
        Self { backend, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Open a connection to a host-allowlisted URL. The URL must be an exact
    /// entry in the allowlist; otherwise the request is refused.
    pub fn connect(&self, url: &str) -> Result<Value> {
        if !self.allowlist.allows(url) {
            return Err(Error::scope_denied(format!(
                "kiri.ws.connect: url not on allowlist: {url}"
            )));
        }
        self.limits.check_bulk_object(url.len() as u64)?;
        let conn_id = self.backend.open(url)?;
        Ok(serde_json::json!({ "connId": conn_id, "url": url }))
    }

    /// Send a bounded text frame on a host-assigned connection id.
    pub fn send(&self, conn_id: u64, message: &str) -> Result<Value> {
        self.limits.check_bulk_object(message.len() as u64)?;
        self.backend.send(conn_id, message)?;
        Ok(serde_json::json!({ "sent": true, "connId": conn_id }))
    }

    /// Close a host-assigned connection id.
    pub fn close(&self, conn_id: u64) -> Result<Value> {
        self.backend.close(conn_id)?;
        Ok(serde_json::json!({ "closed": true, "connId": conn_id }))
    }

    /// Drain pending inbound messages for a host-assigned connection id.
    pub fn drain(&self, conn_id: u64) -> Vec<WsMessage> {
        self.backend.drain(conn_id)
    }
}

/// Build the `kiri.ws.*` handlers bound to one WsService.
pub fn ws_handlers(
    service: WsService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(WS_CAPABILITY);

    let connect_svc = service.clone();
    let send_svc = service.clone();
    let close_svc = service.clone();
    vec![
        (
            command_id::WS_CONNECT,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let url = p.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.ws.connect requires string url")
                })?;
                connect_svc.connect(url)
            }) as Handler,
        ),
        (
            command_id::WS_SEND,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let conn_id = p.get("connId").and_then(|v| v.as_u64()).ok_or_else(|| {
                    Error::invalid_argument("kiri.ws.send requires numeric connId")
                })?;
                let message = p.get("message").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.ws.send requires string message")
                })?;
                send_svc.send(conn_id, message)
            }) as Handler,
        ),
        (
            command_id::WS_CLOSE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let conn_id = p.get("connId").and_then(|v| v.as_u64()).ok_or_else(|| {
                    Error::invalid_argument("kiri.ws.close requires numeric connId")
                })?;
                close_svc.close(conn_id)
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

    struct StubWs {
        conns: Mutex<std::collections::HashMap<u64, String>>,
        inbox: Mutex<std::collections::HashMap<u64, Vec<WsMessage>>>,
        next: Mutex<u64>,
    }
    impl WsBackend for StubWs {
        fn open(&self, url: &str) -> Result<u64> {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let id = *next;
            self.conns.lock().unwrap().insert(id, url.to_string());
            Ok(id)
        }
        fn send(&self, conn_id: u64, message: &str) -> Result<()> {
            self.inbox
                .lock()
                .unwrap()
                .entry(conn_id)
                .or_default()
                .push(WsMessage { direction: "out".to_string(), payload: message.to_string() });
            Ok(())
        }
        fn close(&self, conn_id: u64) -> Result<()> {
            self.conns.lock().unwrap().remove(&conn_id);
            Ok(())
        }
        fn drain(&self, conn_id: u64) -> Vec<WsMessage> {
            self.inbox.lock().unwrap().remove(&conn_id).unwrap_or_default()
        }
    }

    fn allow() -> WsAllowlist {
        WsAllowlist::new(vec!["wss://api.example.com/feed".to_string()])
    }

    fn router() -> Router {
        let svc = WsService::new(
            Arc::new(StubWs {
                conns: Mutex::new(std::collections::HashMap::new()),
                inbox: Mutex::new(std::collections::HashMap::new()),
                next: Mutex::new(0),
            }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_ws(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(WS_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_url_connects() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::WS_CONNECT,
            serde_json::json!({ "url": "wss://api.example.com/feed" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert!(out["payload"]["connId"].as_u64().is_some());
    }

    #[test]
    fn disallowed_url_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::WS_CONNECT,
            serde_json::json!({ "url": "wss://evil.example.net/x" }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn send_then_close() {
        let r = router();
        let c = dispatch(
            &r,
            command_id::WS_CONNECT,
            serde_json::json!({ "url": "wss://api.example.com/feed" }),
        );
        let id = c["payload"]["connId"].as_u64().unwrap();
        let s = dispatch(
            &r,
            command_id::WS_SEND,
            serde_json::json!({ "connId": id, "message": "hello" }),
        );
        assert!(s["error"].is_null(), "unexpected error: {s}");
        let cl = dispatch(&r, command_id::WS_CLOSE, serde_json::json!({ "connId": id }));
        assert!(cl["error"].is_null(), "unexpected error: {cl}");
    }

    #[test]
    fn capability_denied_without_ws_bit() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::WS_CONNECT,
            1,
            1,
            serde_json::json!({ "url": "wss://api.example.com/feed" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
