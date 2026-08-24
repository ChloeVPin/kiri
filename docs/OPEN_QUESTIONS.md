# Open Questions

Unresolved items, with the evidence needed to close them.

## Q-001: WebView2 runtime on `windows-latest` GitHub runners (Windows direct backend only) — CLOSED

A `windows-latest` runner DOES have the WebView2 Evergreen runtime installed.
Verified by correctness run #19: the `Native smoke run (Win32 + WebView2
backend)` step passed and emitted all required startup markers, no install
step required.

- Evidence: Level A - `correctness.yml` `test (windows-latest)` run #19,
  `Native smoke run` + `Native stress run (100 cycles)` both green.

## Q-002: real Windows behavior of the direct Win32 + WebView2 backend

The host is `cargo check`-clean against `x86_64-pc-windows-msvc` but has
never executed on Windows. Open items:

- `SetVirtualHostNameToFolderMapping` requires the folder to exist at the
  mapped path; verify the `--frontend` resolution works on a real machine.
- message loop, timer, and teardown ordering on real Windows (100-cycle
  stress run is the gate; expected on CI).
- QPC-based markers should be cross-checked against the WebView2
  `ProcessFailed`/`NavigationCompleted` ordering.

## Q-003: Tauri baseline IPC latency contribution — RESOLVED (method)

The Tauri baseline routes the `dom`/`frame` markers through
`__TAURI_INTERNALS__.invoke('kiri_marker')`, which costs more than the wry
`window.ipc.postMessage` path. Marker `since_first_ns` values therefore are
not directly comparable across the three targets for phases after
`bridge_ready`.

- Resolution: the Tauri baseline now arms correctly (capability grant in
  `build.rs` `AppManifest::commands` + `capabilities/default.json`, plus a
  direct `invoke` in the injected `BRIDGE_SCRIPT`). Verified on macOS: all 9
  markers, exit 0.
- Decision: option (a) + (c). Only `webview_ready`-and-earlier phases are
  directly comparable across targets; the `dom`/`frame` delta is recorded
  explicitly in the T009 report rather than hidden. No attempt to force Tauri
  onto the wry `window.ipc` path (that is not how Tauri IPC works).

## Q-004: `--frontend` path form and resolution

The cross backend reads `index.html` from `HostOptions.frontend_dir` at
runtime and serves it over `kiri://localhost`; this is proven working on
macOS (native smoke + stress runs pass). On Windows the direct backend maps
the same `--frontend` directory via `SetVirtualHostNameToFolderMapping`;
verify with the first `windows-latest` smoke run and document the canonical
form (`PathScope::canonicalize` on Windows will produce `C:\...` paths).

## Q-005: backpressure policy for the webview → host channel

T006 requires bounded IPC backpressure. The native WebSocket transport now
uses bounded command and inbound queues: outbound saturation returns `busy`,
while newest inbound frames are dropped when the delivery queue is full. This
is a bounded transport policy, but it does not prove a bounded WebView2
`WebMessageReceived` queue.

- Needed evidence: Level A - WebView2 `WebMessageReceived` delivery model
  (postMessage is async; does the host side see a bounded queue?), plus a
  measured stress result.
