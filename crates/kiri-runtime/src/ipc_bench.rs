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
/// Bounded parallel requests per concurrent benchmark batch.
pub const DEFAULT_CONCURRENCY: u32 = 8;
/// Per-call timeout inside the page, milliseconds.
pub const CALL_TIMEOUT_MS: u32 = 30_000;
/// Smoke/watchdog floor when an IPC bench is running.
pub const WATCHDOG_MS: u32 = 180_000;

/// Control-payload content sizes. The last value is 1_048_574 so
/// `JSON.stringify("a".repeat(n))` is exactly 1_048_576 bytes (the default
/// control-payload ceiling). A 1_048_576-character string would serialize to
/// 1_048_578 bytes and be rejected by the validator.
pub const DEFAULT_SIZES: &[usize] = &[0, 64, 1024, 16_384, 262_144, 1_048_574];

/// Through-webview transport the bench exercises. `Default` is the current
/// wire: JSON replies up to 64 KiB and one-shot T008 shared buffers above.
/// `RingZerocopy` opts into the spike transport: one host-owned shared slot
/// arena posted once with read-write access, raw payload bytes in slots, and
/// tiny JSON control messages (see docs/ZEROCOPY_IPC_MOONSHOT.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcBenchTransport {
    Default,
    RingZerocopy,
}

impl IpcBenchTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            IpcBenchTransport::Default => "default",
            IpcBenchTransport::RingZerocopy => "ring_zerocopy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "default" | "json" | "t008_shared" => Some(IpcBenchTransport::Default),
            "ring_zerocopy" | "ring" => Some(IpcBenchTransport::RingZerocopy),
            _ => None,
        }
    }
}

