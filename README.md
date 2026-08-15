<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/kiri.svg">
  <source media="(prefers-color-scheme: light)" srcset="assets/kiri-dark.svg">
  <img alt="Kiri logo" src="assets/kiri.svg" width="180" align="center">
</picture>

<h1 align="center">Kiri</h1>

<p align="center">
  Cross-platform native desktop app runtime for Windows, macOS, and Linux &middot; direct WebView2 host on Windows, wry/tao host on macOS and Linux &middot; platform-neutral control-plane core &middot; Tauri and Wry baselines measured on a shared startup-marker schema
</p>

</div>

Kiri tests the hypothesis that a hand-rolled WebView2 host with a thin
control protocol beats general-purpose wrappers (Tauri, Wry) on startup time,
IPC latency, and memory, while keeping the same security boundaries. The
corpus rule is explicit: keep direct platform ownership as a hypothesis. If
Wry or Tao is as fast and simpler, record that result and switch.

## Status

Tasks T001-T010 are complete and committed. The runtime runs natively on
every desktop platform from one codebase: the direct Win32 + WebView2 backend
on Windows and the wry/tao backend on macOS and Linux. Both backends enforce
the same security boundary (application-origin trust, native-assigned caller
identity and capability authority). All three platforms are equal targets. The wry/tao backend runs natively on macOS and Linux (smoke and stress), and the direct Win32 + WebView2 backend runs natively on Windows (smoke and stress). All three platforms (Windows, macOS, Linux) are equal targets; the macOS dev machine exercises the native wry/tao backend locally while Windows and Linux are exercised by CI and cross-checks.

- 225 tests pass (cargo test --workspace: 198 kiri-core + 2 integration + 25 kiri-runtime)
- control-plane ping + trace (T003) and caller/capability authority (T004)
  implemented; 10k-ping latency distribution emitted
- wry/tao cross backend runs natively on macOS: `kiri-host --smoke` records
  all nine markers and exits 0; `kiri-host-stress` passes multi-cycle
- direct Win32 + WebView2 host cross-checks clean on `x86_64-pc-windows-msvc`
  (`cargo check` and `cargo clippy -D warnings`, zero warnings)
- developer diagnostics panel (T010) ships: a `kiri.diag` command returns a
  privacy-safe runtime snapshot (backend, runtime version, open-resource count,
  recent-request latency waterfall); the `examples/panel` frontend renders it
