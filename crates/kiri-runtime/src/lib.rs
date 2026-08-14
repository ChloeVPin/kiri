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

pub mod markers;
pub mod output;

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
