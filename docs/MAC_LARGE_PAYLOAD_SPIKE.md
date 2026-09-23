# Mac large-payload through-webview protocol ring (spike)

Status: implemented behind `--ipc-bench-transport protocol_ring` (opt-in,
bench-only surface) on the wry/tao host, which covers macOS and Linux.
Measured on hosted `macos-latest` (run 35926754518). Verdict: kill the
beat-Tauri hypothesis. The transport engages 100 percent of replies and
improves about 1 MiB by 17.5 percent over the same-run default wire, but
it does not beat Tauri at 256 KiB or about 1 MiB.

## Root cause

The hosted dual-run at Actions run 35921338087 (commit `6e1e068`) showed
Kiri beating Tauri through the webview at every size up to 16 KiB, then
losing at 256 KiB (3.00 ms vs 2.20 ms) and at about 1 MiB (11.55 ms vs
5.45 ms). The Mac artifact recorded `transport=default`,
`shared_buffer.replies_ok=0`, and ring `replies_ok=0`: no fast reply leg
was engaged.

Code path: `host_cross.rs::post_response` serializes the complete
`WireResponse` to JSON and injects it into the page with wry
`evaluate_script`. A 1 MiB reply therefore pays a full JSON stringify plus
a WKWebView script evaluation of a megabyte source string before the page
can `JSON.parse` it back. `host_cross.rs` also forced `ring_zerocopy`
back to `Default` with "ring_zerocopy transport requires the Windows
WebView2 host", because the reusable slot arena is a WebView2
`SharedBuffer` object that has no WKWebView analog.

Tauri's macOS invoke does not round-trip replies through
`evaluate_script`: the page `fetch`es `ipc://localhost/...` and reads the
response body. That transport class is measurably cheaper at 1 MiB on the
hosted runner.

## Mechanism invented: protocol_ring

`protocol_ring` is a full-duplex binary invoke over the existing `kiri://`
app-origin custom protocol, reusing the platform-neutral KRSL slot
framing from `ring_ipc.rs` as a byte-channel wire format instead of a
shared-memory layout. It is not a port of the WebView2 mechanism (there
is no shared buffer on WKWebView) and it is not a copy of Tauri's invoke
(Kiri carries raw slot frames, not JSON bodies, and the reply leg is
grant-gated).

Request path:

1. The page encodes one KRSL request frame (32 B header + raw payload
   bytes, UTF-8 or JSON codec) and POSTs it to
   `kiri://localhost/.kiri/ipc/invoke`.
2. The wry async protocol handler parks the `RequestAsyncResponder` and
   the body in a queue, then wakes the tao event loop through
   `EventLoopProxy::send_event`.
3. On the event loop, `drain_proto_inbox` validates the frame (magic,
   state, `payload_len` bounds, exact length), reconstructs the same
   `WireRequest` the postMessage pipe would carry, and dispatches through
   the same `ZcIpcGate::dispatch_through_webview` call. Capability bit
   and surface allowlist minting are identical; the same router, caller
   identity, diagnostics, and resource table serve both legs.

Reply path:

1. `ring_ipc::response_echo_bytes` extracts the echoed string for
   `kiri.ping` replies so the binary frame can carry it verbatim
   (`RESP_ECHO_STRING`); other responses carry serialized `WireResponse`
   bytes (`RESP_JSON`).
2. `ring_ipc::ring_reply_authorized` requires a live `ZcIpcGrant` bound
   to this caller and command. No grant, expired grant, or mismatched
   digest means the binary leg refuses.
3. Authorized replies return `application/octet-stream` with one KRSL
   response frame. Everything else (malformed frame, unsupported codec,
   missing or invalid grant, oversize payload, gate denial) returns the
   plain serialized `WireResponse` as `application/json`, and the page
   treats the JSON content type as the ungated fallback leg.

No megabyte string crosses `postMessage` or `evaluate_script` in either
direction. A probe endpoint at `kiri://localhost/.kiri/ipc/ping` lets the
bench script verify the transport is live before sending, and
`window.__kiriProtoReady` falling false downgrades every send to the
default postMessage wire with `proto_ring_send_fallbacks` incremented.

