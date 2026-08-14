# AGENTS.md

Operating contract for agents working in this repository. Read this before
doing anything. The corpus files, where referenced, take precedence over
this summary.

## Project

Kiri is a Windows-first native desktop app runtime. A direct Win32 plus
WebView2 host is compared against Tauri and Wry/Tao baselines on a shared
startup-marker schema and a control-plane protocol defined in the corpus
(`kiri-agent-execution-corpus/`, gitignored, see `docs/research/README.md`).

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
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime-windows --all-targets
cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime-windows --all-targets -- -D warnings
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

Do not run `cargo clippy --workspace --all-targets` on macOS: the Windows
runtime binaries have no `main` off Windows, so it fails with E0601 by
design. CI handles this with per-OS steps.

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

Status: T001 in progress. All local gates green; the host has never executed
on Windows. The queue lives in the corpus at `agent/task_queue.json` (T001
through T010) and is mirrored in `README.md`. Handoff notes are written to
`kiri-agent-execution-corpus/agent/HANDOFF.md` at session end.

Next unblocked step after this repo is pushed: verify the
`windows-host-smoke` workflow on `windows-latest`, confirm WebView2 runtime
availability (Q-001 in `docs/OPEN_QUESTIONS.md`), then close T001 and T002
acceptance from the resulting logs. If the runner lacks WebView2, add an
install step.

## Conventions

- Markers schema is frozen at `docs/research/markers-schema.md`
  (`schema_version: 1`). Changes require a schema bump and corpus doc
  update.
- Startup contract: exit 0 after `first_animation_frame`, exit 2 on
  watchdog. All three targets obey it.
- Commit messages: imperative, one-line summary plus body, reference task
  IDs (for example `T001: ...`).
