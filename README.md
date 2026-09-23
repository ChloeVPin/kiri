# Kiri

Kiri is a Rust runtime for native desktop applications on Linux, macOS, and Windows. A native operation requires both a capability and a host-owned allowlist entry. The product contract is documented in [docs/PRODUCT.md](docs/PRODUCT.md).

## Repository map

- [crates/kiri-core](crates/kiri-core) contains the platform-neutral protocol, capability authority, resources, tracing, and validation.
- [crates/kiri-runtime](crates/kiri-runtime) contains the native host facade and platform backends.
- [baselines/](baselines/) contains comparison applications.
- [examples/](examples/) contains small frontends and diagnostics.
- [benchmark/](benchmark/) contains startup, IPC, and bulk-data tools.
- [docs/](docs/) contains product, API, architecture, and research documentation.

The host embeds a frontend at build time. A local run can override it with `--frontend DIR` or `KIRI_FRONTEND`.

## Build and verify

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
```

For the smoke host and cross-target checks, see [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md). Performance measurements belong in the research documents and should not be inferred from a build result.

## Documentation

- [Getting started](docs/GETTING_STARTED.md)
- [API reference](docs/API_REFERENCE.md)
- [Architecture decisions](docs/DECISIONS.md)
- [Roadmap](docs/ROADMAP.md)

## License

Kiri is available under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
