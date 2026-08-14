//! Blank Tauri baseline app (docs/12-benchmarks.md).
//!
//! Runs the same blank frontend as the direct Kiri host and emits the same
//! startup marker schema. Standalone on purpose: baselines must not depend
//! on kiri-core.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use tauri::webview::PageLoadEvent;
use tauri::{RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

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
        self.recorded.insert(marker, (now_ns, now_ns.saturating_sub(t0)));
    }

    fn has(&self, marker: Marker) -> bool {
        self.recorded.contains_key(&marker)
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

/// Shared marker state; recorded from the IPC command thread and read by the
/// smoke/watchdog poll thread.
struct MarkerState(Arc<Mutex<Markers>>);

#[tauri::command]
fn kiri_marker(state: tauri::State<'_, MarkerState>, json: String) {
    let mut markers = state.0.lock().unwrap();
    let Ok(value) = serde_json::from_str::<Value>(&json) else {
        return;
    };
    match value.get("phase").and_then(|p| p.as_str()) {
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

fn main() {
    let mut markers = Markers::default();
    markers.record(Marker::ProcessSpawnRequested, 0);
    markers.record(Marker::NativeEntry, 0);
    let shared = Arc::new(Mutex::new(markers));

    tauri::Builder::default()
        .manage(MarkerState(shared.clone()))
        .invoke_handler(tauri::generate_handler![kiri_marker])
        .setup({
            let shared = shared.clone();
            move |app| {
                shared.lock().unwrap().record(Marker::PlatformInit, now_ns());
                shared.lock().unwrap().record(Marker::WebViewCreationRequested, now_ns());

                let on_load = {
                    let shared = shared.clone();
                    move |_window: tauri::WebviewWindow,
                          payload: tauri::webview::PageLoadPayload| {
                        if matches!(payload.event(), PageLoadEvent::Finished) {
                            shared.lock().unwrap().record(Marker::WebViewReady, now_ns());
                        }
                    }
                };
                let _window =
                    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                        .title("Tauri baseline")
                        .inner_size(1024.0, 768.0)
                        .on_page_load(on_load)
                        .build()?;
                shared.lock().unwrap().record(Marker::BridgeReady, now_ns());

                if SMOKE {
                    let shared = shared.clone();
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        let t0 = Instant::now();
                        loop {
                            let has_frame = shared.lock().unwrap().has(Marker::FirstAnimationFrame);
                            let elapsed = t0.elapsed().as_millis();
                            if has_frame {
                                if elapsed > EXIT_AFTER_READY_MS {
                                    println!("{}", shared.lock().unwrap().result_json());
                                    handle.exit(0);
                                    return;
                                }
                            } else if elapsed > WATCHDOG_MS {
                                eprintln!("[tauri-baseline] watchdog: ready not reached");
                                std::process::exit(2);
                            }
                            std::thread::sleep(std::time::Duration::from_millis(25));
                        }
                    });
                }
                Ok(())
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| match event {
            RunEvent::WindowEvent { event: WindowEvent::CloseRequested { .. }, .. } => {}
            _ => {}
        });
}