/// JavaScript injected into the Kiri WebView. Uses the real
/// `window.ipc.postMessage` / `window.kiri.send` path and waits for
/// `window.kiri.onResponse` (host `evaluate_script` of the wire response).
///
/// With `transport == IpcBenchTransport::RingZerocopy` the same script also
/// consumes the host-owned slot arena (`window.__kiriRing`, captured from the
/// one-time `sharedbufferreceived` event): request payloads are written as
/// raw bytes into a claimed slot, replies are read back from the same slot,
/// and only tiny JSON control messages cross `postMessage` both ways.
pub fn kiri_script(
    runs: u32,
    warmup: u32,
    sizes: &[usize],
    transport: IpcBenchTransport,
) -> String {
    let sizes_json = serde_json::to_string(sizes).unwrap_or_else(|_| "[]".into());
    let want_ring = transport == IpcBenchTransport::RingZerocopy;
    format!(
        r#"(function () {{
  if (window.__kiriIpcBenchStarted) return;
  window.__kiriIpcBenchStarted = true;
  var RUNS = {runs};
  var WARMUP = {warmup};
  var CONCURRENCY = {concurrency};
  var SIZES = {sizes_json};
  var TIMEOUT = {timeout};
  var WANT_RING = {want_ring};
  var TRANSPORT = WANT_RING ? "ring_zerocopy" : "default";
  var RING = {{
    GLOBAL_MAGIC: {global_magic},
    SLOT_MAGIC: {slot_magic},
    GLOBAL_HDR: {global_hdr},
    SLOT_HDR: {slot_hdr},
    STATE_FREE: 0, STATE_REQ: 1, STATE_RESP: 2,
    CODEC_JSON: 1, CODEC_UTF8: 2,
    RESP_JSON: 0, RESP_ECHO: 1
  }};
  window.__kiriIpcSeq = window.__kiriIpcSeq || 1;
  window.__kiriRing = window.__kiriRing || null;
  window.__kiriRingSeq = window.__kiriRingSeq || 0;
  window.__kiriRingHits = window.__kiriRingHits || 0;
  window.__kiriRingFallbacks = window.__kiriRingFallbacks || 0;
  window.__kiriRingSlots = window.__kiriRingSlots || {{}};
  function ringReady() {{ return !!window.__kiriRing; }}
  function waitRing(ms) {{
    return new Promise(function (resolve) {{
      if (ringReady()) {{ resolve(true); return; }}
      var t0 = performance.now();
      var iv = setInterval(function () {{
        if (ringReady()) {{ clearInterval(iv); resolve(true); }}
        else if (performance.now() - t0 > ms) {{ clearInterval(iv); resolve(false); }}
      }}, 5);
    }});
  }}
  function ringClaim() {{
    var r = window.__kiriRing;
    if (!r) return -1;
    for (var n = 0; n < r.slotCount; n++) {{
      var i = (r.next + n) % r.slotCount;
      var base = r.arena + i * r.slotBytes;
      if (r.dv.getUint8(base + 4) === RING.STATE_FREE) {{
        r.next = (i + 1) % r.slotCount;
        return i;
      }}
    }}
    return -1;
  }}
  function ringFree(i) {{
    var r = window.__kiriRing;
    if (!r) return;
    r.dv.setUint8(r.arena + i * r.slotBytes + 4, RING.STATE_FREE);
  }}
  function sendRing(id, payload) {{
    var r = window.__kiriRing;
    var slot = ringClaim();
    if (slot < 0) return false;
    var codec, bytes;
    if (typeof payload === "string") {{
      codec = RING.CODEC_UTF8;
      bytes = new TextEncoder().encode(payload);
    }} else {{
      codec = RING.CODEC_JSON;
      bytes = new TextEncoder().encode(JSON.stringify(payload === undefined ? null : payload));
    }}
    if (bytes.length > r.payloadCap) return false;
    var base = r.arena + slot * r.slotBytes;
    new Uint8Array(r.buf, base + RING.SLOT_HDR, bytes.length).set(bytes);
    r.dv.setUint32(base + 0, RING.SLOT_MAGIC, true);
    r.dv.setUint8(base + 5, 0);
    r.dv.setUint16(base + 6, codec, true);
    var seq = ++window.__kiriRingSeq;
    r.dv.setBigUint64(base + 8, BigInt(seq), true);
    r.dv.setBigUint64(base + 16, BigInt(id), true);
    r.dv.setUint32(base + 24, 1, true);
    r.dv.setUint32(base + 28, bytes.length, true);
    r.dv.setUint8(base + 4, RING.STATE_REQ);
    window.__kiriRingSlots[id] = slot;
    window.chrome.webview.postMessage({{ type: "cmd", ring: {{ slot: slot, request_id: id }} }});
    return true;
  }}
  function ringResponse(d) {{
    var r = window.__kiriRing;
    if (!r || typeof d.slot !== "number" || d.slot < 0 || d.slot >= r.slotCount) return;
    var base = r.arena + d.slot * r.slotBytes;
    if (r.dv.getUint32(base + 0, true) !== RING.SLOT_MAGIC) return;
    if (r.dv.getUint8(base + 4) !== RING.STATE_RESP) return;
    var rid = Number(r.dv.getBigUint64(base + 16, true));
    var flags = r.dv.getUint8(base + 5);
    var plen = r.dv.getUint32(base + 28, true);
    var resp;
    try {{
      var bytes = new Uint8Array(r.buf, base + RING.SLOT_HDR, plen);
      if (flags === RING.RESP_ECHO) {{
        resp = {{ request_id: rid,
          payload: {{ pong: true, echo: new TextDecoder("utf-8").decode(bytes) }} }};
      }} else {{
        resp = JSON.parse(new TextDecoder("utf-8").decode(bytes));
      }}
    }} catch (err) {{ return; }}
    window.__kiriRingHits++;
    if (window.kiri && window.kiri.onResponse) window.kiri.onResponse(resp);
  }}
  if (window.chrome && window.chrome.webview && window.chrome.webview.addEventListener && !window.__kiriWebMessageHooked) {{
    window.__kiriWebMessageHooked = true;
    window.chrome.webview.addEventListener("message", function (e) {{
      var d = e.data;
      if (typeof d === "string") {{
        try {{ d = JSON.parse(d); }} catch (err) {{ return; }}
      }}
      if (d && d.type === "ring_resp") {{
        ringResponse(d);
        return;
      }}
      if (d && d.request_id !== undefined && window.kiri && window.kiri.onResponse) {{
        window.kiri.onResponse(d);
      }}
    }});
    window.__kiriSharedBufferHits = 0;
    window.chrome.webview.addEventListener("sharedbufferreceived", function (e) {{
      try {{
        var buf = e.getBuffer();
        if (buf.byteLength >= RING.GLOBAL_HDR
            && new DataView(buf).getUint32(0, true) === RING.GLOBAL_MAGIC) {{
          var dv = new DataView(buf);
          window.__kiriRing = {{
            buf: buf, dv: dv, next: 0,
            slotCount: dv.getUint32(8, true),
            slotBytes: dv.getUint32(12, true),
            payloadCap: dv.getUint32(16, true),
            arena: RING.GLOBAL_HDR
          }};
          return;
        }}
        window.__kiriSharedBufferHits++;
        var text = new TextDecoder("utf-8").decode(new Uint8Array(buf));
        window.chrome.webview.releaseBuffer(buf);
        var d = JSON.parse(text);
        if (d && d.request_id !== undefined && window.kiri && window.kiri.onResponse) {{
          window.kiri.onResponse(d);
        }}
      }} catch (err) {{}}
    }});
  }}
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
        var stale = window.__kiriRingSlots[id];
        if (stale !== undefined) {{ delete window.__kiriRingSlots[id]; ringFree(stale); }}
        reject(new Error("ipc timeout after " + TIMEOUT + "ms"));
      }}, TIMEOUT);
      window.kiri.pending[id] = function (resp) {{
        clearTimeout(timer);
        var s = window.__kiriRingSlots[id];
        if (s !== undefined) {{ delete window.__kiriRingSlots[id]; ringFree(s); }}
        if (resp && resp.error) {{
          reject(new Error((resp.error && resp.error.message) || "ipc error"));
        }} else {{
          resolve(resp);
        }}
      }};
      if (WANT_RING) {{
        if (ringReady() && sendRing(id, payload)) return;
        window.__kiriRingFallbacks++;
      }}
      var payloadJson = JSON.stringify(payload);
      var payloadLen = typeof TextEncoder === "function"
        ? new TextEncoder().encode(payloadJson).length
        : payloadJson.length;
      window.kiri.send({{
        magic: "KRI1",
        version: 1,
        flags: 1,
        command_id: 1,
        request_id: id,
        payload_len: payloadLen,
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
    if (WANT_RING) await waitRing(2000);
    var results = [];
    for (var s = 0; s < SIZES.length; s++) {{
      var size = SIZES[s];
      var payload = makePayload(size);
      for (var w = 0; w < WARMUP; w++) {{
        await ping(payload);
      }}
      var samples = [];
      var hitsBefore = window.__kiriSharedBufferHits || 0;
      var ringBefore = window.__kiriRingHits || 0;
      var ringFbBefore = window.__kiriRingFallbacks || 0;
      var tBatch = performance.now();
      for (var i = 0; i < RUNS; i++) {{
        var t0 = performance.now();
        await ping(payload);
        samples.push(performance.now() - t0);
      }}
      var batchMs = performance.now() - tBatch;
      var concurrentBatches = [];
      for (var c = 0; c < RUNS; c++) {{
        var concurrentStart = performance.now();
        var pending = [];
        for (var j = 0; j < CONCURRENCY; j++) pending.push(ping(payload));
        await Promise.all(pending);
        concurrentBatches.push(performance.now() - concurrentStart);
      }}
      var hits = (window.__kiriSharedBufferHits || 0) - hitsBefore;
      var ringHits = (window.__kiriRingHits || 0) - ringBefore;
      var ringFb = (window.__kiriRingFallbacks || 0) - ringFbBefore;
      results.push({{
        size_bytes: size,
        rtt_ms: samples,
        batch_ms: batchMs,
        mean_from_batch_ms: batchMs / RUNS,
        concurrent_batch_ms: concurrentBatches,
        concurrency: CONCURRENCY,
        shared_buffer_hits: hits,
        shared_buffer_used: hits > 0,
        ring_slot_hits: ringHits,
        ring_send_fallbacks: ringFb
      }});
    }}
    post({{ type: "ipc_bench", target: "kiri-host", transport: TRANSPORT,
      runs: RUNS, warmup: WARMUP,
      ring_send_fallbacks: window.__kiriRingFallbacks || 0,
      results: results }});
  }}
  run().catch(function (err) {{
    post({{ type: "ipc_bench", target: "kiri-host", transport: TRANSPORT,
      runs: RUNS, warmup: WARMUP, error: String(err) }});
  }});
}})();"#,
        runs = runs,
        warmup = warmup,
        concurrency = DEFAULT_CONCURRENCY,
        sizes_json = sizes_json,
        timeout = CALL_TIMEOUT_MS,
        want_ring = want_ring,
        global_magic = crate::ring_ipc::GLOBAL_MAGIC,
        slot_magic = crate::ring_ipc::SLOT_MAGIC,
        global_hdr = crate::ring_ipc::GLOBAL_HEADER_BYTES,
        slot_hdr = crate::ring_ipc::SLOT_HEADER_BYTES,
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
  var CONCURRENCY = {concurrency};
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
      var concurrentBatches = [];
      for (var c = 0; c < RUNS; c++) {{
        var concurrentStart = performance.now();
        var pending = [];
        for (var j = 0; j < CONCURRENCY; j++) pending.push(invokeEcho(payload));
        await Promise.all(pending);
        concurrentBatches.push(performance.now() - concurrentStart);
      }}
      results.push({{
        size_bytes: size,
        rtt_ms: samples,
        batch_ms: batchMs,
        mean_from_batch_ms: batchMs / RUNS,
        concurrent_batch_ms: concurrentBatches,
        concurrency: CONCURRENCY
      }});
    }}
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
      await window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
        json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", runs: RUNS, warmup: WARMUP, results: results }})
      }});
    }}
  }}
  run().catch(function (err) {{
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
      window.__TAURI_INTERNALS__.invoke("kiri_ipc_bench_done", {{
        json: JSON.stringify({{ type: "ipc_bench", target: "tauri-baseline", runs: RUNS, warmup: WARMUP, error: String(err) }})
      }});
    }}
  }});
}})();"#,
        runs = runs,
        warmup = warmup,
        concurrency = DEFAULT_CONCURRENCY,
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
        "p99_ms": percentile(&sorted, 0.99),
    })
}

fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Provenance the scoreboard gate (`benchmark/scoreboard_gate.py`) requires:
/// run id (hosted) or host id (local), plus runner/OS/arch. Values come from
/// the measuring machine's environment; nothing is invented.
fn run_metadata() -> Value {
    let run_id = env_nonempty("GITHUB_RUN_ID");
    let url = match (
        env_nonempty("GITHUB_SERVER_URL"),
        env_nonempty("GITHUB_REPOSITORY"),
        run_id.as_deref(),
    ) {
        (Some(server), Some(repo), Some(id)) => {
            json!(format!("{server}/{repo}/actions/runs/{id}"))
        }
        _ => Value::Null,
    };
    let host_id = env_nonempty("KIRI_BENCH_HOST_ID")
        .or_else(|| env_nonempty("HOSTNAME"))
        .or_else(|| env_nonempty("COMPUTERNAME"));
    json!({
        "id": run_id,
        "url": url,
        "runner": env_nonempty("RUNNER_NAME").or_else(|| env_nonempty("RUNNER_OS")),
        "os": env_nonempty("RUNNER_OS").unwrap_or_else(|| std::env::consts::OS.to_string()),
        "arch": env_nonempty("RUNNER_ARCH").unwrap_or_else(|| std::env::consts::ARCH.to_string()),
        "host_id": host_id,
    })
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
            let concurrent_batch_ms =
                item.get("concurrent_batch_ms").cloned().unwrap_or_else(|| json!([]));
            let concurrency = item.get("concurrency").and_then(|v| v.as_u64());
            let shared_hits = item.get("shared_buffer_hits").and_then(|v| v.as_u64());
            let shared_used = item.get("shared_buffer_used").and_then(|v| v.as_bool());
            let ring_hits = item.get("ring_slot_hits").and_then(|v| v.as_u64());
            let ring_fb = item.get("ring_send_fallbacks").and_then(|v| v.as_u64());
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
                "concurrent_batch_ms": concurrent_batch_ms,
                "concurrency": concurrency,
                "shared_buffer_hits": shared_hits,
                "shared_buffer_used": shared_used.unwrap_or(false),
                "ring_slot_hits": ring_hits,
                "ring_send_fallbacks": ring_fb,
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
        "transport": raw.get("transport").cloned().unwrap_or(json!("default")),
        "error": raw.get("error").cloned(),
        "commit": git_commit(),
        "created_unix_ns": created,
        "run": run_metadata(),
        "runs": raw.get("runs").and_then(|v| v.as_u64()).map(|v| v as usize)
            .or_else(|| results_out.first().and_then(|r| r.get("rtt_ms")).and_then(|v| v.as_array()).map(|a| a.len()))
            .unwrap_or(DEFAULT_RUNS as usize),
        "warmup": raw.get("warmup").and_then(|v| v.as_u64()).map(|v| json!(v)).unwrap_or(json!(DEFAULT_WARMUP)),
        "sizes_bytes": results_out.iter().filter_map(|r| r.get("size_bytes").cloned()).collect::<Vec<_>>(),
        "shared_buffer": {
            "threshold_bytes": 64 * 1024,
            "replies_ok": raw.get("shared_buffer_replies_ok").cloned().unwrap_or(json!(0)),
            "replies_fallback": raw.get("shared_buffer_replies_fallback").cloned().unwrap_or(json!(0)),
        },
        "ring": {
            "replies_ok": raw.get("ring_replies_ok").cloned().unwrap_or(json!(0)),
            "replies_fallback": raw.get("ring_replies_fallback").cloned().unwrap_or(json!(0)),
            "send_fallbacks": raw.get("ring_send_fallbacks").cloned().unwrap_or(json!(0)),
        },
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
    use serde_json::{json, Value};

    #[test]
    fn kiri_script_mentions_real_bridge_and_sizes() {
        let script = kiri_script(7, 2, DEFAULT_SIZES, IpcBenchTransport::Default);
        assert!(script.contains("window.kiri.send"));
        assert!(script.contains("command_id: 1"));
        assert!(script.contains("type: \"ipc_bench\""));
        assert!(script.contains("1048574"));
        assert!(script.contains("shared_buffer_used"));
        assert!(script.contains("Promise.all(pending)"));
        assert!(script.contains("concurrent_batch_ms"));
        assert!(script.contains("WANT_RING = false"));
        assert!(!script.contains("bulk_bench"));
    }

    #[test]
    fn kiri_ring_script_enables_ring_transport() {
        let script = kiri_script(7, 2, DEFAULT_SIZES, IpcBenchTransport::RingZerocopy);
        assert!(script.contains("WANT_RING = true"));
        assert!(script.contains("\"ring_zerocopy\""));
        assert!(script.contains("sendRing(id, payload)"));
        assert!(script.contains("type: \"cmd\", ring:"));
        assert!(script.contains("ring_resp"));
        assert!(script.contains("ring_slot_hits"));
    }

    #[test]
    fn tauri_script_uses_invoke_echo() {
        let script = tauri_script(7, 2, DEFAULT_SIZES);
        assert!(script.contains("kiri_echo"));
        assert!(script.contains("kiri_ipc_bench_done"));
        assert!(script.contains("__TAURI_INTERNALS__"));
        assert!(script.contains("Promise.all(pending)"));
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
    fn write_result_records_shared_buffer_proof() {
        let raw = json!({
            "type": "ipc_bench",
            "target": "kiri-host",
            "runs": 2,
            "warmup": 1,
            "shared_buffer_replies_ok": 20,
            "shared_buffer_replies_fallback": 0,
            "results": [{
                "size_bytes": 262144,
                "rtt_ms": [5.0, 5.0],
                "shared_buffer_hits": 2,
                "shared_buffer_used": true
            }]
        });
        let dir = std::env::temp_dir().join("kiri-ipc-proof");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ipc.json");
        write_result(Some(&path), &raw).expect("write");
        let out: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(out["shared_buffer"]["replies_ok"], 20);
        assert_eq!(out["results"][0]["shared_buffer_used"], true);
    }

    #[test]
    fn write_result_records_runs_warmup_and_run_provenance() {
        let raw = json!({
            "type": "ipc_bench",
            "target": "kiri-host",
            "runs": 7,
            "warmup": 3,
            "results": [{
                "size_bytes": 64,
                "rtt_ms": [1.0, 1.0],
                "shared_buffer_hits": 0,
                "shared_buffer_used": false
            }]
        });
        let dir = std::env::temp_dir().join("kiri-ipc-provenance");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ipc.json");
        write_result(Some(&path), &raw).expect("write");
        let out: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(out["runs"], 7);
        assert_eq!(out["warmup"], 3);
        assert!(out["run"].is_object());
        assert!(out["run"]["os"].is_string());
        assert!(out["run"]["arch"].is_string());
        assert!(out["run"].get("id").is_some());
        assert!(out["run"].get("host_id").is_some());
    }

    #[test]
    fn one_mib_string_exceeds_control_ceiling_last_size_does_not() {
        let too_big =
            serde_json::to_vec(&serde_json::Value::String("a".repeat(1_048_576))).unwrap().len();
        let last = serde_json::to_vec(&serde_json::Value::String(
            "a".repeat(*DEFAULT_SIZES.last().unwrap()),
        ))
        .unwrap()
        .len();
        assert!(too_big as u32 > kiri_core::constants::DEFAULT_MAX_CONTROL_PAYLOAD_BYTES);
        assert!(last as u32 <= kiri_core::constants::DEFAULT_MAX_CONTROL_PAYLOAD_BYTES);
    }
}
