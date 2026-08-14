# Decisions

Records of architectural decisions. Evidence levels per corpus AGENTS.md.

## D-001: webview2-com 0.39.1 + windows 0.62 bindings for the direct host

- **Status**: decided (T001)
- **Context**: the direct WebView2 host needs shared-buffer interfaces (T008)
  that only the webview2-com line provides. wry 0.56.1 pins webview2-com 0.38 /
  windows 0.61.
- **Decision**: use `webview2-com` 0.39.1 with `windows`/`windows-core` 0.62.
  The direct host is intentionally independent of wry.
- **Evidence**: Level A - webview2-com 0.39.1 published sources on docs.rs
  (build dated 28 June 2026) and crates.io registry; windows-rs 0.62.2 source
  in the local registry.

## D-002: no WebView2Loader.dll copy / no build.rs on MSVC

- **Status**: decided (T001)
- **Context**: the first plan was to copy `WebView2Loader.dll` next to the
  binary at build time.
- **Decision**: dropped. webview2-com-sys 0.39.1's `link_webview2` macro links
  `WebView2LoaderStatic` (`kind = static`) on MSVC targets, so no DLL is
  needed. Revisit only if a non-MSVC Windows toolchain is ever required.
- **Evidence**: Level A - `webview2-com-sys-0.39.1` source in the local
  registry (`link_webview2` macro, `kind = "static"` for MSVC).

## D-003: baselines are standalone projects, not workspace members

- **Status**: decided (T001)
- **Context**: T001 requires a blank Tauri baseline and a minimal Wry/Tao
  baseline as independent comparators.
- **Decision**: `baselines/wry-tao` and `baselines/tauri` are separate
  packages with their own lockfiles, excluded from the workspace
  (`default-members = ["crates/kiri-core"]` keeps local builds fast). They
  must NOT depend on kiri-core.
- **Evidence**: Level A - workspace `Cargo.toml`, baseline manifests.

## D-004: virtual host mapping (https://app.local) instead of a custom scheme

- **Status**: decided (T001)
- **Context**: the host must serve the frontend from the local disk with a
  stable origin that behaves like a web origin (needed for later origin-based
  security checks).
- **Decision**: `SetVirtualHostNameToFolderMapping` with
  `COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW`, serving
  `https://app.local/index.html`. Constants: `VIRTUAL_HOST_NAME = "app.local"`,
  `FRONTEND_PAGE = "index.html"`.
- **Evidence**: Level A - webview2-com 0.39.1 `ICoreWebView2_3` bindings;
  Windows 11 WebView2 SDK docs.

## D-005: startup markers via the runtime's own QPC clock, shared schema

- **Status**: decided (T001)
- **Context**: benchmarks compare the direct host, wry/t-tao, and tauri on
  identical scenarios; wall-clock in the harness is too coarse for
  sub-millisecond startup phases.
- **Decision**: each target records 9 markers
  (`process_spawn_requested` … `first_animation_frame`) on its own monotonic
  clock and prints one JSON document (`schema_version: 1`) on stdout with
  per-marker `since_first_ns`. The direct host uses QPC (`QueryPerformanceCounter`);
  baselines use `Instant` since boot of their process. Smoke runs exit 0
  only after `first_animation_frame`, exit 2 on watchdog.
- **Evidence**: Level A - marker schema in `docs/research/markers-schema.md`
  (written from corpus `docs/12-benchmarks.md`).

## D-006: cross-platform host; direct Win32 + WebView2 backend on Windows,
wry/tao backend on macOS and Linux

- **Status**: decided (T001), superseded by D-009 (see note)
- **Context**: the original corpus rule was Windows-first: make the Windows
  path work first and only cross-check other platforms. That constraint was
  removed; the host now runs natively on every desktop platform.
- **Decision**: `kiri-runtime` is a platform-neutral facade. On Windows it
  uses the direct Win32 + WebView2 host (`host_windows.rs`); on macOS and
  Linux it uses a wry/tao host (`host_cross.rs`). Both record the same nine
  startup markers on a monotonic clock and obey the shared smoke/exit
  contract, so the benchmark compares like for like. The backend is selected
  by `cfg(target_os = "windows")` at the crate boundary; a `--backend`
  selector is reserved but not yet wired.
- **Evidence**: Level A - `rust-toolchain.toml`, CI workflows, crate
  `Cargo.toml` (`cfg` gating), `lib.rs` facade, native macOS smoke + stress
  runs (exit 0 with all nine markers; 0 stress failures).

## D-009: wry/tao cross backend serves the frontend over a custom
`kiri://localhost` protocol

- **Status**: decided (T001)
- **Context**: the cross backend needs a stable application origin on
  macOS/Linux so the shared blank frontend loads and posts markers the same
  way it does against the Windows virtual host mapping.
- **Decision**: serve `index.html` from `HostOptions.frontend_dir` over a
  custom `kiri` protocol at `kiri://localhost/index.html`, inject the same
  bridge script at document start, and capture `dom`/`frame` ready messages
  via `with_ipc_handler`. The event loop does not return on macOS, so the
  startup result is written and `std::process::exit` is called inside the
  loop. `kiri-host-stress` therefore spawns a fresh `kiri-host` subprocess
  per cycle for true launch-close isolation across all platforms.
- **Evidence**: Level A - wry 0.56.1 / tao 0.36.0 APIs (`with_custom_protocol`,
  `with_ipc_handler`, `EventLoop::run -> !`), native macOS smoke + stress
  runs.

## Open / deferred

- D-007 (open): WebView2 runtime availability on `windows-latest` runners -
  assume present, verify on first smoke run.
- D-008 (open): backpressure policy for the IPC bridge (T006) - recorded in
  `OPEN_QUESTIONS.md`.