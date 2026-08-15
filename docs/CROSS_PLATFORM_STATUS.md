# Cross-Platform Status (T011)

Authoritative per-OS record of what is verified, cross-checked, and blocked.
Written when CI was exhausted (100% of minutes used), so the only machine
that can still *run* gates is the macOS development host (aarch64, real GPU
via the wry/tao backend). Windows and Linux execution cannot happen locally;
those lanes are document + cross-check only.

Do not fake completion: a lane is "verified" only when a gate actually ran
and passed on that platform's real runtime. Cross-checked (compile only) and
blocked are called out explicitly.

## macOS (wry/tao) — VERIFIED, this host

The day-to-day verification target. Real GPU, real WebView.

- `cargo test --workspace`: 72 kiri-core + 6 kiri-runtime unit tests pass.
- `cargo build -p kiri-runtime --bins`: builds.
- `kiri-host --smoke --frontend examples/blank`: all 9 startup markers,
  exit 0 (webview_ready, bridge_ready, dom_ready, first_animation_frame, ...).
- `kiri-host-stress --cycles 100`: 100 launch-close cycles, 0 failures.
- `cargo clippy -p kiri-runtime --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.

T011 added here: `kiri.open` (id 3) / `kiri.close` (id 4) control-plane
commands backed by a real `ResourceTable<()>`, so the diagnostics panel's
`open_resources` count is now honest and dynamic instead of a hardcoded 1.

## Windows (Win32 + WebView2) — CROSS-CHECKED, blocked on execution

`host_windows.rs` is `#[cfg(target_os = "windows")]` and cannot build or run
on this Mac. The only local evidence is compile-level.

- `cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets`: clean.
- `cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings`: clean.
- T011 change applied and compiles: router gains `with_resources`, caller is
  granted the `RESOURCES` capability, the static `open_resources=1` baseline
  was removed in favor of the same dynamic table as macOS.

NOT VERIFIED (needs a Windows machine; previously validated on
windows-latest CI run #17 before minutes ran out): native smoke reached all 4
required startup markers, stress ran 100 cycles / 0 failures. Re-run there
once CI is available, or on a local Windows box.

## Linux (wry/tao) — CROSS-CHECKED, blocked on execution

Same backend as macOS (`host_cross.rs`), so it compiles and unit-tests with
the rest of the workspace. A real WebView render is impossible to verify here
(headless WebKit2GTK cannot finish compositor init without a GPU; proven on CI
run #15/#16 where the headless smoke always hit the watchdog).

- `cargo test -p kiri-runtime` and `cargo test -p kiri-core`: run as part of
  the workspace gate above (pure logic, no display needed).
- `cargo clippy` / `cargo fmt`: clean (same source as macOS).
- NOT VERIFIED: native WebView smoke/stress. On shared CI this is a known
  WebKit2GTK-no-GPU limitation, reported as a soft probe, never failing the
  job. On a Linux box with a real display (or a GPU-enabled runner) the macOS
  gate is the reference; the code path is identical.

## Capability / command catalog (T011)

`kiri.open` and `kiri.close` require the `RESOURCES` capability (bit 2),
enforced by the shared validate pipeline — JavaScript cannot self-grant.
Resource access is owner + generation validated by `ResourceTable`; a stale
or wrong-owner handle is rejected. `gen/commands.ts` regenerated to include
both (id 3 / id 4).

## What is blocked and why

- Windows + Linux *execution* gates: no local host for either OS; CI minutes
  exhausted. Re-run the existing workflows (`correctness.yml`,
  `windows-host-smoke.yml`) once minutes reset or on real hardware.
- `controlled-performance.yml` comparison (T009): needs a self-hosted labeled
  runner `[self-hosted, windows, x64, kiri-perf]`; cannot run on shared CI.
- T008 WebView2 shared-buffer: needs real Windows to implement + benchmark
  against the T007 baseline.
