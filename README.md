<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/kiri.svg">
  <source media="(prefers-color-scheme: light)" srcset="assets/kiri-dark.svg">
  <img alt="Kiri logo" src="assets/kiri.svg" width="160">
</picture>

# Kiri

**A small cross-platform desktop runtime that keeps platform control native and measures the cost of every abstraction.**

</div>

Kiri is an experiment in desktop runtime design. Windows uses a direct Win32 + WebView2 host; macOS and Linux use wry/tao. Both paths share the same platform-neutral control plane, capability model, resource ownership rules, and startup-marker schema.

The project tests a simple hypothesis: direct platform ownership can reduce overhead without weakening the security boundary. That hypothesis is measured against Wry/Tao and Tauri baselines rather than treated as a foregone conclusion.

## Why Kiri exists

- **Native ownership where it matters.** Windows hosts WebView2 directly instead of routing through a general-purpose desktop wrapper.
- **One control plane.** Platform-specific transport stays behind a shared Rust protocol and resource model.
- **Capabilities stay native.** Caller identity, permissions, bounds checks, and resource ownership are assigned and enforced outside JavaScript.
- **Performance is measured.** Startup, IPC, and memory comparisons use the same marker schema across Kiri and its baselines.

## Current status

Kiri runs natively on Windows, macOS, and Linux from one workspace. The direct Win32 + WebView2 host is exercised on Windows; the wry/tao host runs on macOS and Linux. Correctness CI covers formatting, linting, tests, platform builds, and native smoke contracts.

The runtime also includes a capability-gated diagnostics surface, window controls, clipboard access, path and OS queries, and bounded HTTP access. Benchmark work remains evidence-driven: if a simpler baseline performs as well, the project should record that result rather than preserve the original hypothesis.

## Architecture

| Area | Responsibility |
| --- | --- |
| `crates/kiri-core` | Platform-neutral protocol, security authority, resource table, tracing, and shared models |
| `crates/kiri-runtime` | Native host facade with Win32/WebView2 on Windows and wry/tao on macOS and Linux |
| `examples/` | Small frontends used for smoke tests and diagnostics |
| `benchmark/` | Shared startup harness, marker schema inputs, and result tooling |
| `baselines/` | Standalone Wry/Tao and Tauri comparators using the same measurement contract |

The application frontend is served from a trusted application origin. Remote navigation is denied, and JavaScript never assigns its own native identity or capability set.

## Build and verify

From the repository root:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p kiri-runtime --bins
```

On macOS or Linux, run the native smoke contract with:

```sh
./target/debug/kiri-host --smoke \
  --frontend examples/blank \
  --markers-out artifacts/startup.json
```

On Windows:

```powershell
cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host.exe --smoke `
  --frontend examples/blank `
  --markers-out artifacts/startup.json
```

## Benchmarking

Kiri, Wry/Tao, and Tauri use the same startup-marker vocabulary so comparisons stay explicit and reproducible.

```sh
python benchmark/harness.py \
  --name startup-kiri \
  --runs 20 \
  --output artifacts/startup-kiri.json \
  -- ./target/release/kiri-host --smoke --frontend examples/blank
```

Startup markers include native entry, platform initialization, webview readiness, bridge readiness, DOM readiness, application readiness, and first animation frame.

## Security model

Kiri keeps a narrow boundary between web content and native capabilities:

- the application origin is trusted and remote navigation is denied;
- native code assigns caller identity and capability authority;
- filesystem, clipboard, window, path, OS, and HTTP operations route through host-owned controllers;
- resource bounds, ownership checks, origin checks, and backpressure are not relaxed for benchmark results;
- network access is capability-gated and host-allowlisted rather than implicitly open.

## Documentation

- [Architecture decisions](docs/DECISIONS.md)
- [Open questions](docs/OPEN_QUESTIONS.md)
- [Startup marker schema](docs/research/markers-schema.md)
- [Diagnostics and observability](docs/13-diagnostics-observability.md)
- [Repository operating notes](AGENTS.md)

## Scope

Kiri is a focused runtime experiment, not a claim that every desktop application should own its platform integration directly. Its purpose is to make the tradeoffs measurable: startup cost, IPC behavior, memory, implementation complexity, and security boundaries should be visible enough to compare.
