# Open Questions

Unresolved items, with the evidence needed to close them.

## Q-001: WebView2 runtime on `windows-latest` GitHub runners

Does a `windows-latest` runner have the WebView2 Evergreen runtime installed?
If not, the smoke CI job must install it (e.g. via the
`powerapps/install-webview2-win` action or `winget`).

- Needed evidence: Level A — first run of `.github/workflows/windows-host-smoke.yml`.
- Fallback: add an install step gated on a `WebView2 Runtime` check.

## Q-002: real Windows behavior of the direct host

The host is `cargo check`-clean against `x86_64-pc-windows-msvc` but has
never executed on Windows. Open items:

- `SetVirtualHostNameToFolderMapping` requires the folder to exist at the
  mapped path; verify the `--frontend` resolution works on a real machine.
- message loop, timer, and teardown ordering on real Windows (100-cycle
  stress run is the gate; expected on CI).
- QPC-based markers should be cross-checked against the WebView2
  `ProcessFailed`/`NavigationCompleted` ordering.

## Q-003: Tauri baseline IPC latency contribution

The Tauri baseline routes the `dom`/`frame` markers through
`__TAURI_INTERNALS__.invoke('kiri_marker')`, which costs more than the wry
`window.ipc.postMessage` path. Marker `since_first_ns` values therefore are
not directly comparable across the three targets for phases after
`bridge_ready`.

- Options: (a) document that only `webview_ready`-and-earlier phases are
  comparable; (b) add a Tauri-side injection script that uses the same
  `window.ipc` mechanism; (c) accept and record in the perf report.
- Needed evidence: Level A — measured run on the self-hosted runner.

## Q-004: `--frontend` path form on Windows

The smoke CI passes `examples/blank` (forward slashes). Windows accepts this
relative form from the runner's working directory; verify with the first
smoke run and document the canonical form (`PathScope::canonicalize` on
Windows will produce `C:\...` paths).

## Q-005: backpressure policy for the webview → host channel

T006 requires bounded IPC backpressure. Current design notes in
`docs/research/` assume a high-water mark plus drop policy; the exact
semantics (drop-newest vs. block vs. error frame) are not fixed.

- Needed evidence: Level A — WebView2 `WebMessageReceived` delivery model
  (postMessage is async; does the host side see a bounded queue?), plus a
  measured stress result.