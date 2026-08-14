//! Control-plane dispatch (T003): orchestrates validation, authorization,
//! decode, execution, and tracing for one request, then builds the wire
//! response. This is the layer the platform transports call after they have
//! identified the native caller.
//!
//! The dispatch order follows specs/IPC.md exactly: outer type -> version ->
//! command id -> payload length -> authorize. Application command code runs
//! only after validation and authorization succeed. Trace events are emitted
//! for the mandated stages so the latency benchmark can attribute time.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::caller::CallerId;
use crate::capabilities::CapabilityBits;
use crate::error::{Error, Result};
use crate::header::ControlHeader;
use crate::limits::Limits;
use crate::trace::{Stage, TraceEvent, TraceSink};
use crate::validate;
use crate::wire::{WireRequest, WireResponse};

/// Command IDs reserved by the runtime control plane.
pub mod command_id {
    /// Echo/pong command used for liveness and latency probing.
    pub const PING: u32 = 1;
}

/// Capability bits used by built-in control commands.
pub mod capability_bit {
    /// Authorizes the `ping` liveness probe. Bit 0.
    pub const PING: u32 = 0;
}

/// A command handler. Receives the authoritative caller, the request id, and
/// the already-decoded JSON payload. Returns the response payload or an error.
pub type Handler = Arc<dyn Fn(CallerId, u64, &Value) -> Result<Value> + Send + Sync>;

/// Registered command: its required capability and handler.
#[derive(Clone)]
struct Command {
    required: CapabilityBits,
    handler: Handler,
}

/// The control-plane router: maps command IDs to commands and runs the
/// mandated validation + trace pipeline for each request.
///
/// `Router` is cheaply cloneable (handlers are shared via `Arc`); the runtime
/// clones it into each per-connection dispatch context.
#[derive(Clone)]
pub struct Router {
    commands: HashMap<u32, Command>,
    limits: Limits,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        let mut router = Router { commands: HashMap::new(), limits: Limits::default() };
        router.register_ping();
        router
    }

    /// Override the default limits (used by tests and tuning).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Register a command with its required capability and handler.
    pub fn register(&mut self, id: u32, required: CapabilityBits, handler: Handler) {
        self.commands.insert(id, Command { required, handler });
    }

    fn register_ping(&mut self) {
        let mut required = CapabilityBits::empty();
        required.set(capability_bit::PING);
        self.register(
            command_id::PING,
            required,
            Arc::new(|_caller, _request_id, payload| {
                // Echo the payload back so request correlation is observable
                // end to end; the benchmark asserts request_id maps to pong.
                Ok(serde_json::json!({ "pong": true, "echo": payload }))
            }),
        );
    }

    /// Returns true when the command id is registered.
    pub fn is_known(&self, id: u32) -> bool {
        self.commands.contains_key(&id)
    }

    /// Dispatch one parsed wire request from an already-identified caller.
    ///
    /// Emits trace events for receive/authorize/decode/execute/encode/send/
    /// complete and returns the wire response. Malformed input is rejected
    /// with a stable error before any handler runs.
    pub fn dispatch(
        &self,
        caller: CallerId,
        caller_capabilities: &CapabilityBits,
        request: &WireRequest,
        sink: &mut dyn TraceSink,
    ) -> WireResponse {
        let request_id = request.request_id;
        sink.emit(&TraceEvent::new(Stage::Receive).with_request_id(request_id));

        // Reconstruct the logical header for the validation pipeline. The wire
        // envelope carries the same fields the logical protocol requires.
        let header = ControlHeader {
            magic: request.magic,
            version: request.version,
            flags: request.flags,
            command_id: request.command_id,
            request_id,
            payload_len: request.payload_len,
            codec: request.codec,
            reserved: 0,
            resource_count: 0,
        };

        let actual_len = serde_json::to_vec(&request.payload).unwrap_or_default().len() as u32;
        let validated = match validate::validate_request(
            caller,
            &header,
            actual_len,
            caller_capabilities,
            self.command_required(caller, request.command_id),
            &self.limits,
            |id| self.is_known(id),
        ) {
            Ok(v) => v,
            Err(e) => {
                let e = e.with_request_id(request_id);
                sink.emit(
                    &TraceEvent::new(Stage::Complete)
                        .with_request_id(request_id)
                        .with_result_code(e.code.as_str()),
                );
                return WireResponse::err(request_id, e);
            }
        };

        sink.emit(
            &TraceEvent::new(Stage::Authorize)
                .with_request_id(request_id)
                .with_command_id(validated.command_id),
        );
        sink.emit(
            &TraceEvent::new(Stage::Decode)
                .with_request_id(request_id)
                .with_payload_bytes(actual_len as u64),
        );

        let start = crate::trace::MonotonicClock::now_ns();
        let result = match self.commands.get(&validated.command_id) {
            Some(cmd) => (cmd.handler)(caller, request_id, &request.payload),
            None => {
                Err(Error::protocol_error(format!("unknown command id {}", validated.command_id)))
            }
        };
        let elapsed = crate::trace::MonotonicClock::now_ns().saturating_sub(start);

        sink.emit(
            &TraceEvent::new(Stage::Execute)
                .with_request_id(request_id)
                .with_command_id(validated.command_id)
                .with_duration_ns(elapsed),
        );

        let response = match result {
            Ok(payload) => {
                sink.emit(
                    &TraceEvent::new(Stage::Encode).with_request_id(request_id).with_payload_bytes(
                        serde_json::to_vec(&payload).unwrap_or_default().len() as u64,
                    ),
                );
                WireResponse::ok(request_id, payload)
            }
            Err(e) => {
                let e = e.with_request_id(request_id);
                sink.emit(
                    &TraceEvent::new(Stage::Complete)
                        .with_request_id(request_id)
                        .with_result_code(e.code.as_str()),
                );
                WireResponse::err(request_id, e)
            }
        };
        sink.emit(&TraceEvent::new(Stage::Send).with_request_id(request_id));
        sink.emit(&TraceEvent::new(Stage::Complete).with_request_id(request_id));
        response
    }

    fn command_required(&self, _caller: CallerId, id: u32) -> CapabilityBits {
        self.commands.get(&id).map(|c| c.required).unwrap_or_else(CapabilityBits::empty)
    }
}

