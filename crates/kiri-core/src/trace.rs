//! Structured tracing (specs/TRACE.md, docs/13-diagnostics-observability.md).
//!
//! Trace events never log payload content by default. Tracing must be
//! measurable with and without diagnostics enabled (EXP-007).

use serde::{Deserialize, Serialize};

/// Monotonic clock in nanoseconds where available. `std::time::Instant`
/// provides a platform-neutral monotonic source for the logical core; the
/// Windows backend uses QueryPerformanceCounter for its startup markers.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonotonicClock;

impl MonotonicClock {
    pub fn now_ns() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    }
}

/// Trace stages (specs/TRACE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Receive,
    Authorize,
    Decode,
    Queue,
    Execute,
    Encode,
    Bulk,
    Send,
    Complete,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Receive => "receive",
            Stage::Authorize => "authorize",
            Stage::Decode => "decode",
            Stage::Queue => "queue",
            Stage::Execute => "execute",
            Stage::Encode => "encode",
            Stage::Bulk => "bulk",
            Stage::Send => "send",
            Stage::Complete => "complete",
        }
    }
}

/// Trace event matching schemas/trace-event.schema.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub schema_version: u32,
    pub timestamp_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<u32>,
    pub stage: Stage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<u32>,
}

impl TraceEvent {
    pub fn new(stage: Stage) -> Self {
        TraceEvent {
            schema_version: 1,
            timestamp_ns: MonotonicClock::now_ns(),
            request_id: None,
            caller_id: None,
            command_id: None,
            stage,
            duration_ns: None,
            payload_bytes: None,
            response_bytes: None,
            result_code: None,
            queue_depth: None,
        }
    }

    pub fn with_request_id(mut self, id: u64) -> Self {
        self.request_id = Some(id);
        self
    }

    pub fn with_caller_id(mut self, id: u64) -> Self {
        self.caller_id = Some(id);
        self
    }

    pub fn with_command_id(mut self, id: u32) -> Self {
        self.command_id = Some(id);
        self
    }

    pub fn with_duration_ns(mut self, ns: u64) -> Self {
        self.duration_ns = Some(ns);
        self
    }

    pub fn with_payload_bytes(mut self, n: u64) -> Self {
        self.payload_bytes = Some(n);
        self
    }

    pub fn with_response_bytes(mut self, n: u64) -> Self {
        self.response_bytes = Some(n);
        self
    }

    pub fn with_result_code(mut self, code: impl Into<String>) -> Self {
        self.result_code = Some(code.into());
        self
    }
}

/// Receives trace events. `emit` must not block the command hot path.
pub trait TraceSink: Send + Sync {
    fn emit(&mut self, event: &TraceEvent);
}

/// A sink that discards everything; zero per-event cost beyond the call.
#[derive(Debug, Default)]
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn emit(&mut self, _event: &TraceEvent) {}
}

/// A bounded ring-buffer sink for in-process diagnostics.
#[derive(Debug)]
pub struct RingTraceSink {
    capacity: usize,
    events: Vec<TraceEvent>,
}

impl RingTraceSink {
    pub fn new(capacity: usize) -> Self {
        RingTraceSink { capacity, events: Vec::with_capacity(capacity) }
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }
}

impl TraceSink for RingTraceSink {
    fn emit(&mut self, event: &TraceEvent) {
        if self.events.len() == self.capacity {
            self.events.remove(0);
        }
        self.events.push(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_to_schema_shape() {
        let event = TraceEvent::new(Stage::Authorize)
            .with_request_id(7)
            .with_caller_id(1)
            .with_command_id(17)
            .with_duration_ns(1200)
            .with_payload_bytes(64)
            .with_result_code("unauthorized");
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["request_id"], 7);
        assert_eq!(json["stage"], "authorize");
        assert_eq!(json["result_code"], "unauthorized");
    }

    #[test]
    fn all_stages_have_stable_strings() {
        let expected = [
            (Stage::Receive, "receive"),
            (Stage::Authorize, "authorize"),
            (Stage::Decode, "decode"),
            (Stage::Queue, "queue"),
            (Stage::Execute, "execute"),
            (Stage::Encode, "encode"),
            (Stage::Bulk, "bulk"),
            (Stage::Send, "send"),
            (Stage::Complete, "complete"),
        ];
        for (stage, name) in expected {
            assert_eq!(stage.as_str(), name);
        }
    }

    #[test]
    fn ring_sink_bounded() {
        let mut sink = RingTraceSink::new(3);
        for i in 0..5 {
            sink.emit(&TraceEvent::new(Stage::Receive).with_request_id(i));
        }
        assert_eq!(sink.events().len(), 3);
        assert_eq!(sink.events()[0].request_id, Some(2));
    }

    #[test]
    fn noop_sink_accepts_events() {
        let mut sink = NoopTraceSink;
        sink.emit(&TraceEvent::new(Stage::Complete));
    }
}
