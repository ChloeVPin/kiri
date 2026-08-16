//! Through-webview IPC benchmark (page → host → page).
//!
//! Distinct from `kiri-core`'s in-process `bulk_bench`. This script is injected
//! into a live WebView after the first animation frame and measures round-trip
//! time for `kiri.ping` (Kiri) or `kiri_echo` (Tauri) at the control-payload
//! sizes from `benchmark/test-vectors.json`, capped so the serialized JSON
//! stays under the 1 MiB control-payload ceiling.

use std::path::PathBuf;

use serde_json::{json, Value};

/// Default iterations per payload size (after warmup).
pub const DEFAULT_RUNS: u32 = 30;
/// Warmup pings discarded from the sample set.
pub const DEFAULT_WARMUP: u32 = 5;
/// Per-call timeout inside the page, milliseconds.
pub const CALL_TIMEOUT_MS: u32 = 30_000;
/// Smoke/watchdog floor when an IPC bench is running.
pub const WATCHDOG_MS: u32 = 180_000;

/// Control-payload content sizes. The last value is 1_048_574 so
/// `JSON.stringify("a".repeat(n))` is exactly 1_048_576 bytes (the default
/// control-payload ceiling). A 1_048_576-character string would serialize to
/// 1_048_578 bytes and be rejected by the validator.
pub const DEFAULT_SIZES: &[usize] = &[0, 64, 1024, 16_384, 262_144, 1_048_574];

/// JavaScript injected into the Kiri WebView. Uses the real
/// `window.ipc.postMessage` / `window.kiri.send` path and waits for
/// `window.kiri.onResponse` (host `evaluate_script` of the wire response).
pub fn kiri_script(runs: u32, warmup: u32, sizes: &[usize]) -> String {
    let sizes_json = serde_json::to_string(sizes).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(function () {{
  if (window.__kiriIpcBenchStarted) return;
  window.__kiriIpcBenchStarted = true;
  var RUNS = {runs};
  var WARMUP = {warmup};
  var SIZES = {sizes_json};
  var TIMEOUT = {timeout};
  window.__kiriIpcSeq = window.__kiriIpcSeq || 1;
  function post(o) {{
    var s = JSON.stringify(o);
    if (window.chrome && window.chrome.webview && window.chrome.webview.postMessage) {{
      window.chrome.webview.postMessage(typeof o === "string" ? s : o);
    }} else if (window.ipc && window.ipc.postMessage) {{
      window.ipc.postMessage(s);
    }}
  }}
  function ping(payload) {{
    return new Promise(function (resolve, reject) {{
      if (!window.kiri || typeof window.kiri.send !== "function") {{
        reject(new Error("window.kiri.send is not available"));
        return;
      }}
      window.kiri.pending = window.kiri.pending || {{}};
      var id = window.__kiriIpcSeq++;
      var timer = setTimeout(function () {{
        delete window.kiri.pending[id];
        reject(new Error("ipc timeout after " + TIMEOUT + "ms"));
      }}, TIMEOUT);
      window.kiri.pending[id] = function (resp) {{
        clearTimeout(timer);
        if (resp && resp.error) {{
          reject(new Error((resp.error && resp.error.message) || "ipc error"));
        }} else {{
          resolve(resp);
        }}
      }};
      var payloadJson = JSON.stringify(payload);
      window.kiri.send({{
        magic: "KRI1",
        version: 1,
        flags: 1,
        command_id: 1,
        request_id: id,
        payload_len: payloadJson.length,
        codec: 1,
        payload: payload
      }});
    }});
  }}
  function makePayload(size) {{
    if (size === 0) return null;
    return "a".repeat(size);
  }}
  async function run() {{
    var results = [];
    for (var s = 0; s < SIZES.length; s++) {{
      var size = SIZES[s];
      var payload = makePayload(size);
      for (var w = 0; w < WARMUP; w++) {{
        await ping(payload);
      }}
      var samples = [];
      var tBatch = performance.now();
      for (var i = 0; i < RUNS; i++) {{
        var t0 = performance.now();
        await ping(payload);
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
    post({{ type: "ipc_bench", target: "kiri-host", results: results }});
  }}
  run().catch(function (err) {{
    post({{ type: "ipc_bench", target: "kiri-host", error: String(err) }});
  }});
}})();"#,
        runs = runs,
        warmup = warmup,
        sizes_json = sizes_json,
        timeout = CALL_TIMEOUT_MS,
    )
}

/// JavaScript injected into the Tauri baseline. Uses
/// `__TAURI_INTERNALS__.invoke('kiri_echo')`, which is Tauri's real command
/// path (not wry `window.ipc`).
pub fn tauri_script(runs: u32, warmup: u32, sizes: &[usize]) -> String {
    let sizes_json = serde_json::to_string(sizes).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(function () {{
  if (window.__kiriIpcBenchStarted) return;
  window.__kiriIpcBenchStarted = true;
  var RUNS = {runs};
  var WARMUP = {warmup};
  var SIZES = {sizes_json};
  var TIMEOUT = {timeout};
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
      for (var w = 0; w < WARMUP; w++) {{
        await invokeEcho(payload);
      }}
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
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
      await window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
        json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", results: results }})
      }});
    }}
  }}
  run().catch(function (err) {{
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
      window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
        json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", error: String(err) }})
      }});
    }}
  }});
}})();"#,
        runs = runs,
        warmup = warmup,
        sizes_json = sizes_json,
        timeout = CALL_TIMEOUT_MS,
    )
}