/// Build a `WireRequest` for the built-in ping command (helper for tests and
/// the runtime bridge).
pub fn ping_request(request_id: u64, payload: Value) -> WireRequest {
    WireRequest::new(command_id::PING, request_id, 1, payload)
}

/// True when a wire response is a successful pong for the given request id.
pub fn is_pong(response: &WireResponse, request_id: u64) -> bool {
    response.request_id == request_id
        && response.error.is_none()
        && matches!(&response.payload, Some(Value::Object(map)) if map.get("pong") == Some(&Value::Bool(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::latency::LatencyDistribution;
    use crate::trace::RingTraceSink;
    use serde_json::json;

    fn caller_caps() -> CapabilityBits {
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        caps
    }

    #[test]
    fn ping_returns_pong_with_echoed_payload() {
        let router = Router::new();
        let req = ping_request(7, json!({ "hello": "world" }));
        let mut sink = RingTraceSink::new(64);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(is_pong(&resp, 7));
        let payload = resp.payload.unwrap();
        assert_eq!(payload["echo"], json!({ "hello": "world" }));
    }

    #[test]
    fn request_ids_correlate_across_many_requests() {
        let router = Router::new();
        let mut sink = RingTraceSink::new(1024);
        for id in 1u64..=500 {
            let req = ping_request(id, json!({ "n": id }));
            let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
            assert!(is_pong(&resp, id), "request {id} did not correlate");
            assert_eq!(resp.payload.as_ref().unwrap()["echo"]["n"], id);
        }
    }

    #[test]
    fn ten_thousand_ping_loop_completes_with_latency_distribution() {
        let router = Router::new();
        let mut sink = RingTraceSink::new(2048);
        let mut dist = LatencyDistribution::new();
        for id in 1u64..=10_000 {
            let start = crate::trace::MonotonicClock::now_ns();
            let req = ping_request(id, json!({ "i": id }));
            let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
            let elapsed = crate::trace::MonotonicClock::now_ns().saturating_sub(start);
            assert!(is_pong(&resp, id));
            dist.record(elapsed);
        }
        assert_eq!(dist.count(), 10_000);
        let summary = dist.summary();
        assert!(summary.min_ns <= summary.p50_ns);
        assert!(summary.p50_ns <= summary.p99_ns);
        assert!(summary.p99_ns <= summary.max_ns);
        assert!(summary.max_ns > 0, "latency distribution must be emitted");
    }

    #[test]
    fn malformed_magic_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.magic = *b"NOPE";
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn malformed_version_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.version = 999;
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn malformed_payload_length_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.payload_len = req.payload_len + 1; // declared != actual
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn unknown_command_id_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.command_id = 4242;
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn missing_capability_denied() {
        let router = Router::new();
        // caller with no capabilities at all
        let empty = CapabilityBits::empty();
        let req = ping_request(1, json!(null));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &empty, &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn invalid_json_payload_for_ping_still_roundtrips() {
        // ping echoes arbitrary JSON; a non-object payload must not panic.
        let router = Router::new();
        let req = ping_request(3, json!("a-string"));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(is_pong(&resp, 3));
        assert_eq!(resp.payload.as_ref().unwrap()["echo"], json!("a-string"));
    }
}
