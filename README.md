<div align="center">
  <img src="assets/kiri.svg" alt="Kiri logo" width="144" />

  <h1>Kiri</h1>

  <p>A native desktop app runtime for Linux, macOS, and Windows.</p>

  <p>
    <a href="https://github.com/ChloeVPin/kiri/actions/workflows/correctness.yml"><img src="https://github.com/ChloeVPin/kiri/actions/workflows/correctness.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/ChloeVPin/kiri/releases/latest"><img src="https://img.shields.io/github/v/release/ChloeVPin/kiri?label=latest%20release" alt="Latest release" /></a>
    <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey" alt="Linux | macOS | Windows" />
  </p>
</div>

Kiri is a desktop runtime for applications where a granted capability must not
be enough. Native calls are double-gated by a capability bit and a host-owned
allowlist. The product goal and acceptance criteria are in
[`docs/PRODUCT.md`](docs/PRODUCT.md).

The README describes the stable product, architecture, and usage. Changing
measurements, open questions, and planned work are maintained in
[`docs/ROADMAP.md`](docs/ROADMAP.md) and the linked research documents.

## Architecture

Three layers share one control protocol and startup-marker schema:

```
crates/kiri-core       platform-neutral protocol, security authority,
                       resources, tracing, and validation
crates/kiri-runtime    native host facade and platform backends
baselines/             standalone Wry/Tao and Tauri comparison apps
examples/              blank, starter, demo, and diagnostics frontends
benchmark/             startup, IPC, and bulk-data measurement tools
docs/                  product, API, architecture, and research documentation
```

The runtime selects a platform-native host automatically. Linux and macOS use
the wry/tao backend; Windows uses the Win32/WebView2 backend. Both implement
the same origin checks, native caller identity, capability authority, resource
ownership, and control protocol.

The shared startup contract records nine monotonic markers and exits 0 after
`first_animation_frame` in smoke mode, or 2 when the watchdog expires. The
schema is documented in [`docs/research/markers-schema.md`](docs/research/markers-schema.md).

## Build and verify

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
./target/debug/kiri-host-stress --frontend examples/blank --cycles 3
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

The host runs natively on the current operating system. The Windows target can
also be cross-checked from Linux or macOS with the installed MSVC target.

## Run an application

The host packs a frontend at compile time. `--frontend DIR` overrides the
frontend for local runs and benchmarks. `KIRI_FRONTEND` is also supported.

```sh
cargo build -p kiri-runtime --bin kiri-host
./target/debug/kiri-host --smoke

# Build and run the interactive demo.
KIRI_EMBED_FRONTEND="$PWD/examples/demo" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host

# Scaffold an application without owning this repository.
./tools/create-kiri-app.sh ~/Desktop/my-kiri-app
```

See [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) for application
scaffolding, frontend bridges, packaging, and release usage.

## Documentation

- [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md): build and run an app
- [`docs/API_REFERENCE.md`](docs/API_REFERENCE.md): control-plane commands
- [`docs/PRODUCT.md`](docs/PRODUCT.md): product goal and acceptance criteria
- [`docs/ROADMAP.md`](docs/ROADMAP.md): planned work and product gaps
- [`docs/DECISIONS.md`](docs/DECISIONS.md): architecture decisions
- [`docs/research/markers-schema.md`](docs/research/markers-schema.md): marker contract

## Project rules

- Keep Linux, macOS, and Windows as equal desktop targets.
- Keep platform transport behind narrow interfaces.
- Enforce origin, capability, bounds, ownership, and backpressure checks.
- Do not turn an unmeasured hypothesis into a performance claim.