## Security behavior

- Same double gate: `ZcIpcGate::dispatch_through_webview` mints the grant
  only when the caller capability bit and the declared host allowlist
  both admit. Denied requests carry no grant, so their error stub rides
  the JSON leg.
- The binary reply leg accepts only `ZcIpcGrant` currency through
  `ring_reply_authorized`, fail closed, identical to the Windows slot
  publish path. No parallel permit exists.
- `/.kiri/ipc/invoke` and `/.kiri/ipc/ping` are intercepted only while
  the transport is engaged; otherwise they fall through to asset serving
  and 404. Default applications are unchanged.
- The `kiri://` scheme remains app-origin gated by the existing
  navigation and origin checks; the fetch is same-origin from the app
  page and the handler emits no CORS headers, so cross-origin pages
  cannot read it.
- `payload_len` is capped at `SLOT_PAYLOAD_CAP` (1 MiB) and the frame
  must match its declared length exactly; bounds checks mirror
  `read_request`.

## Measurements

Hosted `macos-latest`, Actions run 35926754518 on this branch tip
(`ipc-kiri-proto-ring.json`, `ipc-kiri.json`, `ipc-tauri.json` in the
`perf-macos-latest` artifact; 20 runs x 8 concurrent, release build).
Same-run means:

| size_bytes | kiri default | kiri protocol_ring | tauri | proto/tauri |
|-----------:|-------------:|-------------------:|------:|------------:|
| 0 | 0.550 | 0.650 | 3.750 | 0.17 WIN |
| 64 | 0.450 | 0.550 | 0.850 | 0.65 WIN |
| 1024 | 0.350 | 0.400 | 1.700 | 0.24 WIN |
| 16384 | 0.900 | 0.500 | 1.000 | 0.50 WIN |
| 262144 | 2.000 | 2.000 | 1.550 | 1.29 LOSE |
| 1048574 | 6.850 | 5.650 | 4.450 | 1.27 LOSE |

Engagement: `protocol_ring.replies_ok=1110`, `replies_fallback=0`,
`send_fallbacks=0`, `proto_ring_hits=180` per size. Every reply took the
binary leg end to end on WKWebView.

Reading: the transport works and helps exactly where predicted, about 1
MiB improved 17.5 percent over the same-run default wire (6.85 -> 5.65
ms) and the small-size wins hold. It does not beat Tauri at either
target size. Residual gap is probably the extra hop this design pays:
the responder is parked and dispatched on the event loop (wake + drain +
`onResponse` callback routing) while Tauri answers the fetch on its
scheme path directly (evidence D, inference, not measured). At 256 KiB
protocol_ring is at parity with default, so the reply-leg JSON eval was
not the dominant term there.

Local spike-only confirmation (Linux/WebKitGTK under Xvfb, debug build):
570/570 replies on the binary leg, zero fallbacks, parity with default
(140.8 vs 142.5 ms at about 1 MiB) because the WebKitGTK scheme handler
dominates that round trip. Not publishable; mechanism proof only.

Windows artifacts from the same run are unchanged in behavior:
`ring_zerocopy` still records `replies_ok=1110` with the shared
`response_echo_bytes` helper.

## Kill / continue recommendation

Kill the beat-Tauri hypothesis for this mechanism. With the binary leg
carrying 100 percent of replies, protocol_ring still loses to Tauri at
256 KiB (2.00 vs 1.55 ms) and about 1 MiB (5.65 vs 4.45 ms) on hosted
macOS. The evaluate_script-reply bottleneck is real (the same-run 1 MiB
improvement over default proves the reply leg was part of the cost), but
removing it is not sufficient: Tauri's remaining edge sits in the invoke
round trip itself, not in reply transport.

Optional follow-up if the gap is worth chasing later: answer the fetch
on the protocol thread with a pre-dispatched command table (skipping the
event-loop drain for `kiri.ping`-shaped commands), which would isolate
whether the parked-responder hop is the residual 1.2 ms at 1 MiB. That
is a different spike, not a fix to this one.

The transport code stays behind its opt-in flag with honest counters and
proof gating; nothing here weakens the default wire, the grant model, or
the Windows claims.