/// Percentile of a sorted sample set. `p` in 0.0..=1.0.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let k = (sorted.len() - 1) as f64 * p;
    let f = k.floor() as usize;
    let c = (f + 1).min(sorted.len() - 1);
    if f == c {
        sorted[f]
    } else {
        let w = k - f as f64;
        sorted[f] * (1.0 - w) + sorted[c] * w
    }
}

fn summarize(rtt_ms: &[f64]) -> Value {
    let mut sorted = rtt_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len() as f64;
    let mean = if n == 0.0 { 0.0 } else { sorted.iter().sum::<f64>() / n };
    json!({
        "min_ms": sorted.first().copied().unwrap_or(0.0),
        "max_ms": sorted.last().copied().unwrap_or(0.0),
        "mean_ms": mean,
        "median_ms": percentile(&sorted, 0.5),
        "p95_ms": percentile(&sorted, 0.95),
    })
}

fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Attach summaries + metadata and write the through-webview artifact.
pub fn write_result(path: Option<&PathBuf>, raw: &Value) -> Result<(), String> {
    let mut results_out = Vec::new();
    if let Some(arr) = raw.get("results").and_then(|v| v.as_array()) {
        for item in arr {
            let size = item.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
            let rtt: Vec<f64> = item
                .get("rtt_ms")
                .and_then(|v| v.as_array())
                .map(|xs| xs.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            let batch_ms = item.get("batch_ms").and_then(|v| v.as_f64());
            let mean_from_batch = item.get("mean_from_batch_ms").and_then(|v| v.as_f64());
            let mut summary = summarize(&rtt);
            if let Some(obj) = summary.as_object_mut() {
                if let Some(v) = batch_ms {
                    obj.insert("batch_ms".into(), json!(v));
                }
                if let Some(v) = mean_from_batch {
                    obj.insert("mean_from_batch_ms".into(), json!(v));
                }
            }
            results_out.push(json!({
                "size_bytes": size,
                "rtt_ms": rtt,
                "batch_ms": batch_ms,
                "mean_from_batch_ms": mean_from_batch,
                "summary": summary,
            }));
        }
    }

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let artifact = json!({
        "schema_version": 1,
        "name": "through-webview-ipc",
        "target": raw.get("target").cloned().unwrap_or(json!("unknown")),
        "error": raw.get("error").cloned(),
        "commit": git_commit(),
        "created_unix_ns": created,
        "runs": results_out.first().and_then(|r| r.get("rtt_ms")).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(DEFAULT_RUNS as usize),
        "warmup": raw.get("warmup").cloned().unwrap_or(json!(DEFAULT_WARMUP)),
        "sizes_bytes": results_out.iter().filter_map(|r| r.get("size_bytes").cloned()).collect::<Vec<_>>(),
        "results": results_out,
    });

    let text = serde_json::to_string_pretty(&artifact).map_err(|e| e.to_string())?;
    match path {
        Some(path) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, &text).map_err(|e| e.to_string())?;
            eprintln!("[kiri] wrote through-webview ipc artifact {}", path.display());
        }
        None => println!("{text}"),
    }
    if raw.get("error").is_some() {
        return Err(raw.get("error").unwrap().to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::wire::WireRequest;

    #[test]
    fn kiri_script_mentions_real_bridge_and_sizes() {
        let script = kiri_script(7, 2, DEFAULT_SIZES);
        assert!(script.contains("window.kiri.send"));
        assert!(script.contains("command_id: 1"));
        assert!(script.contains("type: \"ipc_bench\""));
        assert!(script.contains("1048574"));
        assert!(!script.contains("bulk_bench"));
    }

    #[test]
    fn tauri_script_uses_invoke_echo() {
        let script = tauri_script(7, 2, DEFAULT_SIZES);
        assert!(script.contains("kiri_echo"));
        assert!(script.contains("kiri_ipc_bench_done"));
        assert!(script.contains("__TAURI_INTERNALS__"));
    }

    #[test]
    fn js_shaped_ping_deserializes_as_wire_request() {
        let payload = serde_json::Value::String("a".repeat(64));
        let payload_len = serde_json::to_vec(&payload).unwrap().len() as u32;
        let req: WireRequest = serde_json::from_value(serde_json::json!({
            "magic": "KRI1",
            "version": 1,
            "flags": 1,
            "command_id": 1,
            "request_id": 42,
            "payload_len": payload_len,
            "codec": 1,
            "payload": payload,
        }))
        .expect("js-shaped ping must deserialize");
        assert_eq!(req.command_id, 1);
        assert_eq!(req.request_id, 42);
        assert_eq!(req.payload_len, payload_len);
    }

    #[test]
    fn one_mib_string_exceeds_control_ceiling_last_size_does_not() {
        let too_big = serde_json::to_vec(&serde_json::Value::String("a".repeat(1_048_576)))
            .unwrap()
            .len();
        let last = serde_json::to_vec(&serde_json::Value::String(
            "a".repeat(*DEFAULT_SIZES.last().unwrap()),
        ))
        .unwrap()
        .len();
        assert!(too_big as u32 > kiri_core::constants::DEFAULT_MAX_CONTROL_PAYLOAD_BYTES);
        assert!(last as u32 <= kiri_core::constants::DEFAULT_MAX_CONTROL_PAYLOAD_BYTES);
    }
}
