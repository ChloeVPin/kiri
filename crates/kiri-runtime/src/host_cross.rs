//! Cross-platform host backend (macOS, Linux) built on `wry` + `tao`.
//!
//! This backend hosts the same blank frontend and records the same nine
//! startup markers on a monotonic clock, so the benchmark compares like for
//! like across the direct host and the wry/tao baseline. It shares the smoke
//! contract: exit 0 after `first_animation_frame` plus `--exit-after-ready-ms`,
//! exit 2 on watchdog.
//!
//! The WebView is created on the main thread, which is also where wry/tao
//! dispatch their events. The bridge script installed at document start posts
//! ready-phase markers over `window.chrome.webview.postMessage` (WebView2),
//! `window.ipc.postMessage` (wry), or the Tauri internals; on wry the
//! `with_ipc_handler` receives those messages and drives the markers.

#![cfg(not(target_os = "windows"))]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::http::Response as WryResponse;
use wry::{PageLoadEvent, WebViewBuilder};

use crate::markers::{Marker, StartupMarkers};
use crate::output::write_startup_result;
use crate::HostOptions;

/// The frontend is served over a custom `kiri://localhost` protocol so the
/// page loads from a stable application origin. The served file is read from
/// `HostOptions.frontend_dir` (`index.html`) at runtime; the directory is
/// required, matching the Windows direct host's virtual-host mapping.
fn frontend_index_bytes(options: &HostOptions) -> std::borrow::Cow<'static, [u8]> {
    if let Some(dir) = options.frontend_dir.as_ref() {
        let path = dir.join("index.html");
        if let Ok(bytes) = std::fs::read(&path) {
            return std::borrow::Cow::Owned(bytes);
        }
        eprintln!(
            "[kiri] frontend index.html not found at {}; serving embedded blank page",
            path.display()
        );
    }
    const FALLBACK: &str = include_str!("../../../examples/blank/index.html");
    std::borrow::Cow::Borrowed(FALLBACK.as_bytes())
}

/// Bridge script injected at document start. It only fires on the
/// application origin and uses whichever native bridge the host exposes.
const BRIDGE_SCRIPT: &str = r#"
    (function () {
      if (window.kiri) { return; }
      function post(o) {
        var s = JSON.stringify(o);
        if (window.chrome && window.chrome.webview && window.chrome.webview.postMessage) {
          window.chrome.webview.postMessage(s);
        } else if (window.ipc && window.ipc.postMessage) {
          window.ipc.postMessage(s);
        } else if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
          window.__TAURI_INTERNALS__.invoke('kiri_marker', { json: s });
        }
      }
      window.addEventListener('DOMContentLoaded', function () {
        post({ type: 'ready', phase: 'dom' });
      });
      requestAnimationFrame(function () {
        post({ type: 'ready', phase: 'frame' });
      });
    })();
"#;

/// The cross-platform host: window, WebView, event loop, and marker capture.
pub struct CrossHost;

impl CrossHost {
    /// Run one full session and record the startup markers.
    ///
    /// On macOS/Linux `tao::EventLoop::run` does not return; the host emits
    /// the startup result and exits the process directly (matching the wry/tao
    /// baseline). The function's return type is preserved for the shared
    /// facade, but in practice the smoke/watchdog paths terminate via
    /// `std::process::exit`.
    pub fn run(options: &HostOptions) -> Result<StartupMarkers, i32> {
        run_inner(options.clone())
    }
}

/// Monotonic clock in nanoseconds, relative to process start.
fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static T0: OnceLock<Instant> = OnceLock::new();
    let t0 = *T0.get_or_init(Instant::now);
    Instant::now().duration_since(t0).as_nanos() as u64
}

/// Record one marker into the shared map.
fn record(markers: &Rc<RefCell<StartupMarkers>>, marker: Marker) {
    markers.borrow_mut().record(marker, now_ns());
}

fn run_inner(options: HostOptions) -> Result<StartupMarkers, i32> {
    let markers = Rc::new(RefCell::new(StartupMarkers::new()));
    record(&markers, Marker::ProcessSpawnRequested);
    record(&markers, Marker::NativeEntry);

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(options.title.clone())
        .with_inner_size(tao::dpi::LogicalSize::new(options.width as f64, options.height as f64))
        .build(&event_loop)
        .map_err(|e| {
            eprintln!("[kiri] window creation failed: {e}");
            1
        })?;
    record(&markers, Marker::PlatformInit);

    record(&markers, Marker::WebViewCreationRequested);

    let smoke = options.smoke;
    let markers_out = options.markers_out.clone();
    let exit_after_ready_ms = options.exit_after_ready_ms as u128;
    let watchdog_ms = options.watchdog_ms as u128;

    let webview = WebViewBuilder::new()
        .with_custom_protocol("kiri".into(), {
            let options = options.clone();
            move |_id, _request| {
                WryResponse::builder()
                    .header("Content-Type", "text/html")
                    .body(frontend_index_bytes(&options))
                    .unwrap()
            }
        })
        .with_url("kiri://localhost/index.html")
        .with_initialization_script(BRIDGE_SCRIPT)
        .with_on_page_load_handler({
            let markers = markers.clone();
            move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    record(&markers, Marker::WebViewReady);
                }
            }
        })
        .with_ipc_handler({
            let markers = markers.clone();
            move |msg| {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(msg.body()) else {
                    return;
                };
                match value.get("phase").and_then(|p| p.as_str()) {
                    Some("dom") => {
                        // Recover webview_ready if page-load lagged (parity
                        // with the Windows host's dom-message fallback).
                        if !markers.borrow().has(Marker::WebViewReady) {
                            record(&markers, Marker::WebViewReady);
                        }
                        record(&markers, Marker::DomReady);
                        record(&markers, Marker::AppReady);
                    }
                    Some("frame") => {
                        record(&markers, Marker::FirstAnimationFrame);
                    }
                    _ => {}
                }
            }
        })
        .build(&window)
        .map_err(|e| {
            eprintln!("[kiri] webview build failed: {e}");
            1
        })?;
    record(&markers, Marker::BridgeReady);

    let t0 = Instant::now();
    let mut smoke_armed = false;
    let mut frame_at: Option<Instant> = None;

    // `webview` is moved into the event-loop closure so it stays alive for the
    // lifetime of the session (it owns the native view and IPC handlers).
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *control_flow = ControlFlow::Exit;
        } else {
            // Keep the webview alive for the whole loop by holding a reference
            // each iteration; it owns the native view and IPC handlers.
            let _ = &webview;
            let _ = event;
        }

        if smoke {
            let elapsed = t0.elapsed().as_millis();
            if elapsed > watchdog_ms {
                eprintln!("[kiri] watchdog: ready state not reached within the watchdog");
                std::process::exit(2);
            }
            let has_frame = markers.borrow().has(Marker::FirstAnimationFrame);
            if has_frame && !smoke_armed {
                smoke_armed = true;
                frame_at = Some(Instant::now());
            }
            if let Some(frame) = frame_at {
                if frame.elapsed().as_millis() > exit_after_ready_ms {
                    // Emit the startup result (stdout + optional file) and exit
                    // cleanly. tao's macOS loop does not return, so this is the
                    // single emission point for the cross backend.
                    let recorded = markers.borrow().clone_markers();
                    write_startup_result(&recorded, markers_out.as_ref());
                    std::process::exit(0);
                }
            }
        }
    });
}
