# Zero-copy through-webview IPC moonshot (spike)

Status: implemented behind `--ipc-bench-transport ring_zerocopy` (opt-in,
bench-only surface). Through-webview RTT is unmeasured on this host; see the
measurement table below.

## Mechanism: reusable host-owned slot arena

T008 creates one `ICoreWebView2SharedBuffer` per reply over 64 KiB, copies the
serialized JSON response into it, posts it read-only, and closes it. The page
then UTF-8 decodes and `JSON.parse`s the whole response. That is a COM
allocation plus a full JSON round trip per large reply.

The ring transport replaces that with one arena for the whole session:

1. When the ipc bench starts (first animation frame), the host calls
   `CreateSharedBuffer` once for a 16 MiB arena, writes a global header
   (`KRIR` magic, version, slot count, slot stride, payload capacity), and
   posts it once with `PostSharedBufferToScript(...,
   COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE, {"kind":"kiri_ring"})`. The
   host keeps the buffer object alive for the session; script keeps the
   `ArrayBuffer` from the single `sharedbufferreceived` event and never calls
   `releaseBuffer`. This is the documented WebView2 model: both parties map
   the same memory and writes are visible to all parties.
2. The arena holds 16 slots of `32 B header + 1 MiB payload`. Slot headers
   are little-endian: `KRSL` magic, state byte, flags byte, codec, seq,
   request_id, command_id, payload_len. A slot cycles free -> request ->
   response -> free; the page is the only allocator, the host is the only
   response writer.
3. Request path: the page claims a free slot, writes raw payload bytes (UTF-8
   for string payloads, JSON otherwise), publishes the header, and posts a
   tiny control `{"type":"cmd","ring":{"slot":i,"request_id":id}}`.
4. The host validates the slot (index bounds, magic, state, payload_len <=
   1 MiB), copies the payload out of shared memory (a private snapshot, so a
   racing page cannot mutate the bytes being dispatched), rebuilds a
   `WireRequest`, and runs the unchanged dispatch pipeline: origin gate,
   capability bit, allowlists, payload limits.
5. Reply path: `kiri.ping` responses (`{pong:true, echo:<string>}`) write the
   echo string bytes verbatim (`RESP_ECHO_STRING`), so no megabyte JSON is
   ever encoded or parsed. Any other response shape is serialized once as
   JSON into the slot (`RESP_JSON`). The host then posts a tiny control
   `{"type":"ring_resp","slot":i,"seq":n,"request_id":id}`; the page reads
   the slot, rebuilds the response object, and frees the slot.
6. Fallbacks stay honest: no free slot, oversized payload, or malformed slot
   content falls back to the ordinary JSON + T008 wire, and every fallback is
   counted (`ring_send_fallbacks` page-side, `ring_replies_fallback`
   host-side). If `init_ring` itself fails, the host injects the default
   bench script and the artifact reports `transport: "default"`.

## How it differs from Tauri and from T008

- Tauri `invoke` ships every request and response as serialized values over
  the ordinary webview channel; it has no shared-memory transport, so a
  ~1 MiB payload is JSON-encoded and parsed on every call.
- T008 (current Kiri wire) halves that for large replies but still creates a
  new shared buffer per reply and still JSON-encodes/decodes the payload.
- The ring allocates once, carries raw payload bytes both directions, and
  needs only ~100-byte control messages per call. The remaining cost per
  call is two small `postMessage` hops plus one bounded memcpy into the
  arena on each side, which is the irreducible copy of a shared-memory
  transport.

## Security posture

Unchanged. Ring requests still arrive through `WebMessageReceived` behind
`is_app_origin_url`, and the rebuilt `WireRequest` goes through
`Router::dispatch`, so capability bits, allowlists, payload-length checks,
and limits all apply exactly as on the JSON wire. The transport adds bounds,
magic, state, and request_id checks, and it snapshots payload bytes before
parsing. No check was weakened for speed.

## Measured numbers

### Through-webview RTT

Not measured on this host (Linux container, no display; WebView2 is
Windows-only). The ring path is implemented and compile-verified for
`x86_64-pc-windows-msvc`; measurement requires the hosted Windows run:

```powershell
.\kiri-host.exe --frontend examples\blank --ipc-bench `
  --ipc-bench-runs 30 --ipc-bench-transport ring_zerocopy `
  --ipc-bench-out artifacts\ipc-ring.json
# baseline, same runs/sizes/warmup:
.\kiri-host.exe --frontend examples\blank --ipc-bench `
  --ipc-bench-runs 30 --ipc-bench-out artifacts\ipc-default.json
