# Cross-Platform Status (T011)

Authoritative per-OS record of what is verified, cross-checked, and blocked.
The repository is now PUBLIC, so GitHub-hosted runners are free/unlimited and
every OS runs the SAME gate on the SAME path (`.github/workflows/correctness.yml`).
Windows native execution is now VERIFIED on `windows-latest` (run #19 closed
Q-001). Linux native run stays a SOFT probe (no GPU on runners).

Do not fake completion: a lane is "verified" only when a gate actually ran
and passed on that platform's real runtime. Cross-checked (compile only) and
blocked are called out explicitly.

## macOS (wry/tao) — VERIFIED, this host

The day-to-day verification target. Real GPU, real WebView.

- `cargo test --workspace`: 186 kiri-core + 25 kiri-runtime unit tests pass.
- `cargo build -p kiri-runtime --bins`: builds.
- `kiri-host --smoke --frontend examples/blank`: all 9 startup markers,
  exit 0 (webview_ready, bridge_ready, dom_ready, first_animation_frame, ...).
- `kiri-host-stress --cycles 100`: 100 launch-close cycles, 0 failures.
- `cargo clippy -p kiri-runtime --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.

T011 added here: `kiri.open` (id 3) / `kiri.close` (id 4) control-plane
commands backed by a real `ResourceTable<()>`, so the diagnostics panel's
`open_resources` count is now honest and dynamic instead of a hardcoded 1.

## Windows (Win32 + WebView2) — VERIFIED on real Windows (CI)

`host_windows.rs` is `#[cfg(target_os = "windows")]` and cannot build or run
on this Mac. The only local evidence is compile-level.

- `cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets`: clean.
- `cargo clippy --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings`: clean.
- T011 change applied and compiles: router gains `with_resources`, caller is
  granted the `RESOURCES` capability, the static `open_resources=1` baseline
  was removed in favor of the same dynamic table as macOS.

VERIFIED on `windows-latest` (correctness run #19, after repo went public):
native smoke reached all required startup markers, 100-cycle stress ran 0
failures. Q-001 (WebView2 Evergreen runtime present on windows-latest) is
CLOSED - no install step was required. Cross-checks (`cargo check`/`clippy`
against `x86_64-pc-windows-msvc`) remain green on macOS/Linux runners.

NOTE (run #31866478695, commit 051696a): the native smoke had silently been
failing the marker assertion (missing webview_ready and dom_ready) while
the prior handoff claimed Q-001 was closed. Root cause: the injected bridge
script registered its DOMContentLoaded listener too late on WebView2 150/151
for a fast local page, so the dom ready message was lost. first_animation_frame
survived because it is driven by requestAnimationFrame. Fixed by guarding the
listener with a document.readyState check; the existing dom-message fallback
then also recovers webview_ready. Now genuinely green on real Windows.


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

- Windows + Linux *execution* gates: no local host for either OS on this Mac. On this
  PUBLIC repo, GitHub-hosted CI minutes are unlimited and free, so re-runs cost
  nothing -- the real constraint is hardware, not quota. CI already covers
  windows-latest (hard native gate) and ubuntu-latest (soft GPU probe) on every
  push/PR; re-run via `gh run rerun` or just push.
- `controlled-performance.yml` comparison (T009): needs a self-hosted labeled
  runner `[self-hosted, windows, x64, kiri-perf]`; cannot run on shared CI.
- T008 WebView2 shared-buffer: needs real Windows to implement + benchmark
  against the T007 baseline.
