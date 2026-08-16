//! Kiri runtime: the native host.
//!
//! The crate exposes a platform-neutral API. The actual WebView backend is
//! selected automatically: on Windows the direct Win32 + WebView2 host is
//! used; on macOS and Linux a wry/tao backend hosts the same frontend and
//! records the same startup markers, so the benchmark compares like for like.
//!
//! Both backends obey the shared startup contract in
//! `docs/research/markers-schema.md`: record the nine markers on a monotonic
//! clock and exit 0 after `first_animation_frame` (smoke mode) or 2 on the
//! ready watchdog.

pub mod assets;
pub mod autostart_ctl;
pub mod clipboard_ctl;
pub mod deeplink_ctl;
pub mod dialog_ctl;
pub mod ipc_bench;
pub mod markers;
pub mod notification_ctl;
pub mod opener_ctl;
pub mod output;
pub mod plugins;
pub mod shell_ctl;
pub mod shortcut_ctl;
pub mod sidecar_ctl;
pub mod store_ctl;
pub mod tray_ctl;
pub mod window_ctl;
pub mod window_state_ctl;

#[cfg(not(target_os = "windows"))]
mod host_cross;
#[cfg(target_os = "windows")]
mod host_windows;

use std::path::PathBuf;

pub use markers::{Marker, MarkerRecord, StartupMarkers, StartupResult};
pub use output::write_startup_result;

/// Version of the WebView2 COM bindings the Windows direct host is verified
/// against.
pub const WEBVIEW2_COM_BINDINGS: &str = "0.39.1";

/// Options for one host session, supplied by the launcher.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// Directory served as the application origin. The frontend
    /// (`index.html`) lives here. On the cross backend it is read at runtime;
    /// on Windows it is mapped by WebView2's virtual host mapping.
    pub frontend_dir: Option<PathBuf>,
    /// Optional path to also write the startup result JSON to (in addition to
    /// stdout). The cross backend needs this because its event loop does not
    /// return; the Windows backend uses it as well for parity.
    pub markers_out: Option<PathBuf>,
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Smoke mode: exit shortly after the first animation frame.
    pub smoke: bool,
    /// Grace period after the first animation frame before posting quit
    /// (smoke mode only).
    pub exit_after_ready_ms: u32,
    /// Hard timeout for reaching ready state (smoke/stress runs; CI cannot
    /// hang). 0 disables the watchdog.
    pub watchdog_ms: u32,
    /// After the first animation frame, inject a through-webview ping/echo
    /// bench and exit once the page posts the result. Distinct from `--smoke`.
    pub ipc_bench: bool,
    /// Iterations per payload size (after warmup) when `ipc_bench` is set.
    pub ipc_bench_runs: u32,
    /// Optional path for the through-webview IPC artifact.
    pub ipc_bench_out: Option<PathBuf>,
    /// Payload content sizes for `--ipc-bench`. Empty means DEFAULT_SIZES.
    pub ipc_bench_sizes: Vec<usize>,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            frontend_dir: None,
            markers_out: None,
            title: "Kiri host".into(),
            width: 1024,
            height: 768,
            smoke: false,
            exit_after_ready_ms: 250,
            watchdog_ms: 30_000,
            ipc_bench: false,
            ipc_bench_runs: crate::ipc_bench::DEFAULT_RUNS,
            ipc_bench_out: None,
            ipc_bench_sizes: crate::ipc_bench::DEFAULT_SIZES.to_vec(),
        }
    }
}

/// Which native backend hosts the WebView.
///
/// Reserved for a future `--backend` selector; the host currently resolves
/// automatically by target OS. Not yet wired into the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Backend {
    /// Pick the platform-native backend (direct host on Windows, wry/tao elsewhere).
    Auto,
    /// Direct Win32 + WebView2 host (Windows only).
    Windows,
    /// Cross-platform wry/tao backend.
    Cross,
}

/// Resolve the backend to use for this platform.
///
/// `Auto` maps to the direct Win32 + WebView2 host on Windows and to the
/// cross-platform wry/tao backend everywhere else.
#[allow(dead_code)]
pub fn resolve_backend(requested: Backend) -> Backend {
    match requested {
        Backend::Auto => {
            #[cfg(target_os = "windows")]
            {
                Backend::Windows
            }
            #[cfg(not(target_os = "windows"))]
            {
                Backend::Cross
            }
        }
        other => other,
    }
}

/// Run one host session with the platform-native backend and record the
/// startup markers. Returns the recorded markers.
///
/// Use [`run_session`] to also emit the startup result and obtain a process
/// exit code.
pub fn run(options: &HostOptions) -> Result<StartupMarkers, i32> {
    #[cfg(target_os = "windows")]
    {
        host_windows::WindowsHost::run(options)
    }
    #[cfg(not(target_os = "windows"))]
    {
        host_cross::CrossHost::run(options)
    }
}

