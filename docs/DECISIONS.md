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

## D-004: custom scheme `kiri://localhost` on every OS

- **Status**: superseded T001 virtual-host decision (2026-08-17)
- **Context**: `https://app.local` paid a ~2s Windows LLMNR/mDNS tax before
  first paint. `https://app.localhost` removed the tax but still started
  Chromium's HTTPS network service.
- **Decision**: register `kiri` via `ICoreWebView2EnvironmentOptions4` and
  navigate to `kiri://localhost/index.html`. Serve assets with
  `WebResourceRequested`. Same origin as the wry/tao backend.
- **Evidence**: hosted `windows-latest` 20-run medians: `app.local` 2811 ms
  process / 2482 ms `webview_ready`; `app.localhost` 846 / 511 ms. Custom
  scheme is the remaining network-stack cut.

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

## D-006: cross-platform host with native backends on Linux, macOS, and Windows

- **Status**: decided (T001), superseded by D-009 (see note)
- **Context**: the host runs natively on every desktop platform from one
  codebase, and all three platforms (Linux, macOS, Windows) are equal targets.
  The macOS development machine is the
  day-to-day verification target for the cross (wry/tao) backend, while the
  Windows direct backend is cross-checked and CI-run.
- **Decision**: `kiri-runtime` is a platform-neutral facade. On Windows it
  uses a wry/tao host (`host_cross.rs`) on macOS and Linux and the direct
  Win32 + WebView2 host (`host_windows.rs`) on Windows. Both record the same nine
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


## D-010: shared security policy, identical trust boundary on every backend

- **Status**: decided (T004)
- **Context**: the Windows direct backend enforced an application-origin trust
  boundary (`is_app_origin_url`) and native-assigned caller identity/capability
  authority. The wry/tao cross backend did not, so the two backends did not
  share an equal security posture.
- **Decision**: move the origin/navigation/capability policy into
  `kiri-core::security` (`is_app_origin`, `is_navigation_allowed`,
  `trusted_frontend_capabilities`) and apply it from both backends. The cross
  backend now blocks remote navigation (`with_navigation_handler`) and rejects
  IPC whose document URL is not the application origin, matching the Windows
  gate. Caller identity and capability mask are assigned by native code only;
  JavaScript never supplies them.
- **Evidence**: Level A - `kiri-core/src/security.rs`, `host_cross.rs`
  (navigation + IPC origin gate), `host_windows.rs` (`is_app_origin_url`),
  `dispatch.rs` (capability check before execution); native macOS smoke run
  records all nine markers and exits 0 with the gate active.

## Open / deferred

- D-007 (closed): WebView2 runtime availability on `windows-latest` was
  verified by native smoke and stress runs, including the shared-buffer path.
- D-008 (open): backpressure policy for the IPC bridge (T006) - recorded in
  `OPEN_QUESTIONS.md`.
