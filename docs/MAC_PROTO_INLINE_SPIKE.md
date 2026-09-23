# Mac large-payload through-webview protocol inline (spike)

Status: implemented behind `--ipc-bench-transport protocol_inline` (opt-in,
bench-only surface) on the wry/tao host, which covers macOS and Linux.
Follow-up to `docs/MAC_LARGE_PAYLOAD_SPIKE.md` (PR #42 kill report): this
spike tests the residual-hop hypothesis recorded there. Hosted numbers are
recorded below once the controlled-performance run on this branch lands.

## Residual hop hypothesis being tested

`protocol_ring` carries binary KRSL frames over the `kiri://` scheme and
engaged 100 percent of replies on hosted macOS (run 35926754518), yet lost
to Tauri at 256 KiB (2.00 vs 1.55 ms) and about 1 MiB (5.65 vs 4.45 ms).
The recorded residual inference (evidence D) was the parked-responder hop:

1. The wry async protocol handler parked the `RequestAsyncResponder` and
   body in `ProtoInbox`, then woke the tao loop via
   `EventLoopProxy::send_event` (host_cross.rs, protocol handler).
2. `drain_proto_inbox` -> `handle_proto_invoke` dispatched on the next
   event-loop iteration, on the main thread, because the router and gate
   were held in `Rc<RefCell<...>>`.
3. Only then did `responder.respond` complete the fetch.

Tauri answers the fetch on its scheme path directly, with no parked
responder and no event-loop wake.

## Mechanism invented: protocol_inline

`protocol_inline` answers the invoke inside the wry asynchronous custom
protocol callback itself, on whatever thread WebKit invoked it on. No
`ProtoInbox` entry, no `EventLoopProxy` wake, no event-loop iteration, for
every command the inline router knows. Same `kiri://localhost/.kiri/ipc/invoke`
endpoint, same KRSL request/response framing as `protocol_ring`
(`ring_ipc::read_request_frame` / `encode_response_frame`), same
`application/octet-stream` binary leg gated by a live `ZcIpcGrant` and the
same `application/json` fallback leg.

The pieces that made it possible without weakening the gate:

- `Router` is `Send + Sync` (every handler is `Arc<dyn Fn + Send + Sync>`),
  so a dedicated inline `Router::new()` lives in an `Arc` shared with the
  protocol callback. It registers exactly `kiri.ping` today: the only
  command whose dispatch is provably free of window, menu, tray, clipboard,
  and other main-thread state. A regression test asserts the inline router
  knows no other catalog command and that `required_bits` for `kiri.ping`
  matches the production router bit-for-bit.
- `ZcIpcGate` moved from `Rc<RefCell<...>>` to `Arc<Mutex<...>>`. There is
  still exactly one gate object and one mint path: the postMessage leg, the
  parked drain, and the inline path all lock the same instance and run
  `dispatch_through_webview` (mint -> redeem -> `Router::dispatch`), so
  capability-bit AND allowlist double-gating is identical.
- `RequestAsyncResponder` is `Send` (wry documents responding from a
  separate thread as the intended use), so `responder.respond` completes
  the fetch from the protocol thread. On macOS the responder calls the
  `WKURLSchemeTask` `didReceiveResponse`/`didReceiveData`/`didFinish`
  sequence directly; on WebKitGTK wry marshals the response to the GTK
  main context internally, which still removes one of the two hops the
  parked path paid.
- `Diagnostics` and `ResourceTable` are already `Arc<Mutex<...>>`, so the
  same sink and resource accounting run on either thread.

What still parks: any invoke frame whose `command_id` the inline router
does not know, and any malformed frame. Those take the unchanged parked
path (`ProtoInbox` + `EventLoopProxy` + `drain_proto_inbox`) and dispatch
through the full production router on the tao thread, so no window/menu
mutation ever leaves the event loop. The `parked` counter reports exactly
how many fetches still paid the hop.

## Security behavior

- Same double gate, same mint path. `dispatch_through_webview` runs on the
  shared `ZcIpcGate`; a permit mints only when the capability bit AND the
  declared surface allowlist admit, and the binary reply leg still requires
  a live `ZcIpcGrant` via `ring_ipc::ring_reply_authorized`. Denials carry
  no grant and ride the JSON leg, fail closed. No parallel permit exists.
- The inline router is narrower than the production router by
  construction: unknown command ids deny at `required_bits` (fail closed),
  and everything not on the inline table parks to the event-loop drain.
- Dispatching `kiri.ping` off the main thread does not weaken
  origin/allowlist checks; those live in the gate and the service layer,
  not in the thread the dispatch happens to run on.
- New native surface for Security review: the `ZcIpcGate` is now locked
  from a non-main thread (the wry protocol callback), and a command
  dispatch runs there for the first time. Scope is `kiri.ping` only, and
  the gate object itself was always `Send`; the review question is whether
  exposing gated dispatch on the protocol thread widens the threat model.
  The endpoints, scheme, and reply-leg currency are unchanged from
  protocol_ring.

## Honest counters

The artifact reports `protocol_inline.answered_inline` (binary replies
completed on the protocol thread, the hop-eliminated count),
`protocol_inline.replies_fallback` (inline answers that took the JSON leg),
`protocol_inline.parked` (invokes that still parked for the event-loop
drain), and `protocol_inline.send_fallbacks` (page-side sends that fell
back when the transport probe failed). `protocol_ring.replies_ok` /
`replies_fallback` continue to count only replies the parked drain served,
so an artifact distinguishes hop-free replies from parked ones.

## Measurements

Local unit-level proof on this Linux box: `proto_frame_response` answers a
KRSL ping frame on the binary leg under a live grant, fails closed to the
JSON leg on capability denial, and returns a 400 JSON protocol error on a
malformed frame (tests in `host_cross.rs`).

Local spike-only engagement proof (Linux/WebKitGTK under Xvfb, debug
build, `--ipc-bench-runs 3 --ipc-bench-sizes 0,64,1024`): the run
completed with `transport=protocol_inline`, `answered_inline=96`,
`parked=0`, `replies_fallback=0`, `send_fallbacks=0`, and
`first_invoke_dispatched` / `first_invoke_responded` markers merged with
their protocol-thread timestamps. Every invoke was answered inside the
protocol callback; nothing parked. Not publishable; mechanism proof only.

Hosted `macos-latest`: pending (`controlled-performance` run on this
branch tip, `ipc-kiri-proto-inline.json` in the `perf-macos-latest` and
`perf-macos-latest-ipc` artifacts). Fill in same-run means for 262144 and
1048574 bytes versus `ipc-tauri.json` and `ipc-kiri-proto-ring.json` when
the run lands.

## Kill / continue recommendation

Pending hosted numbers. The mechanism is the one the protocol_ring kill
report named as the next experiment: if answering on the protocol thread
does not beat Tauri at 256 KiB and about 1 MiB, the residual gap is not
the parked-responder hop and this line of mechanism is done.
