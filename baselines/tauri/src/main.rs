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

/// Mirror of the wry/tao baseline bridge, but hardened for Tauri: Tauri's
/// `__TAURI_INTERNALS__.invoke` is not present at document-start, so the
/// one-shot guard used by the wry/tao bridge silently drops the marker when
/// internals are not yet ready. This variant waits for a ready channel
/// (`window.ipc.postMessage` or Tauri internals) and then flushes both
/// ready-phase markers, retrying on an interval until the channel exists.
const BRIDGE_SCRIPT: &str = r#"
    (function () {
      function post(o) {
        var s = JSON.stringify(o);
        if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
          window.__TAURI_INTERNALS__.invoke('kiri_marker', { json: s });
        } else if (window.ipc && window.ipc.postMessage) {
          window.ipc.postMessage(s);
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
const DEFAULT_WATCHDOG_MS: u128 = 30_000;

fn ipc_bench_enabled() -> bool {
    matches!(std::env::var("KIRI_IPC_BENCH").as_deref(), Ok("1") | Ok("true") | Ok("yes"))
}

fn ipc_bench_runs() -> u32 {
    std::env::var("KIRI_IPC_BENCH_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(30)
}

fn ipc_bench_warmup() -> u32 {
    std::env::var("KIRI_IPC_BENCH_WARMUP").ok().and_then(|v| v.parse().ok()).unwrap_or(5)
}

fn ipc_bench_out() -> std::path::PathBuf {
    std::env::var("KIRI_IPC_BENCH_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("artifacts/ipc-tauri.json"))
}

fn watchdog_ms() -> u128 {
    if ipc_bench_enabled() {
        180_000
    } else {
        DEFAULT_WATCHDOG_MS
    }
}

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

struct IpcBenchState {
    injected: Mutex<bool>,
    result: Mutex<Option<String>>,
}

type SharedBench = Arc<IpcBenchState>;

/// Through-webview echo: the Tauri-side counterpart of kiri.ping.
#[tauri::command]
fn kiri_echo(payload: String) -> String {
    payload
}

#[tauri::command]
fn kiri_ipc_bench_done(state: tauri::State<'_, SharedBench>, json: String) {
    *state.result.lock().unwrap() = Some(json);
}

#[tauri::command]
fn kiri_marker(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MarkerState>,
    bench: tauri::State<'_, SharedBench>,
    json: String,
) {
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
            if ipc_bench_enabled() {
                let mut injected = bench.injected.lock().unwrap();
                if !*injected {
                    *injected = true;
                    drop(injected);
                    let script = tauri_ipc_bench_script(ipc_bench_runs(), ipc_bench_warmup());
                    let _ = window.eval(&script);
                }
            }
        }
        _ => {}
    }
}

fn ipc_bench_sizes() -> Vec<usize> {
    if let Ok(raw) = std::env::var("KIRI_IPC_BENCH_SIZES") {
        let parsed: Vec<usize> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    vec![0, 64, 1024, 16_384, 262_144, 1_048_574]
}

fn tauri_ipc_bench_script(runs: u32, warmup: u32) -> String {
    // Keep default sizes in lockstep with kiri-runtime::ipc_bench::DEFAULT_SIZES.
    let sizes = ipc_bench_sizes();
    let sizes_json = serde_json::to_string(&sizes).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(function () {{
  if (window.__kiriIpcBenchStarted) return;
  window.__kiriIpcBenchStarted = true;
  var RUNS = {runs};
  var WARMUP = {warmup};
  var SIZES = {sizes_json};
  function invokeEcho(payload) {{
    if (!window.__TAURI_INTERNALS__ || typeof window.__TAURI_INTERNALS__.invoke !== "function") {{
      return Promise.reject(new Error("Tauri invoke is not available"));
    }}
    return window.__TAURI_INTERNALS__.invoke("kiri_echo", {{ payload: payload }});
  }}
  function makePayload(size) {{
    if (size === 0) return "";
    return "a".repeat(size);
  }}
  async function run() {{
    var results = [];
    for (var s = 0; s < SIZES.length; s++) {{
      var size = SIZES[s];
      var payload = makePayload(size);
      for (var w = 0; w < WARMUP; w++) {{ await invokeEcho(payload); }}
      var samples = [];
      var tBatch = performance.now();
      for (var i = 0; i < RUNS; i++) {{
        var t0 = performance.now();
        await invokeEcho(payload);
        samples.push(performance.now() - t0);
      }}
      var batchMs = performance.now() - tBatch;
      results.push({{
        size_bytes: size,
        rtt_ms: samples,
        batch_ms: batchMs,
        mean_from_batch_ms: batchMs / RUNS
      }});
    }}
    await window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
      json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", results: results }})
    }});
  }}
  run().catch(function (err) {{
    window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
      json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", error: String(err) }})
    }});
  }});
}})();"#,
        runs = runs,
        warmup = warmup,
        sizes_json = sizes_json,
    )
}

