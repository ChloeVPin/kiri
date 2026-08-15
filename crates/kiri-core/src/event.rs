//! Restricted event-bus surface (`kiri.event`).
//!
//! This converts Tauri's unrestricted event model into a Kiri strength
//! (audit item 16). Tauri's `event` module lets any granted frontend emit and
//! listen on any channel once the capability is present — a cross-module
//! information-leak and spoofing surface: a malicious or buggy frontend can
//! forge events on channels owned by other modules, or snoop on their traffic.
//! Kiri requires BOTH the `EVENT` capability bit AND a host allowlist of exact
//! channel names; the frontend may only publish/subscribe pre-approved channels
//! whose names are host-owned. A granted capability addressing an unknown
//! channel is refused, so JavaScript can never forge or snoop cross-module
//! events. The actual bus is behind a `EventBusBackend` trait (mirrors
//! `ShellRunner`): the native host injects a real bus; tests use a `StubBus`
//! and assert authorization and channel-allowlist enforcement headlessly.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.event.*` commands.
/// Reuses the shared `EVENT` capability bit (5) so this restricted surface
/// stays in lockstep with `capability_bit::EVENT` and `for_command`. A separate
/// bit here would let the real runtime grant bit 5 while the handler required
/// bit 2, denying every `kiri.event.*` call.
pub const EVENT_CAPABILITY: u32 = crate::dispatch::capability_bit::EVENT;

/// One host-approved event channel. The frontend references `channel` only; it
/// cannot invent a channel name. The host owns the set of valid channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedChannel {
    pub name: String,
}

/// Host-configured allowlist of event channels. Default-deny: a publish or
/// subscribe succeeds only if the exact channel name is listed. The host owns
/// the channel namespace; the frontend only picks a known channel.
#[derive(Debug, Clone, Default)]
pub struct EventAllowlist {
    channels: Vec<AllowedChannel>,
}

impl EventAllowlist {
    pub fn new(channels: Vec<AllowedChannel>) -> Self {
        Self { channels }
    }

    fn allows(&self, name: &str) -> bool {
        self.channels.iter().any(|c| c.name == name)
    }

    pub fn channels(&self) -> &[AllowedChannel] {
        &self.channels
    }
}

/// Transport seam. The native host provides a real bus; tests provide a stub.
/// Kept trait-based so the logical protocol has zero platform deps.
pub trait EventBusBackend: Send + Sync {
    /// Subscribe to `channel`; returns a host-assigned subscriber id.
    fn subscribe(&self, channel: &str) -> u64;
    /// Publish `payload` to every subscriber of `channel`.
    fn publish(&self, channel: &str, payload: Value);
    /// Drain queued publications for a subscriber id.
    fn drain(&self, subscriber_id: u64) -> Vec<Value>;
}

/// Capability-scoped event service bounded to a channel allowlist plus limits.
#[derive(Clone)]
pub struct EventService {
    backend: Arc<dyn EventBusBackend>,
    allowlist: Arc<EventAllowlist>,
    limits: Arc<Limits>,
}

impl EventService {
    pub fn new(
        backend: Arc<dyn EventBusBackend>,
        allowlist: EventAllowlist,
        limits: Limits,
    ) -> Self {
        Self { backend, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Subscribe to a host-allowlisted channel. The frontend may only name a
    /// pre-approved channel; an unknown channel is refused.
    pub fn subscribe(&self, channel: &str) -> Result<Value> {
        if !self.allowlist.allows(channel) {
            return Err(Error::scope_denied(format!(
                "kiri.event.listen: channel not on allowlist: {channel}"
            )));
        }
        let id = self.backend.subscribe(channel);
        Ok(serde_json::json!({ "listener_id": id, "channel": channel }))
    }

    /// Publish to a host-allowlisted channel. The frontend may only target a
    /// pre-approved channel; an unknown channel is refused. Payload size is
    /// bounded by the shared bulk-object ceiling.
    pub fn publish(&self, channel: &str, payload: &Value) -> Result<Value> {
        if !self.allowlist.allows(channel) {
            return Err(Error::scope_denied(format!(
                "kiri.event.emit: channel not on allowlist: {channel}"
            )));
        }
        let serialized = serde_json::to_vec(payload).map_err(|e| {
            Error::invalid_argument(format!("kiri.event.emit: payload not serializable: {e}"))
        })?;
        self.limits.check_bulk_object(serialized.len() as u64)?;
        self.backend.publish(channel, payload.clone());
        Ok(serde_json::json!({ "emitted": true, "channel": channel }))
    }

    /// Drain pending publications for a host-assigned subscriber id.
    pub fn drain(&self, subscriber_id: u64) -> Vec<Value> {
        self.backend.drain(subscriber_id)
    }

    /// Report which host-allowlisted channels exist (names only). Lets the
    /// frontend discover what it may use without ever naming an arbitrary
    /// channel itself.
    pub fn list(&self) -> Value {
        serde_json::json!({
            "channels": self.allowlist.channels().iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        })
    }
}

/// Build the kiri.event handlers bound to one EventService.
pub fn event_handlers(
    service: EventService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(EVENT_CAPABILITY);

    let sub_svc = service.clone();
    let pub_svc = service.clone();
    let list_svc = service.clone();
    vec![
        (
            command_id::EVENT_PUBLISH,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let channel = p.get("event").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.event.emit requires string event")
                })?;
                let payload = p.get("payload").cloned().unwrap_or(Value::Null);
                pub_svc.publish(channel, &payload)
            }) as Handler,
        ),
        (
            command_id::EVENT_SUBSCRIBE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let channel = p.get("event").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.event.listen requires string event")
                })?;
                sub_svc.subscribe(channel)
            }) as Handler,
        ),
        (
            command_id::EVENT_CHANNELS,
            required,
            Arc::new(move |_c, _rid, _p: &Value| Ok(list_svc.list())) as Handler,
        ),
    ]
}