/// Build host options from the `kiri-host` CLI surface.
#[allow(clippy::too_many_arguments)]
pub fn host_options_from_args(
    frontend_dir: Option<PathBuf>,
    markers_out: Option<PathBuf>,
    title: String,
    width: u32,
    height: u32,
    smoke: bool,
    exit_after_ready_ms: u32,
    watchdog_ms: u32,
) -> HostOptions {
    HostOptions {
        frontend_dir,
        markers_out,
        title,
        width,
        height,
        smoke,
        exit_after_ready_ms,
        watchdog_ms,
        ipc_bench: false,
        ipc_bench_runs: crate::ipc_bench::DEFAULT_RUNS,
        ipc_bench_out: None,
        ipc_bench_sizes: crate::ipc_bench::DEFAULT_SIZES.to_vec(),
    }
}

/// Run one host session and emit the startup result JSON (to stdout, or to
/// `options.markers_out` when set). Returns a process exit code.
pub fn run_session(options: &HostOptions) -> i32 {
    match run(options) {
        Ok(markers) => {
            write_startup_result(&markers, options.markers_out.as_ref());
            0
        }
        Err(code) => code,
    }
}
/// The markers a smoke run must have recorded before the watchdog fires.
pub fn require_smoke_markers(markers: &StartupMarkers) -> Result<(), String> {
    for required in
        [Marker::BridgeReady, Marker::WebViewReady, Marker::DomReady, Marker::FirstAnimationFrame]
    {
        if !markers.has(required) {
            return Err(format!("missing marker {}", required.name()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod control_plane_tests {
    use kiri_core::caller::CallerRegistry;
    use kiri_core::capabilities::CapabilityBits;
    use kiri_core::dispatch::is_pong;
    use kiri_core::trace::NoopTraceSink;
    use kiri_core::wire::WireRequest;
    use serde_json::json;

    /// Mirrors exactly how the bridge handler (`cmd` message) parses and
    /// dispatches a control request, so the wiring logic is covered without a
    /// live WebView (T003 end-to-end path). The request is built the same way
    /// the real bridge does: a properly framed `WireRequest` wrapped in a
    /// `{ "type": "cmd", "request": ... }` envelope.
    fn dispatch_from_bridge_message(request: WireRequest) -> kiri_core::wire::WireResponse {
        let msg = json!({ "type": "cmd", "request": request });
        let req_val = msg.get("request").unwrap();
        let request: WireRequest =
            serde_json::from_value(req_val.clone()).expect("bridge request must deserialize");
        let mut registry = CallerRegistry::new();
        let caller = registry.register();
        let mut caps = CapabilityBits::empty();
        caps.set(kiri_core::dispatch::capability_bit::PING);
        let events = kiri_core::platform::EventBus::new();
        let diagnostics = kiri_core::diagnostics::Diagnostics::new();
        let resources = std::sync::Arc::new(std::sync::Mutex::new(
            kiri_core::resources::ResourceTable::<()>::new(),
        ));
        let router = crate::plugins::PluginHost::build_router_with_plugins(
            &diagnostics,
            &resources,
            caller,
            &crate::plugins::PluginManifest::empty(),
            &crate::plugins::PluginRegistry::empty(),
        )
        .with_platform(events);
        let mut sink = NoopTraceSink;
        router.dispatch(caller, &caps, &request, &mut sink)
    }

    #[test]
    fn bridge_ping_message_roundtrips_to_pong() {
        let request = kiri_core::dispatch::ping_request(11, json!({ "x": 1 }));
        let resp = dispatch_from_bridge_message(request);
        assert!(is_pong(&resp, 11));
    }

    #[test]
    fn bridge_unknown_command_rejected() {
        let mut request = kiri_core::dispatch::ping_request(2, json!(null));
        request.command_id = 9999;
        let resp = dispatch_from_bridge_message(request);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, kiri_core::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn bridge_rejects_non_app_origin_message() {
        // The cross backend gates every IPC message on the calling document
        // URL (T004): only the application origin may drive the control plane.
        // A message whose document URL is a remote origin must never reach
        // dispatch, so it cannot be turned into a pong.
        assert!(!kiri_core::security::is_app_origin("https://evil.example.com/page"));
        assert!(!kiri_core::security::is_app_origin("null"));
        // The local application document URL is the only trusted origin.
        assert!(kiri_core::security::is_app_origin("kiri://localhost/index.html"));
    }

    #[test]
    fn bridge_malformed_message_rejected() {
        let mut request = kiri_core::dispatch::ping_request(3, json!(null));
        request.magic = *b"BAD!";
        let resp = dispatch_from_bridge_message(request);
        // Bad magic is rejected by the validation pipeline, so it is never a
        // successful pong.
        assert!(!is_pong(&resp, 3));
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, kiri_core::error::ErrorCode::ProtocolError);
    }
}