fn write_ipc_artifact(raw: &str) {
    let path = ipc_bench_out();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| {
        serde_json::json!({ "type": "ipc_bench", "target": "tauri-baseline", "error": raw })
    });
    let mut results_out = Vec::new();
    if let Some(arr) = parsed.get("results").and_then(|v| v.as_array()) {
        for item in arr {
            let rtt: Vec<f64> = item
                .get("rtt_ms")
                .and_then(|v| v.as_array())
                .map(|xs| xs.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            let mut sorted = rtt.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = sorted.len() as f64;
            let mean = if n == 0.0 { 0.0 } else { sorted.iter().sum::<f64>() / n };
            let median = if sorted.is_empty() {
                0.0
            } else {
                sorted[sorted.len() / 2]
            };
            let batch_ms = item.get("batch_ms").and_then(|v| v.as_f64());
            let mean_from_batch = item.get("mean_from_batch_ms").and_then(|v| v.as_f64());
            results_out.push(serde_json::json!({
                "size_bytes": item.get("size_bytes").cloned().unwrap_or(Value::from(0)),
                "rtt_ms": rtt,
                "batch_ms": batch_ms,
                "mean_from_batch_ms": mean_from_batch,
                "summary": {
                    "min_ms": sorted.first().copied().unwrap_or(0.0),
                    "max_ms": sorted.last().copied().unwrap_or(0.0),
                    "mean_ms": mean,
                    "median_ms": median,
                    "batch_ms": batch_ms,
                    "mean_from_batch_ms": mean_from_batch,
                }
            }));
        }
    }
    let artifact = serde_json::json!({
        "schema_version": 1,
        "name": "through-webview-ipc",
        "target": "tauri-baseline",
        "error": parsed.get("error").cloned(),
        "results": results_out,
    });
    if let Ok(text) = serde_json::to_string_pretty(&artifact) {
        let _ = std::fs::write(&path, text);
        eprintln!("[tauri-baseline] wrote through-webview ipc artifact {}", path.display());
    }
}

fn main() {
    let mut markers = Markers::default();
    // Mirror the wry/tao baseline clock convention: t0 is locked on the first
    // real now_ns() sample (NativeEntry), so every later marker is measured from
    // process start. Hardcoding both early markers to 0 corrupts the t0 reference
    // and collapses all early phases to ~0 (Q-003 fragility).
    markers.record(Marker::ProcessSpawnRequested, 0);
    markers.record(Marker::NativeEntry, now_ns());
    let shared = Arc::new(Mutex::new(markers));

    let bench = Arc::new(IpcBenchState { injected: Mutex::new(false), result: Mutex::new(None) });

    tauri::Builder::default()
        .manage(MarkerState(shared.clone()))
        .manage(bench.clone())
        .invoke_handler(tauri::generate_handler![kiri_marker, kiri_echo, kiri_ipc_bench_done])
        .setup({
            let shared = shared.clone();
            let bench = bench.clone();
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
                let _main_window =
                    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                        .title("Tauri baseline")
                        .inner_size(1024.0, 768.0)
                        .initialization_script(BRIDGE_SCRIPT)
                        .on_page_load(on_load)
                        .build()?;
                shared.lock().unwrap().record(Marker::BridgeReady, now_ns());

                if SMOKE {
                    let shared = shared.clone();
                    let bench = bench.clone();
                    let handle = app.handle().clone();
                    let want_ipc = ipc_bench_enabled();
                    let wd = watchdog_ms();
                    std::thread::spawn(move || {
                        let t0 = Instant::now();
                        loop {
                            let has_frame = shared.lock().unwrap().has(Marker::FirstAnimationFrame);
                            let elapsed = t0.elapsed().as_millis();
                            if want_ipc {
                                if let Some(raw) = bench.result.lock().unwrap().clone() {
                                    write_ipc_artifact(&raw);
                                    println!("{}", shared.lock().unwrap().result_json());
                                    handle.exit(0);
                                    return;
                                }
                                if elapsed > wd {
                                    eprintln!("[tauri-baseline] watchdog: ipc bench not finished");
                                    std::process::exit(2);
                                }
                            } else if has_frame {
                                if elapsed > EXIT_AFTER_READY_MS {
                                    println!("{}", shared.lock().unwrap().result_json());
                                    handle.exit(0);
                                    return;
                                }
                            } else if elapsed > wd {
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