```

| Payload | ring_zerocopy mean RTT | default wire mean RTT |
|--------:|---------------------:|----------------------:|
| 16 KiB | N/A (not measured) | N/A |
| 256 KiB | N/A | N/A |
| ~1 MiB | N/A | N/A |

On pull requests, the `controlled-performance` workflow now runs both
transports on `windows-latest`: the default wire writes
`artifacts/ipc-kiri.json` and `ring_zerocopy` writes
`artifacts/ipc-kiri-ring.json` (same frontend, runs, and warmup), and both
land in the `perf-windows-latest-ipc` upload. Each artifact also stamps a
`run` provenance block (run id/url, runner, os, arch, host_id) that
`benchmark/scoreboard_gate.py` requires. The table stays N/A until those
artifacts exist; numbers are only filled in from CI-measured runs.

For context, the last hosted default-wire numbers (Actions run 31988662774,
20 measured replies per size): 0.92 ms at 16 KiB, 4.75 ms at 256 KiB,
22.46 ms at ~1 MiB; Tauri invoke measured 11.08 ms at 256 KiB and 40.52 ms
at ~1 MiB in the same run. See docs/SHARED_BUFFER_REPORT.md.

### In-process serialization cost model (this host, release build)

`crates/kiri-runtime/examples/ring_cost.rs`, 50 iterations per size on the
Linux CI box. This is a component measurement only, not IPC: it isolates the
per-reply serde work the ring removes (host encode plus a parse equivalent)
against the single memcpy the ring still pays.

| Payload | JSON encode (host) | JSON parse (page-side stand-in) | memcpy |
|--------:|-------------------:|--------------------------------:|-------:|
| 16 KiB | 0.011 ms | 0.004 ms | 0.000 ms |
| 256 KiB | 0.174 ms | 0.051 ms | 0.009 ms |
| ~1 MiB | 0.718 ms | 0.229 ms | 0.065 ms |

So at ~1 MiB the ring removes roughly 0.95 ms of Rust-side serde work per
reply (and materially more on the JS side, where `JSON.parse` of a megabyte
string plus `TextDecoder` is typically slower than serde_json), plus the
per-reply `CreateSharedBuffer`/`Close` COM calls the default path still
makes. Whether that shows up in through-webview RTT is exactly what the
hosted run above must measure.

### Verification done on this host

- `ring_ipc` unit tests: layout, global header, request/response state
  cycle, capacity and ownership bounds, request_id checks.
- The emitted `ring_zerocopy` bench script was syntax-checked and exercised
  end to end in Node 20 against a simulated host implementing the same slot
  layout over a real `ArrayBuffer`: 27 ring round-trips per size (3 measured
  runs plus 3 batches of 8 concurrent pings), 0 send fallbacks, 0 wire
  sends, and both codecs exercised (`CODEC_UTF8_STRING` -> `RESP_ECHO_STRING`
  for string payloads, `CODEC_JSON` -> `RESP_JSON` for the null payload).
- `cargo check` and `cargo clippy` for `x86_64-pc-windows-msvc` are clean.
- The real WebView2 pieces (one-time `PostSharedBufferToScript` with
  READ_WRITE, persistent host-side mapping) are API-verified against the
  published WebView2 spec but not runtime-verified here.

## Kill vs continue

Recommendation: continue. The mechanism is implemented, compiles for the
Windows target, keeps every security gate, and removes real per-reply costs
that the in-process numbers confirm. The through-webview verdict needs the
hosted Windows measurement.

Kill criteria: kill the spike (revert to the default wire) if, on
`windows-latest` with the same sizes/runs/warmup, `ring_zerocopy` fails to
beat the default wire's mean RTT by a clear margin at 256 KiB and ~1 MiB, or
if it regresses the small-payload path (16 KiB and below) beyond noise. A
kill means keeping T008, which already crosses over JSON at 64 KiB.

## Files in this spike

- `crates/kiri-runtime/src/ring_ipc.rs`: arena layout and slot framing,
  unit-tested on every OS.
- `crates/kiri-runtime/src/host_windows.rs`: `init_ring` (one-time
  READ_WRITE post), `handle_ring_request`, reply publish, counters.
- `crates/kiri-runtime/src/ipc_bench.rs`: `IpcBenchTransport`, ring-aware
  injected bench script, `transport` and `ring` fields in the artifact.
- `crates/kiri-runtime/src/lib.rs` + `src/bin/kiri-host.rs`: `HostOptions.
  ipc_bench_transport` and `--ipc-bench-transport`.
- `crates/kiri-runtime/src/host_cross.rs`: compile-clean fallback (default
  wire plus an explicit note) since WebView2 does not exist off Windows.
- `crates/kiri-runtime/examples/ring_cost.rs`: the in-process cost model
  used above.
