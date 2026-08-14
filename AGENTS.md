# AGENTS.md

Operating contract for agents working in this repository. Read this before
doing anything. The corpus files, where referenced, take precedence over
this summary.

## Project

Kiri is a cross-platform native desktop app runtime. A direct Win32 plus
WebView2 host on Windows and a wry/tao host on macOS and Linux are compared
against Tauri and Wry/Tao baselines on a shared startup-marker schema and a
control-plane protocol defined in the corpus
(`kiri-agent-execution-corpus/`, gitignored, see `docs/research/README.md`).
The host runs natively on every desktop platform; the direct backend is
Windows-only, the wry/tao backend covers macOS and Linux.

The direct platform ownership is a hypothesis to be tested, not a goal. If a
baseline matches or beats the direct host on the measured contract, record
that result and prefer the simpler path.

## Environment facts

- Host machine: macOS (aarch64), Rust 1.97 stable via `rust-toolchain.toml`.
- `x86_64-pc-windows-msvc` target is installed locally.
- Windows binaries cannot execute on this machine. The Windows runtime is
  validated by compile cross-checks locally and by the
  `windows-host-smoke` CI workflow on real Windows.
- The corpus is authoritative: specs, task queue, evidence policy. Do not
  edit corpus docs except task status and handoff notes.
- Nothing is committed until a coherent unit is done and every gate is green.

## Verification gates (run all before committing)

```sh
cargo test --workspace                                  # kiri-core: 45 tests
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
- Windows-only code lives in `crates/kiri-runtime-windows` and is gated with
  `cfg(target_os = "windows")` at the crate boundary.
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

Status: T001 through T004 complete and committed. The runtime runs
natively on every desktop platform from one codebase (direct Win32 + WebView2
on Windows, wry/tao on macOS and Linux), and all four tasks pass their gates
on the macOS development machine via the native wry/tao backend. The Windows
direct backend is cross-checked locally and exercised on real Windows by CI.
The queue lives in the corpus at `agent/task_queue.json` (T001 through T010)
and is mirrored in `README.md`. Handoff notes are written to
`kiri-agent-execution-corpus/agent/HANDOFF.md` at session end.

Next unblocked steps: T005 (command codegen + static routing), T006
(generational resource table), T007 (message bulk path benchmark) are
Mac-runnable. T008 (WebView2 shared-buffer) and T009/T010 (comparison,
diagnostics) depend on real Windows / self-hosted perf hardware and are
gated on CI. Verify the `windows-host-smoke` workflow on `windows-latest` to
close Q-001 and the direct backend's remaining acceptance.

## Conventions

- Markers schema is frozen at `docs/research/markers-schema.md`
  (`schema_version: 1`). Changes require a schema bump and corpus doc
  update.
- Startup contract: exit 0 after `first_animation_frame`, exit 2 on
  watchdog. All three targets obey it.
- Commit messages: imperative, one-line summary plus body, reference task
  IDs (for example `T001: ...`).
