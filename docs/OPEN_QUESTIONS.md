# Open Questions

Unresolved items, with the evidence needed to close them.

## Q-001: WebView2 runtime on `windows-latest` GitHub runners (Windows direct backend only) — CLOSED

A `windows-latest` runner DOES have the WebView2 Evergreen runtime installed.
Verified by correctness run #19: the `Native smoke run (Win32 + WebView2
backend)` step passed and emitted all required startup markers, no install
step required.

- Evidence: Level A - `correctness.yml` `test (windows-latest)` run #19,
  `Native smoke run` + `Native stress run (100 cycles)` both green.

## Q-002: real Windows behavior of the direct Win32 + WebView2 backend — MOSTLY CLOSED

Superseded by hosted `correctness` on `windows-latest`: native smoke, stress,
and `examples/menu-smoke` exercise the Win32 + WebView2 host (see
`docs/CROSS_PLATFORM_STATUS.md` and `docs/STATUS.md`). The old claim that the
host "has never executed on Windows" is false.

Remaining nuance (optional follow-ups, not blockers for "runs on Windows"):

- Document the canonical `--frontend` absolute path form on Windows
  (`PathScope::canonicalize` / lexical absolute helpers in `host_windows.rs`).
- Keep watching QPC marker ordering vs WebView2 `ProcessFailed` /
  `NavigationCompleted` if a future marker schema change needs it.
- Embedded and disk frontends are both served via `WebResourceRequested`
  (`handle_app_resource` → `serve_embedded` / `serve_checked`); folder mapping
  is not required for the packed path.

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

## Q-004: `--frontend` path form and resolution — MOSTLY CLOSED

Cross-backend `kiri://` + `--frontend` is proven on macOS (smoke/stress).
Windows hosted correctness also green: the direct host serves app-origin
assets through `WebResourceRequested` (`serve_checked` for a disk root,
`serve_embedded` when no `--frontend`). Remaining doc-only follow-up: publish
the canonical Windows absolute path form in getting-started notes.

## Q-005: backpressure policy for the webview → host channel

T006 requires bounded IPC backpressure. The native WebSocket transport now
uses bounded command and inbound queues: outbound saturation returns `busy`,
while newest inbound frames are dropped when the delivery queue is full. This
is a bounded transport policy, but it does not prove a bounded WebView2
`WebMessageReceived` queue.

- Needed evidence: Level A - WebView2 `WebMessageReceived` delivery model
  (postMessage is async; does the host side see a bounded queue?), plus a
  measured stress result.
