# Kiri

A Rust desktop runtime for **Linux, macOS, and Windows**: a smaller native host
plus a web UI, where a granted capability is **not** enough.

JavaScript can only reach what the host named twice: a **capability bit** and a
**host-owned allowlist** (host, command, path, template, channel, or scheme).
Tauri often treats the capability as sufficient. That difference is the product.

Same OS WebView engines as Tauri (WebKit / WKWebView / WebView2). We do **not**
claim faster rendering. The winnable edges are the control plane, footprint, and
a narrower default surface.

> **Maturity:** Kiri is a **working runtime with a demo**, not a finished Tauri
> replacement. Acceptance criteria and what we refuse to claim live in
> [`docs/PRODUCT.md`](docs/PRODUCT.md).

## Why leave Tauri (when you should)

| Edge | Honest status |
|------|----------------|
| Double-gated native ops | Product bet: capability **and** allowlist |
| Smaller unstripped host | ~3.6–4.4× vs Tauri baseline on hosted runners; “3.7×” matches the About claim (see scoreboard) |
| Through-webview IPC | Often faster in published tables; **not** every payload |
| Startup | **Not** a universal win; hosted medians flip |
| Plugin zoo / mobile / store signing | Tauri wins today |

Full tables, run ids, and caveats:
[`docs/VS_TAURI.md`](docs/VS_TAURI.md) (short) ·
[`docs/COMPETITIVE_ANALYSIS.md`](docs/COMPETITIVE_ANALYSIS.md) (authoritative scoreboard).

Migrate mapping: [`docs/TEMPLATE_MIGRATION_TAURI.md`](docs/TEMPLATE_MIGRATION_TAURI.md).

## Try it (no clone)

Published host: **[v0.1.6](https://github.com/ChloeVPin/kiri/releases/tag/v0.1.6)**
(workspace `Cargo.toml` may already say `0.1.7`; that is source, not a download).

Release assets today: `darwin-aarch64`, `linux-x86_64`, `windows-x86_64`
(Intel Mac and Linux ARM are **not** published).

```sh
curl -fsSL https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.sh | bash -s ~/Desktop/my-kiri-app
```

Windows PowerShell and first-run friction (Gatekeeper / SmartScreen / `run.sh` /
`run.cmd`) are documented in [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md).
Archives are Ed25519-checked via `RELEASES.json` but **not** Apple-notarized or
Authenticode-signed.

There is no crates.io / npm / brew install path yet; curl|bash (or cloning) is
the current evaluation path.

## Build from this repo

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
```

Smoke host and cross-target checks: [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md).
Do not infer performance from a local build; cite the scoreboard.

The host embeds a frontend at build time (`KIRI_EMBED_FRONTEND`). A local run
can override with `--frontend DIR` or `KIRI_FRONTEND`. Scaffolded apps use a
sidecar `frontend/` folder (no recompile); compile-time embed still needs a
source checkout + Rust. See Getting Started §3.

## Repository map

- [`crates/kiri-core`](crates/kiri-core): protocol, capability authority, resources, tracing, validation
- [`crates/kiri-runtime`](crates/kiri-runtime): native host facade and platform backends
- [`baselines/`](baselines/): comparison applications (Tauri, Wry/Tao)
- [`examples/`](examples/): small frontends and diagnostics
- [`benchmark/`](benchmark/): startup, IPC, and bulk-data tools
- [`docs/`](docs/): product, API, architecture, and research documentation

## Documentation

- [Getting started](docs/GETTING_STARTED.md)
- [Product contract](docs/PRODUCT.md)
- [Kiri vs Tauri (short)](docs/VS_TAURI.md)
- [Competitive scoreboard](docs/COMPETITIVE_ANALYSIS.md)
- [API reference](docs/API_REFERENCE.md)
- [Architecture decisions](docs/DECISIONS.md)
- [Roadmap](docs/ROADMAP.md)
- [Status](docs/STATUS.md)

## License

Kiri is available under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