- capability-gated `kiri.window.*` control surface (ids 14-22) implemented across
  both backends; every window operation is authorized by the central capability
  authority and routed through a host-owned controller, so JS never reaches the
  native handle (exceeds Tauri's window module on the security axis)
- capability-gated kiri.clipboard read/write (ids 23-24) implemented across both
  backends; clipboard access requires the CLIPBOARD capability bit and flows through a
  host-owned ClipboardController (arboard on macOS/Linux/Windows), so JS never touches
  the OS clipboard directly (exceeds Tauri's clipboard plugin on the security axis)
- capability-gated `kiri.path.*` / `kiri.os.*` (ids 25-37) implemented across both
  backends; path math (dirname/basename/extname/stem/join/isAbsolute) and read-only OS
  directory discovery (home/temp/app config|data|cache/document/app dir) are behind the
  PATH capability bit and never expose env vars or filesystem roots to JS (exceeds Tauri's
  path/os plugins on the security axis: Tauri grants them by default)
- capability-scoped `kiri.http.get` (id 38) implemented across both backends; fetches are
  behind the HTTP capability bit AND a host allowlist (default-deny), with responses bounded
  by the same bulk-object ceiling as `kiri.fs`. Tauri's `http` plugin allows arbitrary
  fetches by default; Kiri refuses any host not on the explicit allowlist (exceeds Tauri's
  http plugin on the security axis). Transport is a trait seam (`HttpClient`); the seed
  `StdHttpClient` does loopback/plaintext for tests, a TLS client slots in unchanged.
- Wry/Tao and Tauri baselines compile clean
- the direct Win32 + WebView2 host runs natively on real Windows
  (`windows-latest` CI hard gate): native smoke + 100-cycle stress pass (Q-001 closed)

## Architecture

Three layers, one shared marker schema:

```
crates/kiri-core               platform-neutral logical protocol, security
                               authority, resource table, tracing. Pure Rust,
                               zero platform deps, 72 tests
crates/kiri-runtime            the native host. Platform-neutral facade
                               (`lib.rs`) dispatches to a direct Win32 +
                               WebView2 backend on Windows
                               (`host_windows.rs`) and a wry/tao backend on
                               macOS/Linux (`host_cross.rs`). Bins: kiri-host
                               (smoke runner), kiri-host-stress (launch-close
                               loop)
baselines/wry-tao              standalone comparator: tao 0.36 + wry 0.56,
                               custom protocol wry://localhost
baselines/tauri                standalone comparator: tauri 2.11,
                               invoke command kiri_marker
examples/blank                 shared blank frontend with a 3-way bridge
                               adapter (native / wry / tauri)
benchmark/                     harness.py + test-vectors.json (from corpus)
```

The direct host serves the frontend over `SetVirtualHostNameToFolderMapping`
at `https://app.local/index.html` and records nine startup markers on a QPC
monotonic clock: `process_spawn_requested`, `native_entry`,
`platform_initialized`, `webview_creation_requested`, `webview_ready`,
`bridge_ready`, `dom_ready`, `app_ready`, `first_animation_frame`. Smoke
runs exit 0 after `first_animation_frame`, exit 2 on watchdog. Schema:
`docs/research/markers-schema.md`.

Bindings: `webview2-com` 0.39.1 with `windows` 0.62 for the direct host (the
only line that exposes the shared-buffer interfaces needed by T008), kept
deliberately independent of wry, which pins an older API generation.

## Repository layout

```
.github/workflows/            correctness (all-OS hard gates),
                              controlled-performance (self-hosted runner)
baselines/                    standalone comparators, own lockfiles
benchmark/                    harness.py, test-vectors.json, README
crates/kiri-core/             11 modules, 72 tests
crates/kiri-runtime/         lib.rs (facade), host_windows.rs (WebView2),
                              host_cross.rs (wry/tao), markers.rs, output.rs,
                              bin/kiri-host, bin/kiri-host-stress
examples/panel/               developer diagnostics frontend (T010)
docs/                         DECISIONS.md, OPEN_QUESTIONS.md,
                              research/markers-schema.md,
                              13-diagnostics-observability.md
examples/blank/               shared frontend (T001-T004)
examples/panel/                developer diagnostics frontend (T010)
```

The execution corpus (`kiri-agent-execution-corpus/`, gitignored) is the
authoritative source of specs, docs, and the machine-readable task queue
(`agent/task_queue.json`). It is linked from `docs/research/README.md`.

## Build and verify

The dev machine runs macOS (aarch64); the Windows direct host cannot execute
locally, so it is validated by the cross-target compile. The wry/tao backend
runs natively here and is exercised by the smoke and stress runs. Local gates:

```sh
cargo test --workspace                       # 45 tests
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins           # native host (macOS/Linux)
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
./target/debug/kiri-host-stress --frontend examples/blank --cycles 3
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

On Windows, the same workspace checks run natively plus the smoke contract:

```sh
cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host.exe --smoke --frontend examples/blank --markers-out artifacts/startup.json
./target/release/kiri-host-stress.exe --cycles 100 --frontend examples/blank
```

On macOS/Linux the host runs natively with the wry/tao backend:

```sh
cargo build -p kiri-runtime --bin kiri-host
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out artifacts/startup.json
./target/debug/kiri-host-stress --cycles 100 --frontend examples/blank
```

Baselines follow the same smoke contract (exit 0 + one JSON document on
stdout, exit 2 on watchdog) so the harness can drive all three:

```sh
python benchmark/harness.py --name startup-kiri --runs 20 --output artifacts/startup-kiri.json \
  -- ./target/release/kiri-host --smoke --frontend examples/blank
```

## CI

- `correctness`: fmt, clippy, tests on windows/macos/ubuntu; the Windows
  runtime is cross-checked on non-Windows runners
- `windows-host-smoke`: smoke run with marker verification plus a 100-cycle
  launch-close stress run, on `windows-latest`
- `controlled-performance`: manual dispatch on a self-hosted
  `[self-hosted, windows, x64, kiri-perf]` runner; startup benchmarks for all
  three targets, artifacts uploaded

## Roadmap

Task queue is maintained in the corpus (`agent/task_queue.json`, status
mirrored in this repo's docs):

- T001-T007 done (bootstrap, lifecycle, control-plane, authority, codegen,
  resource table, bulk benchmark)
- T008 WebView2 read-only shared-buffer path (blocked: needs real Windows)
- T009 direct host versus Wry/Tao versus Tauri comparison (blocked: needs T008 +
  self-hosted perf hardware)
- T010 minimal diagnostics panel (done, macOS-runnable)

## Documentation

- `docs/DECISIONS.md`: architectural decisions with evidence levels
  (webview2-com 0.39.1, no DLL copy on MSVC, standalone baselines, virtual
  host mapping, marker schema, cross-platform backends)
- `docs/OPEN_QUESTIONS.md`: unresolved items and the evidence needed to close
  them (WebView2 runtime on CI runners, first real Windows run, Tauri IPC
  marker comparability, backpressure policy)
- `docs/research/markers-schema.md`: the shared measurement contract

## Agent operations

`AGENTS.md` at the repository root carries the operating contract for
agents working on this repo: read the relevant spec before editing, evidence
levels, stop conditions, the cross-platform verification contract, and the
exact verification commands. `docs/HANDOFF` style handoffs are written into the corpus
(`agent/HANDOFF.md`).

## Rules of the project

- The host runs natively on every desktop platform (Windows via the direct
  Win32 + WebView2 backend, macOS and Linux via the wry/tao backend). Keep
  platform transport behind narrow interfaces; do not let one engine's
  quirks leak into the shared control protocol.
- Every backend enforces the same security boundary: the application origin is
  trusted, remote navigation is denied, and caller identity plus capability
  authority are assigned by native code only (never by JavaScript).
- Never optimize away capability checks, origin checks, bounds checks,
  ownership checks, or backpressure to produce a better benchmark.
- Never convert a hypothesis into a fact because an implementation seems
  plausible. Record environment and failure mode for every failed
  experiment.
- No emojis or decorative prose in docs, no unmeasured performance claims.