/// Bridge the existing in-process `platform::EventBus` to the restricted
/// `EventBusBackend` trait so the runtime can reuse one real bus for both the
/// legacy R-3 surface and the channel-allowlisted audit-16 surface.
impl EventBusBackend for crate::platform::EventBus {
    fn subscribe(&self, channel: &str) -> u64 {
        self.subscribe(channel)
    }
    fn publish(&self, channel: &str, payload: Value) {
        self.publish(channel, payload)
    }
    fn drain(&self, subscriber_id: u64) -> Vec<Value> {
        self.drain(subscriber_id)
    }
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

    struct StubBus {
        next: Mutex<u64>,
        subs: Mutex<std::collections::HashMap<u64, String>>,
        queues: Mutex<std::collections::HashMap<String, Vec<Value>>>,
    }
    impl EventBusBackend for StubBus {
        fn subscribe(&self, channel: &str) -> u64 {
            let id = {
                let mut n = self.next.lock().unwrap();
                *n += 1;
                *n
            };
            self.subs.lock().unwrap().insert(id, channel.to_string());
            id
        }
        fn publish(&self, channel: &str, payload: Value) {
            self.queues.lock().unwrap().entry(channel.to_string()).or_default().push(payload);
        }
        fn drain(&self, subscriber_id: u64) -> Vec<Value> {
            let channel = self.subs.lock().unwrap().get(&subscriber_id).cloned();
            match channel {
                Some(ch) => self
                    .queues
                    .lock()
                    .unwrap()
                    .get_mut(&ch)
                    .map(|q| std::mem::take(q))
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    }

    fn allow() -> EventAllowlist {
        EventAllowlist::new(vec![
            AllowedChannel { name: "ping".to_string() },
            AllowedChannel { name: "update".to_string() },
        ])
    }

    fn router() -> Router {
        let svc = EventService::new(
            Arc::new(StubBus {
                next: Mutex::new(0),
                subs: Mutex::new(std::collections::HashMap::new()),
                queues: Mutex::new(std::collections::HashMap::new()),
            }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_event(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(EVENT_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_emit_returns_emitted() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::EVENT_PUBLISH,
            serde_json::json!({ "event": "ping", "payload": { "n": 1 } }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["emitted"], true);
    }

    #[test]
    fn unknown_channel_emit_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::EVENT_PUBLISH,
            serde_json::json!({ "event": "evil", "payload": {} }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn allowed_listen_returns_id() {
        let r = router();
        let out =
            dispatch(&r, command_id::EVENT_SUBSCRIBE, serde_json::json!({ "event": "update" }));
        assert!(out["error"].is_null());
        assert!(out["payload"]["listener_id"].as_u64().is_some());
    }

    #[test]
    fn unknown_channel_listen_denied() {
        let r = router();
        let out = dispatch(&r, command_id::EVENT_SUBSCRIBE, serde_json::json!({ "event": "evil" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn list_returns_channel_names_only() {
        let r = router();
        let out = dispatch(&r, command_id::EVENT_CHANNELS, serde_json::json!({}));
        assert!(out["error"].is_null());
        let channels = out["payload"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);
    }
}
