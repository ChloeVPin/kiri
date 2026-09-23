# Mac large-payload through-webview protocol ring (spike)

Status: implemented behind `--ipc-bench-transport protocol_ring` (opt-in,
bench-only surface) on the wry/tao host, which covers macOS and Linux. Local
WebKitGTK end-to-end verified; hosted `macos-latest` numbers pending on the
controlled-performance workflow for this branch.

## Root cause

The hosted dual-run at Actions run 35921338087 (commit `6e1e068`) showed
Kiri beating Tauri through the webview at every size up to 16 KiB, then
losing at 256 KiB (3.00 ms vs 2.20 ms) and at about 1 MiB (11.55 ms vs
5.45 ms). The Mac artifact recorded `transport=default`,
`shared_buffer.replies_ok=0`, and ring `replies_ok=0`: no fast reply leg was
engaged.

Code path: `host_cross.rs::post_response` serializes the complete
`WireResponse` to JSON and injects it into the page with wry
`evaluate_script`. A 1 MiB reply therefore pays a full JSON stringify plus a
WKWebView script evaluation of a megabyte source string before the page can
`JSON.parse` it back. `host_cross.rs` also forced `ring_zerocopy` back to
`Default` with "ring_zerocopy transport requires the Windows WebView2 host",
because the reusable slot arena is a WebView2 `SharedBuffer` object that has
no WKWebView analog.

Tauri's macOS invoke does not round-trip replies through `evaluate_script`:
the page `fetch`es `ipc://localhost/...` and reads the response body. That
transport class is measurably cheaper at 1 MiB on the hosted runner (5.45 ms
vs Kiri's 11.55 ms `evaluate_script` wire).

## Mechanism invented: protocol_ring

`protocol_ring` is a full-duplex binary invoke over the existing `kiri://`
app-origin custom protocol, reusing the platform-neutral KRSL slot framing
from `ring_ipc.rs` as a byte-channel wire format instead of a shared-memory
layout. It is not a port of the WebView2 mechanism (there is no shared
buffer on WKWebView) and it is not a copy of Tauri's invoke (Kiri carries
raw slot frames, not JSON bodies, and the reply leg is grant-gated).

Request path:

1. The page encodes one KRSL request frame (32 B header + raw payload
   bytes, UTF-8 or JSON codec) and POSTs it to
   `kiri://localhost/.kiri/ipc/invoke`.
2. The wry async protocol handler parks the `RequestAsyncResponder` and the
   body in a queue, then wakes the tao event loop through
   `EventLoopProxy::send_event`.
3. On the event loop, `drain_proto_inbox` validates the frame (magic,
   state, `payload_len` bounds, exact length), reconstructs the same
   `WireRequest` the postMessage pipe would carry, and dispatches through
   the same `ZcIpcGate::dispatch_through_webview` call. Capability bit and
   surface allowlist minting are identical; the same router, caller
   identity, diagnostics, and resource table serve both legs.

Reply path:

1. `ring_ipc::response_echo_bytes` extracts the echoed string for
   `kiri.ping` replies so the binary frame can carry it verbatim
   (`RESP_ECHO_STRING`); other responses carry serialized `WireResponse`
   bytes (`RESP_JSON`).
2. `ring_ipc::ring_reply_authorized` requires a live `ZcIpcGrant` bound to
   this caller and command. No grant, expired grant, or mismatched digest
   means the binary leg refuses.
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
  only when the caller capability bit and the declared host allowlist both
  admit. Denied requests carry no grant, so their error stub rides the JSON
  leg.
- The binary reply leg accepts only `ZcIpcGrant` currency through
  `ring_reply_authorized`, fail closed, identical to the Windows slot
  publish path. No parallel permit exists.
- `/.kiri/ipc/invoke` and `/.kiri/ipc/ping` are intercepted only while the
  transport is engaged; otherwise they fall through to asset serving and
  404. Default applications are unchanged.
- The `kiri://` scheme remains app-origin gated by the existing navigation
  and origin checks; the fetch is same-origin from the app page and the
  handler emits no CORS headers, so cross-origin pages cannot read it.
- `payload_len` is capped at `SLOT_PAYLOAD_CAP` (1 MiB) and the frame must
  match its declared length exactly; bounds checks mirror `read_request`.

## Measurements

Hosted `macos-latest` comparison (preferred proof): pending. The
controlled-performance workflow gained a `protocol_ring` leg on the macOS
runner producing `artifacts/ipc-kiri-proto-ring.json` gated by the
scoreboard proof checker.

Local spike-only numbers (Linux, WebKitGTK under Xvfb, debug build, 10
runs x 8 concurrent, sizes 0..1 MiB). WebKitGTK's scheme round trip is not
WKWebView's; these numbers say nothing about the Mac win/lose question and
exist only to show the mechanism engages end to end:

| size_bytes | default mean_ms | protocol_ring mean_ms | replies leg |
|-----------:|----------------:|----------------------:|-------------|
| 0 | 0.9 | 0.2 | 570 binary / 0 fallback total |
| 64 | 0.1 | 0.3 | |
| 1024 | 0.4 | 0.4 | |
| 16384 | 3.1 | 2.9 | |
| 262144 | 36.8 | 35.4 | |
| 1048574 | 142.5 | 140.8 | |

Artifact counters: `protocol_ring.replies_ok=570`,
`replies_fallback=0`, `send_fallbacks=0`, `proto_ring_hits=90` per size.
On WebKitGTK the transport is at parity with `evaluate_script` at 1 MiB;
the soup scheme handler itself dominates the round trip there. WKWebView's
`WKURLSchemeHandler` is different code, and the hosted Tauri comparison is
the evidence that the fetch-over-scheme class wins on that engine.

## Kill / continue recommendation

Continue to hosted measurement. The mechanism is verified end to end on
the shared wry code path (all replies took the binary leg, zero fallbacks,
gate intact), the transport class already wins on macOS for Tauri, and the
worst plausible outcome is parity plus a small queue hop. If the hosted
artifact shows no improvement over `default` at 256 KiB and about 1 MiB,
kill: the evaluate_script bottleneck hypothesis would be disproven for
WKWebView and the honest artifact still documents the attempt.
