//! Monotonic startup markers (docs/12-benchmarks.md).
//!
//! Markers are recorded in nanoseconds on a QueryPerformanceCounter-based
//! monotonic clock, plus a delta from the first recorded marker so results
//! are stable regardless of when the clock is first read.

use std::collections::BTreeMap;

use serde::Serialize;

/// Marker names from docs/12-benchmarks.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    ProcessSpawnRequested,
    NativeEntry,
    PlatformInit,
    WebViewCreationRequested,
    WebViewReady,
    BridgeReady,
    DomReady,
    AppReady,
    FirstAnimationFrame,
    /// First `window.kiri.send()` entered native dispatch (lazy Router attach).
    FirstInvokeDispatched,
    /// First control-plane response was produced.
    FirstInvokeResponded,
}

impl Marker {
    pub const ALL: [Marker; 11] = [
        Marker::ProcessSpawnRequested,
        Marker::NativeEntry,
        Marker::PlatformInit,
        Marker::WebViewCreationRequested,
        Marker::WebViewReady,
        Marker::BridgeReady,
        Marker::DomReady,
        Marker::AppReady,
        Marker::FirstAnimationFrame,
        Marker::FirstInvokeDispatched,
        Marker::FirstInvokeResponded,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Marker::ProcessSpawnRequested => "process_spawn_requested",
            Marker::NativeEntry => "native_entry",
            Marker::PlatformInit => "platform_initialized",
            Marker::WebViewCreationRequested => "webview_creation_requested",
            Marker::WebViewReady => "webview_ready",
            Marker::BridgeReady => "bridge_ready",
            Marker::DomReady => "dom_ready",
            Marker::AppReady => "app_ready",
            Marker::FirstAnimationFrame => "first_animation_frame",
            Marker::FirstInvokeDispatched => "first_invoke_dispatched",
            Marker::FirstInvokeResponded => "first_invoke_responded",
        }
    }
}

/// Records monotonic marker timestamps.
#[derive(Debug, Default)]
pub struct StartupMarkers {
    // The first recorded timestamp serves as the t0 reference.
    t0_ns: Option<u64>,
    markers: BTreeMap<Marker, (u64, u64)>, // marker -> (absolute ns, delta from t0)
}

#[derive(Debug, Serialize)]
pub struct StartupResult {
    pub schema_version: u32,
    pub markers: Vec<MarkerRecord>,
}

#[derive(Debug, Serialize)]
pub struct MarkerRecord {
    pub name: &'static str,
    /// Absolute monotonic timestamp in nanoseconds.
    pub timestamp_ns: u64,
    /// Nanoseconds since the first recorded marker.
    pub since_first_ns: u64,
}

impl StartupMarkers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, marker: Marker, now_ns: u64) {
        let t0 = *self.t0_ns.get_or_insert(now_ns);
        self.markers.insert(marker, (now_ns, now_ns.saturating_sub(t0)));
    }

    /// Serialize to the `startup-result.json` shape (WP1 acceptance).
    pub fn result(&self) -> StartupResult {
        let markers = Marker::ALL
            .iter()
            .filter_map(|m| {
                self.markers.get(m).map(|(ts, since)| MarkerRecord {
                    name: m.name(),
                    timestamp_ns: *ts,
                    since_first_ns: *since,
                })
            })
            .collect();
        StartupResult { schema_version: 1, markers }
    }

    pub fn has(&self, marker: Marker) -> bool {
        self.markers.contains_key(&marker)
    }

    /// Produce an owned snapshot of the recorded markers.
    pub fn clone_markers(&self) -> StartupMarkers {
        StartupMarkers { t0_ns: self.t0_ns, markers: self.markers.clone() }
    }

    pub fn delta_ns(&self, from: Marker, to: Marker) -> Option<u64> {
        let a = self.markers.get(&from)?;
        let b = self.markers.get(&to)?;
        Some(b.0.saturating_sub(a.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_record_and_report_deltas() {
        let mut sm = StartupMarkers::new();
        sm.record(Marker::NativeEntry, 1000);
        sm.record(Marker::WebViewReady, 3500);
        assert_eq!(sm.delta_ns(Marker::NativeEntry, Marker::WebViewReady), Some(2500));
        assert!(sm.has(Marker::WebViewReady));
        assert!(!sm.has(Marker::AppReady));
    }

    #[test]
    fn result_json_contains_all_recorded_markers_in_order() {
        let mut sm = StartupMarkers::new();
        for (i, m) in Marker::ALL.iter().enumerate() {
            sm.record(*m, 1000 + i as u64 * 100);
        }
        let result = sm.result();
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["schema_version"], 1);
        let markers = json["markers"].as_array().unwrap();
        assert_eq!(markers.len(), 11);
        assert_eq!(markers[0]["name"], "process_spawn_requested");
        assert_eq!(markers[0]["since_first_ns"], 0);
        assert_eq!(markers[8]["name"], "first_animation_frame");
        assert_eq!(markers[9]["name"], "first_invoke_dispatched");
        assert_eq!(markers[10]["name"], "first_invoke_responded");
    }
}
