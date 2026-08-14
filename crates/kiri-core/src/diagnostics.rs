//! Runtime diagnostics aggregator (T010, docs/13-diagnostics-observability.md).
//!
//! `Diagnostics` implements `TraceSink`, so the control-plane router can feed
//! every request's stage events into it. It keeps a bounded ring of recent
//! requests with their per-stage timings, the live open-resource count, and
//! version/backend metadata, and can serialize a privacy-safe snapshot for the
//! developer panel. Per the trace spec, payload *content* is never stored;
//! only sizes and result categories are retained.

use std::sync::Mutex;

use serde::Serialize;

use crate::trace::{Stage, TraceEvent, TraceSink};

/// Maximum number of recent requests retained for the panel.
pub const MAX_RECENT: usize = 64;

/// One stage entry in a request's latency waterfall.
#[derive(Debug, Clone, Serialize)]
pub struct StageTiming {
    pub stage: String,
    /// Milliseconds elapsed during this stage (delta to the next event).
    pub ms: f64,
}

/// A single observed request, privacy-safe (no payload content).
#[derive(Debug, Clone, Serialize)]
pub struct RequestTrace {
    pub request_id: u64,
    pub command_id: u32,
    /// Resolved command string ID when the catalog knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<u64>,
    /// "ok" or the error category string.
    pub result: String,
    pub total_ms: f64,
    pub payload_bytes: u64,
    pub response_bytes: u64,
    pub stages: Vec<StageTiming>,
}

/// Serializable snapshot consumed by the developer panel.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsSnapshot {
    pub schema_version: u32,
    pub runtime_version: String,
    /// "cross" (wry/tao) or "windows" (Win32/WebView2).
    pub backend: String,
    pub open_resources: u32,
    pub recent_requests: Vec<RequestTrace>,
}

#[derive(Default)]
struct Inner {
    recent: Vec<RequestTrace>,
    open_resources: u32,
    /// Pending events keyed by request id while a request is in flight.
    pending: Vec<TraceEvent>,
}

