# Contributing to Kiri

Thanks for considering a contribution. Kiri aims to be **correct before fast**
and **secure before broad**.

## Ground Rules

- Read `AGENTS.md` first — it is the operating contract (verification gates,
  evidence levels, corpus authority).
- Keep Linux, macOS, and Windows equal. Do not add Windows-only or
  macOS-only features without a tracked decision in `docs/DECISIONS.md`.
- Never remove capability checks, origin checks, bounds checks, ownership
  checks, or backpressure to improve a benchmark number.
- No `unsafe` without a decision entry and review.
- Follow `rustfmt` and `clippy -D warnings` — CI enforces them.

## Development Setup

```sh
git clone https://github.com/ChloeVPin/kiri.git
cd kiri
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings
```

`rust-toolchain.toml` pins the toolchain to 1.97. Baselines have their own
lockfiles and must stay standalone (never depend on `kiri-core`).

## Verification Gates (run before committing)

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
./target/debug/kiri-host-stress --frontend examples/blank --cycles 3
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings
cargo clippy -p kiri-runtime --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

CI (`correctness.yml`) runs the same matrix on `ubuntu-latest`,
`macos-latest`, and `windows-latest`.

## Commit Messages

Imperative, one-line summary plus body, reference task IDs:

```
T009: stabilize Wry/Tao warmup to 45s symmetric
```

See `docs/PRODUCT.md` for the product goal and `docs/ROADMAP.md` for
planned work. The corpus at `kiri-agent-execution-corpus/` is authoritative
for specs (see `docs/research/README.md`).

## Pull Requests

- Keep platform transport behind narrow interfaces; the control protocol is
  logical, not tied to one WebView engine.
- Evidence levels: **A** = vendor docs / measured local result,
  **B** = maintained implementation source, **C** = issue tracker / forum,
  **D** = inference. Architecture decisions affecting security or performance
  need **A** or an explicit local experiment.
- Stop and open a decision item if: platform API behavior contradicts a spec,
  a security boundary becomes ambiguous, a benchmark moves >10% without cause,
  an ABI change would break a published plugin contract, or a required platform
  API is missing on the declared support floor.
- Run `benchmark/harness.py` for performance claims and retain artifacts;
  do not quote `bulk_bench` as IPC.

## Reporting Issues

Use the issue templates. Include OS, Kiri version (`Cargo.toml`), backend
(`wry/tao` or `Win32/WebView2`), and reproduction steps with markers output
if applicable.

## License

By contributing, you agree that your contributions will be licensed under the
same dual license as Kiri: **MIT OR Apache-2.0**.
