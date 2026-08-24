# AGENTS.md

Operating contract for agents working in this repository. Read this before
doing anything. The corpus files, where referenced, take precedence over
this summary.

## Project

Kiri is a cross-platform native desktop app runtime. The product goal is
`docs/PRODUCT.md`. Platform-native hosts on Linux, macOS, and Windows are compared
against Tauri and Wry/Tao baselines on a shared startup-marker schema and a
control-plane protocol defined in the corpus
(`kiri-agent-execution-corpus/`, gitignored, see `docs/research/README.md`).
The host runs natively on every desktop platform. The Win32/WebView2 backend is
Windows-only; the wry/tao backend covers macOS and Linux.

The direct platform ownership is a hypothesis to be tested, not a goal. If a
baseline matches or beats the direct host on the measured contract, record
that result and prefer the simpler path.

## Environment facts

- Host machine: macOS (aarch64), Rust 1.97 stable via `rust-toolchain.toml`.
- `x86_64-pc-windows-msvc` target is installed locally.
- Windows binaries cannot execute on this machine. The Windows runtime is
  validated by compile cross-checks locally and by the
  `correctness` CI workflow on real Windows.
- The corpus is authoritative: specs, task queue, evidence policy. Do not
  edit corpus docs except task status and handoff notes.
- Nothing is committed until a coherent unit is done and every gate is green.

## Verification gates (run all before committing)

```sh
cargo test --workspace                                  # 220 core + 54 runtime + 2 integration tests
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins                      # native host (macOS/Linux)
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
./target/debug/kiri-host-stress --frontend examples/blank --cycles 3
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings
cargo clippy -p kiri-runtime --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

The host runs natively on macOS/Linux via the wry/tao backend, so its clippy
and smoke/stress runs are part of the local gate. Do not run
`cargo clippy --workspace --all-targets`: the Windows direct backend has no
`main` off Windows, so it fails with E0601 by design; it is checked with the
per-target commands above. CI handles Windows-native runs with per-OS steps.

Baselines have their own lockfiles and must stay standalone: they never
depend on `kiri-core`.

## Code rules

- Read the relevant corpus spec before editing code.
- Keep platform transport behind narrow interfaces; the control protocol is
  logical, not tied to one WebView engine.
- Never remove capability checks, origin checks, bounds checks, ownership
  checks, or backpressure to improve a benchmark number.
- Do not add comments unless asked. Follow existing style (`rustfmt`).
- Windows-only code lives in `crates/kiri-runtime/src/host_windows.rs`
  (`#![cfg(target_os = "windows")]` at the module boundary) and is selected by the
  `kiri-runtime` facade through `cfg(target_os = "windows")`; the wry/tao backend in
  `host_cross.rs` covers every other platform.
- Evidence levels: A = vendor docs/source/standard or measured local result,
  B = maintained implementation source, C = issue tracker/forum, D =
  inference. Architecture decisions affecting security or performance
  claims need A or an explicit local experiment.
- Stop and open a decision item if: platform API behavior contradicts a
  spec, a security boundary becomes ambiguous, a benchmark moves more than
  10 percent without an identified cause, an ABI change would break a
  published plugin contract, or a required platform API is missing on the
  declared support floor.

## Current state and next work

Product goal: `docs/PRODUCT.md`. Status: T001 through T007 and T010 are complete and committed. The runtime
runs natively on every desktop platform from one codebase (wry/tao on macOS and
Linux, Win32 + WebView2 on Windows), and the Mac-headless gates
are green. The Windows direct backend is cross-checked locally and exercised
on real Windows by CI. The unsigned G-3 release path is implemented in
`tools/packaging/package.sh` and `.github/workflows/unsigned-release.yml`:
native OS signing is deliberately out of scope, while the application-level
Ed25519 manifest signs the exact artifact bytes. The release workflow requires
the `KIRI_UPDATE_SIGNING_KEY_HEX` repository secret and rejects the known test
key for publication.
The queue lives in the corpus at `agent/task_queue.json` and current evidence
is summarized in `docs/STATUS.md`. Handoff notes are written to
`kiri-agent-execution-corpus/agent/HANDOFF.md` at session end.

Next work: Windows T009 numbers come from hosted `windows-latest` (no local
Windows box required); wry/tao is not yet a stable Windows comparison. T008
(WebView2 shared-buffer) is implemented and verified on real Windows. Local macOS
can ship an unsigned `.app` / `.dmg` via `tools/packaging/make-app.sh` and
`make-dmg.sh` (frontend is compile-time packed). `./tools/create-kiri-app.sh DIR`
copies the starter UI. An interactive demo ships at `examples/demo`.
Developer docs are in `docs/GETTING_STARTED.md` and `docs/API_REFERENCE.md`.
G-1 mobile and G-2 ecosystem breadth remain larger roadmap items. A fresh
update public key is now pinned in the runtime; its private half must be
supplied to GitHub as `KIRI_UPDATE_SIGNING_KEY_HEX` before the first public
release tag. The `correctness` workflow is the native all-OS correctness
path; it includes the Windows smoke/stress acceptance on `windows-latest`.

## Conventions

- Markers schema is frozen at `docs/research/markers-schema.md`
  (`schema_version: 1`). Changes require a schema bump and corpus doc
  update.
- Startup contract: exit 0 after `first_animation_frame`, exit 2 on
  watchdog. All three targets obey it.
- Commit messages: imperative, one-line summary plus body, reference task
  IDs (for example `T001: ...`).
