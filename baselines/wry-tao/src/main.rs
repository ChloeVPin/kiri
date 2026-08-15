//! Minimal Wry/Tao baseline host (docs/12-benchmarks.md).
//!
//! Runs the same blank frontend as the direct Kiri host and emits the same
//! startup marker schema, so the benchmark harness can compare apples to
//! apples. Standalone on purpose: baselines must not depend on kiri-core.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebViewBuilder};

const FRONTEND_HTML: &str = include_str!("../../../examples/blank/index.html");
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

const SMOKE: bool = true;
const EXIT_AFTER_READY_MS: u128 = 250;
const WATCHDOG_MS: u128 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Marker {
    ProcessSpawnRequested,
    NativeEntry,
    PlatformInit,
    WebViewCreationRequested,
    WebViewReady,
    BridgeReady,
    DomReady,
    AppReady,
    FirstAnimationFrame,
}

impl Marker {
    const ALL: [Marker; 9] = [
        Marker::ProcessSpawnRequested,
        Marker::NativeEntry,
        Marker::PlatformInit,
        Marker::WebViewCreationRequested,
        Marker::WebViewReady,
        Marker::BridgeReady,
        Marker::DomReady,
        Marker::AppReady,
        Marker::FirstAnimationFrame,
    ];

    const fn name(self) -> &'static str {
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
        }
    }
}

#[derive(Default)]
struct Markers {
    t0_ns: Option<u64>,
    recorded: BTreeMap<Marker, (u64, u64)>,
}

impl Markers {
    fn record(&mut self, marker: Marker, now_ns: u64) {
        let t0 = *self.t0_ns.get_or_insert(now_ns);
        self.recorded.insert(marker, (now_ns, now_ns - t0));
    }

    fn result_json(&self) -> String {
        let markers: Vec<Value> = Marker::ALL
            .iter()
            .filter_map(|m| {
                self.recorded.get(m).map(|(ts, since)| {
                    serde_json::json!({
                        "name": m.name(),
                        "timestamp_ns": ts,
                        "since_first_ns": since,
                    })
                })
            })
            .collect();
        serde_json::json!({ "schema_version": 1, "markers": markers }).to_string()
    }
}

fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static T0: OnceLock<Instant> = OnceLock::new();
    let t0 = *T0.get_or_init(Instant::now);
    Instant::now().duration_since(t0).as_nanos() as u64
}

fn main() {
    let markers = Rc::new(RefCell::new(Markers::default()));
    markers.borrow_mut().record(Marker::ProcessSpawnRequested, 0);
    markers.borrow_mut().record(Marker::NativeEntry, now_ns());

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Wry/Tao baseline")
        .with_inner_size(tao::dpi::LogicalSize::new(1024.0, 768.0))
        .build(&event_loop)
        .expect("window creation failed");
    markers.borrow_mut().record(Marker::PlatformInit, now_ns());

    markers.borrow_mut().record(Marker::WebViewCreationRequested, now_ns());
    let webview = WebViewBuilder::new()
        .with_custom_protocol("wry".into(), |_id, _request| {
            wry::http::Response::builder()
                .header("Content-Type", "text/html")
                .body(std::borrow::Cow::Borrowed(FRONTEND_HTML.as_bytes()))
                .unwrap()
        })
        .with_url("wry://localhost/index.html")
        .with_initialization_script(BRIDGE_SCRIPT)
        .with_on_page_load_handler({
            let markers = markers.clone();
            move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    markers.borrow_mut().record(Marker::WebViewReady, now_ns());
                }
            }
        })
        .with_ipc_handler({
            let markers = markers.clone();
            move |msg| {
                let Ok(value) = serde_json::from_str::<Value>(msg.body()) else {
                    return;
                };
                let phase = value.get("phase").and_then(|p| p.as_str());
                let mut markers = markers.borrow_mut();
                match phase {
                    Some("dom") => {
                        markers.record(Marker::DomReady, now_ns());
                        markers.record(Marker::AppReady, now_ns());
                    }
                    Some("frame") => {
                        markers.record(Marker::FirstAnimationFrame, now_ns());
                    }
                    _ => {}
                }
            }
        })
        .build(&window)
        .expect("webview build failed");
    markers.borrow_mut().record(Marker::BridgeReady, now_ns());

    let t0 = Instant::now();
    let mut smoke_armed = false;
    let mut frame_at: Option<Instant> = None;

    event_loop.run(move |event, _, control_flow| {
        // Wake periodically so the smoke watchdog and exit timer are checked
        // even when WebView2 has no native event to dispatch on Windows.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(25));
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // Keep the WebView alive for the whole loop; it is destroyed
                // when main returns.
                let _ = &webview;
                *control_flow = ControlFlow::Exit;
            }
            // wry auto-resizes the webview on Windows when the parent is
            // resized (see WebViewBuilder::build docs).
            _ => {}
        }

        if SMOKE {
            let elapsed = t0.elapsed().as_millis();
            if elapsed > WATCHDOG_MS {
                eprintln!("[wry-tao-baseline] watchdog: ready not reached");
                std::process::exit(2);
            }
            let has_frame = markers.borrow().recorded.contains_key(&Marker::FirstAnimationFrame);
            if has_frame && !smoke_armed {
                smoke_armed = true;
                frame_at = Some(Instant::now());
            }
            if let Some(frame) = frame_at {
                if frame.elapsed().as_millis() > EXIT_AFTER_READY_MS {
                    println!("{}", markers.borrow().result_json());
                    *control_flow = ControlFlow::Exit;
                }
            }
        }
    });
}
