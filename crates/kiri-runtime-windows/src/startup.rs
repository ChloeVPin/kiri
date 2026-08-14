//! Session orchestration shared by the `kiri-host` and `kiri-host-stress`
//! binaries: run one host session and emit the startup result.

#![cfg(target_os = "windows")]

use crate::host::{write_startup_result, HostOptions, WindowsHost};
use crate::markers::StartupMarkers;

/// Build host options from the `kiri-host` CLI surface (WP1).
pub fn host_options_from_args(
    frontend_dir: Option<std::path::PathBuf>,
    title: String,
    width: u32,
    height: u32,
    smoke: bool,
    exit_after_ready_ms: u32,
    watchdog_ms: u32,
) -> HostOptions {
    HostOptions { frontend_dir, title, width, height, smoke, exit_after_ready_ms, watchdog_ms }
}

/// Run one host session and emit the startup result JSON (to stdout, or to
/// `--markers-out` when given). Returns an exit code suitable for the
/// process.
pub fn run_session(options: &HostOptions, markers_out: Option<&std::path::PathBuf>) -> i32 {
    match WindowsHost::run(options) {
        Ok(markers) => {
            write_startup_result(&markers, markers_out);
            0
        }
        Err(err) => {
            eprintln!("[kiri] host error: {err}");
            1
        }
    }
}

/// The markers a smoke run must have recorded before the watchdog fires.
/// `process_spawn_requested` is recorded by the host process itself in this
/// slice (the external launcher records it in the full pipeline).
pub fn require_smoke_markers(markers: &StartupMarkers) -> Result<(), String> {
    use crate::markers::Marker;
    for required in
        [Marker::BridgeReady, Marker::WebViewReady, Marker::DomReady, Marker::FirstAnimationFrame]
    {
        if !markers.has(required) {
            return Err(format!("missing marker {}", required.name()));
        }
    }
    Ok(())
}
