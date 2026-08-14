# Kiri

Windows-first native desktop app runtime: a direct Win32 plus WebView2 host,
a platform-neutral control-plane core, and Tauri/Wry baselines measured on a
shared startup-marker schema.

Kiri tests the hypothesis that a hand-rolled WebView2 host with a thin
control protocol beats general-purpose wrappers (Tauri, Wry) on startup time,
IPC latency, and memory, while keeping the same security boundaries. The
corpus rule is explicit: keep direct platform ownership as a hypothesis. If
Wry or Tao is as fast and simpler, record that result and switch.

## Status

T001 in progress. Locally green on every gate that can run without Windows:

- 45 kiri-core tests pass (`cargo test --workspace`)
- direct host cross-checks clean on `x86_64-pc-windows-msvc`
  (`cargo check` and `cargo clippy -D warnings`, zero warnings)
- Wry/Tao and Tauri baselines compile clean
- the host skeleton has never executed on real Windows; that validation is
  the gate for closing T001/T002 acceptance, gated on the
  `windows-host-smoke` CI workflow

## Architecture

Three layers, one shared marker schema:

```
crates/kiri-core               platform-neutral logical protocol, security
                               authority, resource table, tracing. Pure Rust,
                               zero platform deps, 45 tests
crates/kiri-runtime-windows    direct Win32 window + ICoreWebView2 host.
                               Windows-only. Bins: kiri-host (smoke runner),
                               kiri-host-stress (launch-close loop)
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
.github/workflows/            correctness, windows-host-smoke,
                              controlled-performance (self-hosted runner)
baselines/                    standalone comparators, own lockfiles
benchmark/                    harness.py, test-vectors.json, README
crates/kiri-core/             10 modules, 45 tests
crates/kiri-runtime-windows/  host.rs, startup.rs, markers.rs, main,
                              kiri-host-stress
docs/                         DECISIONS.md, OPEN_QUESTIONS.md,
                              research/markers-schema.md
examples/blank/               shared frontend
```

The execution corpus (`kiri-agent-execution-corpus/`, gitignored) is the
authoritative source of specs, docs, and the machine-readable task queue
(`agent/task_queue.json`). It is linked from `docs/research/README.md`.

## Build and verify

The machine runs macOS; the Windows runtime cannot execute locally, so the
local gate is the cross-target compile:

```sh
cargo test --workspace                       # 45 tests
cargo fmt --all -- --check
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime-windows --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime-windows --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

On Windows, the same workspace checks run natively plus the smoke contract:

```sh
cargo build --release -p kiri-runtime-windows --bin kiri-host
./target/release/kiri-host.exe --smoke --frontend examples/blank --markers-out artifacts/startup.json
./target/release/kiri-host-stress.exe --cycles 100 --frontend examples/blank
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

- T001 bootstrap and baselines (in progress)
- T002 direct host lifecycle (window, local origin, markers, stress loop)
- T003 control-plane ping and request tracing
- T004 native caller identity and capability authority
- T005 deterministic command codegen and static routing
- T006 generational resource table and file resource
- T007 ordinary-message bulk path benchmark
- T008 WebView2 read-only shared-buffer path
- T009 direct host versus Wry/Tao versus Tauri comparison
- T010 minimal diagnostics panel

## Documentation

- `docs/DECISIONS.md`: architectural decisions with evidence levels
  (webview2-com 0.39.1, no DLL copy on MSVC, standalone baselines, virtual
  host mapping, marker schema, Windows-first)
- `docs/OPEN_QUESTIONS.md`: unresolved items and the evidence needed to close
  them (WebView2 runtime on CI runners, first real Windows run, Tauri IPC
  marker comparability, backpressure policy)
- `docs/research/markers-schema.md`: the shared measurement contract

## Agent operations

`AGENTS.md` at the repository root carries the operating contract for
agents working on this repo: read the relevant spec before editing, evidence
levels, stop conditions, Windows-first rule, and the exact verification
commands. `docs/HANDOFF` style handoffs are written into the corpus
(`agent/HANDOFF.md`).

## Rules of the project

- Windows path works first. Do not expand to macOS/Linux to avoid a hard
  Windows problem.
- Never optimize away capability checks, origin checks, bounds checks,
  ownership checks, or backpressure to produce a better benchmark.
- Never convert a hypothesis into a fact because an implementation seems
  plausible. Record environment and failure mode for every failed
  experiment.
- No emojis or decorative prose in docs, no unmeasured performance claims.
