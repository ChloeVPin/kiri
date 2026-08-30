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

- `cargo test --workspace`: 220 kiri-core + 57 kiri-runtime unit tests pass (plus 2 integration tests).
- `cargo build -p kiri-runtime --bins`: builds.
- `kiri-host --smoke --frontend examples/blank`: all 9 startup markers,
  exit 0 (webview_ready, bridge_ready, dom_ready, first_animation_frame, ...).
- `kiri-host-stress --cycles 100`: 100 launch-close cycles, 0 failures.
- `cargo clippy -p kiri-runtime --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- The cross-backend `kiri://` handler uses Wry's asynchronous custom-protocol
  path, so frontend file reads do not block the WebView event thread. The
  change is verified by the native smoke/stress gates below and by a local
  release benchmark; hosted performance must be rerun after publication.

The smoke/stress bullets above refer to the last successful native gate. A
fresh local macOS retry on 2026-08-24 hit the startup watchdog before
`webview_ready`; this is retained as incomplete local evidence because the
same host's WebView environment has previously failed to initialize. It does
not replace the successful hosted correctness evidence.

T011 added here: `kiri.open` (id 3) / `kiri.close` (id 4) control-plane
commands backed by a real `ResourceTable<()>`, so the diagnostics panel's
`open_resources` count is now honest and dynamic instead of a hardcoded 1.

G-12 menu: `kiri.menu.set` (72) / `kiri.menu.invoke` (73) is wired via
`MenuDispatcher` (queue 32, 2 s timeout in `menu_dispatch.rs:11`) + `NativeMenu`
(muda 0.19.3, `native_menu.rs:65`) on the event-loop thread
(`host_cross.rs:428` / `host_cross.rs:648` drain + `host_cross.rs:669` `muda::MenuEvent`
→ `window.kiri.onMenuAction`); through-webview smoke is `examples/menu-smoke`
(`menu_smoke` ok flag gates `host_cross.rs:541` exit 1 on failure) and is a hard
gate on `macos-latest` in `correctness.yml:134`. Manual keyboard/screen-reader
check is the remaining human eye-test.

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
against `x86_64-pc-windows-msvc`) remain green on Linux/macOS runners.

G-12 menu (Windows): same `MenuDispatcher` + `NativeMenuWindows`
(`native_menu_windows.rs:39`, `host_windows.rs:358` wnd_proc drain +
`host_windows.rs:360` `muda::MenuEvent` → `window.kiri.onMenuAction`) with
through-webview smoke as hard gate on `windows-latest` in
`correctness.yml:161` (`menu_smoke` ok flag gates `host_windows.rs:955` exit 1
on failure). Manual keyboard/screen-reader check is the remaining human eye-test.

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
- `controlled-performance.yml` now runs on public hosted macOS and Windows
  runners with **3 warmups / 45 s** timeout for Wry/Tao (symmetric, `continue-on-error: true` soft) and hard gates for Kiri/Tauri (`controlled-performance.yml:38,114-122`, `on: pull_request`). Actions runs `32696370579` and `32730288110` (commit `0d7e9a6`, 20 runs) completed both hosted jobs and produced startup, IPC, and binary-size artifacts. The Windows Wry/Tao startup baseline is explicitly **incomplete after its 45 s warmup timeout** (soft, `continue-on-error: true`), so it is not treated as a stable three-way Windows comparison; Kiri/Tauri and IPC results are usable. The current authoritative scoreboard is the single table in [`COMPETITIVE_ANALYSIS.md`](COMPETITIVE_ANALYSIS.md) (run `32730288110`); historical tables are archived at `docs/archive/COMPETITIVE_HISTORY.md`.

The follow-up correctness run `32730288096` and performance run
`32730288110` both completed successfully, but **T009 remains open and awaits the next hosted `controlled-performance` run** for a stable Kiri vs Wry/Tao vs Tauri three-way on Windows with the new 45 s symmetric warmup (see `controlled-performance.yml:117,122`). Hosted startup medians vary materially between runs (e.g., macOS Kiri 1,819 ms vs 851 ms), so no universal startup win is claimed.
- T008 WebView2 shared-buffer is verified on real Windows; see
  [`SHARED_BUFFER_REPORT.md`](SHARED_BUFFER_REPORT.md). T009 remains open only
  for a stable hosted three-way.