/// Shared, thread-safe diagnostics sink.
#[derive(Clone, Default)]
pub struct Diagnostics {
    inner: std::sync::Arc<Mutex<Inner>>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the current live open-resource count (set by the runtime from
    /// its `ResourceTable`).
    pub fn set_open_resources(&self, count: u32) {
        let mut g = self.inner.lock().unwrap();
        g.open_resources = count;
    }

    /// Build a privacy-safe snapshot for the developer panel.
    pub fn snapshot(&self, runtime_version: &str, backend: &str) -> DiagnosticsSnapshot {
        let g = self.inner.lock().unwrap();
        DiagnosticsSnapshot {
            schema_version: 1,
            runtime_version: runtime_version.to_string(),
            backend: backend.to_string(),
            open_resources: g.open_resources,
            recent_requests: g.recent.clone(),
        }
    }
}

impl TraceSink for Diagnostics {
    fn emit(&mut self, event: &TraceEvent) {
        let mut g = self.inner.lock().unwrap();
        match event.stage {
            Stage::Receive => {
                // Start a fresh in-flight capture for this request.
                g.pending.retain(|e| e.request_id != event.request_id);
                g.pending.push(event.clone());
            }
            Stage::Complete => {
                // Assemble the request trace from everything captured so far.
                let mut events: Vec<TraceEvent> = g
                    .pending
                    .iter()
                    .filter(|e| e.request_id == event.request_id)
                    .cloned()
                    .collect();
                g.pending.retain(|e| e.request_id != event.request_id);
                events.push(event.clone());
                events.sort_by_key(|e| e.timestamp_ns);

                let total_ns = events
                    .last()
                    .and_then(|e| {
                        e.timestamp_ns.checked_sub(
                            events.first().map(|e| e.timestamp_ns).unwrap_or(e.timestamp_ns),
                        )
                    })
                    .unwrap_or(0);

                let mut stages = Vec::new();
                for w in events.windows(2) {
                    let delta = w[1].timestamp_ns.saturating_sub(w[0].timestamp_ns);
                    stages.push(StageTiming {
                        stage: w[0].stage.as_str().to_string(),
                        ms: delta as f64 / 1_000_000.0,
                    });
                }

                // The real pipeline stamps command_id/caller_id on the
                // mid-pipeline events, not on Receive/Complete, so scan
                // every captured event for the authoritative ids.
                let command_id = events.iter().filter_map(|e| e.command_id).next().unwrap_or(0);
                let command_name = crate::commands::command_name(command_id).map(|s| s.to_string());
                let caller_id = events.iter().filter_map(|e| e.caller_id).next();
                let payload_bytes =
                    events.iter().filter_map(|e| e.payload_bytes).next().unwrap_or(0);
                let response_bytes =
                    events.iter().filter_map(|e| e.response_bytes).next().unwrap_or(0);
                let result = event.result_code.clone().unwrap_or_else(|| "ok".to_string());

                let trace = RequestTrace {
                    request_id: event.request_id.unwrap_or(0),
                    command_id,
                    command_name,
                    caller_id,
                    result,
                    total_ms: total_ns as f64 / 1_000_000.0,
                    payload_bytes,
                    response_bytes,
                    stages,
                };
                g.recent.push(trace);
                if g.recent.len() > MAX_RECENT {
                    let drop = g.recent.len() - MAX_RECENT;
                    g.recent.drain(0..drop);
                }
            }
            _ => {
                g.pending.push(event.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{MonotonicClock, Stage};

    fn ev(stage: Stage, request_id: u64, ts_offset_ns: u64) -> TraceEvent {
        let mut e = TraceEvent::new(stage).with_request_id(request_id);
        e.timestamp_ns = MonotonicClock::now_ns() + ts_offset_ns;
        e
    }

    #[test]
    fn assembles_recent_request_without_payload_content() {
        let mut d = Diagnostics::new();
        d.emit(&ev(Stage::Receive, 7, 0));
        d.emit(&ev(Stage::Authorize, 7, 1_000_000));
        d.emit(&ev(Stage::Decode, 7, 2_000_000).with_payload_bytes(64));
        d.emit(&ev(Stage::Execute, 7, 3_000_000).with_duration_ns(500_000));
        d.emit(&ev(Stage::Encode, 7, 4_000_000).with_response_bytes(48));
        d.emit(&ev(Stage::Send, 7, 5_000_000));
        d.emit(&ev(Stage::Complete, 7, 6_000_000).with_result_code("ok").with_command_id(1));

        let snap = d.snapshot("0.1.0", "cross");
        assert_eq!(snap.recent_requests.len(), 1);
        let r = &snap.recent_requests[0];
        assert_eq!(r.request_id, 7);
        assert_eq!(r.command_id, 1);
        assert_eq!(r.command_name.as_deref(), Some("kiri.ping"));
        assert_eq!(r.result, "ok");
        assert_eq!(r.payload_bytes, 64);
        assert_eq!(r.response_bytes, 48);
        assert!(r.total_ms > 0.0);
        assert!(!r.stages.is_empty());
    }

    #[test]
    fn open_resources_is_reported() {
        let d = Diagnostics::new();
        d.set_open_resources(3);
        assert_eq!(d.snapshot("0.1.0", "cross").open_resources, 3);
    }

    #[test]
    fn recent_is_bounded() {
        let mut d = Diagnostics::new();
        for i in 0..(MAX_RECENT + 10) {
            d.emit(&ev(Stage::Receive, i as u64, 0));
            d.emit(&ev(Stage::Complete, i as u64, 1_000_000).with_result_code("ok"));
        }
        assert_eq!(d.snapshot("0.1.0", "cross").recent_requests.len(), MAX_RECENT);
    }
}
