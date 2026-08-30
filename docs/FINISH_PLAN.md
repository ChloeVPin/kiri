# Kiri Finish-to-Ship Plan

> Goal per `docs/PRODUCT.md:14`: four acceptance criteria = download/scaffold without cloning, open window talking to host, ship own UI to three OSes from CI, one page of honest Kiri-vs-Tauri numbers. Until then, working runtime with demo — not finished product.

## 0. About / positioning — DONE this session

**Problem:** `crates/kiri-runtime/Cargo.toml:6` read `direct Win32 + WebView2 on Windows, wry/tao on macOS/Linux` (Windows-first). Two docs used Windows-first slash ordering.

**Fix applied:**
- `crates/kiri-runtime/Cargo.toml:6` → `Native host for Kiri for Linux, macOS, and Windows (wry/tao on Linux and macOS, Win32 + WebView2 on Windows)`
- `crates/kiri-core/src/lib.rs:7` → `Linux, macOS, and Windows backends`
- `docs/DEEP_AUDIT_TAURI.md:18` → `Linux, macOS, and Windows`
- `docs/DEEP_AUDIT_TAURI.md:154` → `Linux, macOS, and Windows call sites`
- `docs/COMPETITIVE_ANALYSIS.md:59` → `Linux, macOS, and Windows`

Canonical order is now `Linux, macOS, and Windows` everywhere, matching `README.md:6`, `AGENTS.md:10`, `STATUS.md:9`. No GitHub repo file stores the About blurb separately; if the GitHub UI still shows a Windows-first tagline, update Settings → About → Description to: `Native desktop app runtime for Linux, macOS, and Windows — a granted capability must not be enough` and topics `rust, tauri-alternative, webview, desktop`.

---

## Current state (evidence level A unless noted)

- `v0.1.6` published, 279 tests (220 core + 57 runtime + 2 integration), `cargo fmt/clippy` green, native smoke+stress green on macOS and on `windows-latest` (correctness run #19, 100 cycles `CROSS_PLATFORM_STATUS.md:50`), T008 shared-buffer verified (`SHARED_BUFFER_REPORT.md:5` run 31988662774, 20/20 replies at 256 KiB + ~1 MiB).
- Control plane: 74 ids (1–74) double-gated, host allowlists wired in both backends (`host_cross.rs`/`host_windows.rs`).
- Packaging: unsigned `.app` zip, `.exe` zip, `.tar.gz` + Ed25519 `RELEASES.json` via `tools/packaging/package.sh` + `.github/workflows/unsigned-release.yml`. POSIX + PowerShell scaffolders exist.

## What "finished" means — exit gates

1. `cargo test --workspace` + `cargo fmt --check` + native `cargo clippy -D warnings` + `cargo check/clippy --target x86_64-pc-windows-msvc` + baselines `cargo check` on all three OSes (already in `correctness.yml`).
2. Hosted `controlled-performance` produces a stable three-way set (Kiri vs wry/tao vs Tauri) for startup + IPC on macOS **and** Windows — no `incomplete` Wry/Tao warmup.
3. Native menu visible + activatable on Linux, macOS, and Windows with allowlist enforcement and hosted correctness coverage (`ARCHITECTURE_MENU.md:68`).
4. `tools/create-kiri-app.sh` and `.ps1` produce bit-identical verifier behavior (SHA check) and documented parity/FORCE fallback.
5. One page of published numbers (`COMPETITIVE_ANALYSIS.md` current table) claims only what hosted artifacts show, with runner + distribution metadata.

## Mess inventory (why it feels unfinished)

1. **Doc sprawl / contradictions:** 16 files under `docs/*.md` (not 13), five roadmaps (`PRODUCT.md`, `ROADMAP.md`, `GAP_MATRIX.md`, `EXCEED_TAURI_PLAN.md`, `DEEP_AUDIT_TAURI.md:103`), test-total `279` vs `252` (`DEEP_AUDIT_TAURI.md:172`), footprint "not measured" vs measured (`COMPETITIVE_ANALYSIS.md:43` vs `:194`), `STATUS.md:33` green vs `CROSS_PLATFORM_STATUS.md:29` incomplete smoke. `OPEN_QUESTIONS.md:Q-002/004/005` still open while `CROSS_PLATFORM_STATUS.md:50` claims VERIFIED.
2. **CI masking:** `correctness.yml:141` Linux soft probe always passes, `controlled-performance.yml:67` six× `continue-on-error: true` + `if-no-files-found: warn` hides hung WebView as `incomplete` green.
3. **Menu shipped as code but not as product:** 5 commits `9fcc472..322c989` landed `muda 0.19.3` adapter + dispatcher + wiring, but `ARCHITECTURE_MENU.md:80` still `service_unavailable` and no hosted eye-test.
4. **Windows Wry/Tao flake:** every hosted run reports `warmup timeout at 20 s` (`STATUS.md:76`), asymmetry `controlled-performance.yml:121` vs `:128` timeouts.
5. **Scaffolder drift:** `sh` auto-detects `aarch64/x86_64`, local fallback, `ditto`; `ps1` hardcodes `windows-x86_64`, always downloads starter, different SHA method.

## Phased plan

### Phase 0 — Hygiene baseline (1–2 days, no behavior change) — ✅ DONE `731d806`

| Task | Files | Verify |
|---|---|---|
| Freeze single source of truth: keep `PRODUCT.md`, `ROADMAP.md`, `STATUS.md`, `CROSS_PLATFORM_STATUS.md`, `COMPETITIVE_ANALYSIS.md`, `ARCHITECTURE_MENU.md`; mark `DEEP_AUDIT_TAURI.md` + `EXCEED_TAURI_PLAN.md` as `docs/archive/*` with header `superseded by GAP_MATRIX.md + ROADMAP.md` | `docs/*.md`, `AGENTS.md:82` T-queue line | `cargo fmt --check` still passes, `docs/ROADMAP.md:34` tracking list updated |
| Reconcile numbers: set one canonical `279` total, fix `DEEP_AUDIT_TAURI.md:172` `252`, reconcile `STATUS.md:33` vs `CROSS_PLATFORM_STATUS.md:29` with dated note `local 2026-08-24 incomplete vs hosted green` | `docs/STATUS.md`, `docs/CROSS_PLATFORM_STATUS.md`, `docs/COMPETITIVE_ANALYSIS.md:43` | `grep -rn "279"` clean (no stale 252 remaining) |
| Normalize platform order to `Linux, macOS, and Windows` lint: add `grep` check to CI | `.github/workflows/correctness.yml` | CI fails if Windows-first slash order reappears |

**Exit:** `docs/*.md` ≤ 10 active files, no contradictory median/weight claims, `cargo test --workspace` count matches docs.

### Phase 1 — Native app menu to real 증거 (3–5 days) — ✅ DONE `868a09b` + `b4e26a4`

Current: `kiri.menu.set/invoke` ids 72–73 capability+allowlist done (`crates/kiri-core/src/app_menu.rs`), `menu_dispatch.rs:59` + `native_menu.rs:32` + `native_menu_windows.rs:51` wired, but `ARCHITECTURE_MENU.md:80` returns `service_unavailable`.

Steps in `crates/kiri-runtime/src/{host_cross.rs:401, host_windows.rs:1160, menu_dispatch.rs:138}`:
1. Replace stub runner with `MudaMenuRunner` on main thread (already thread-affine, bounded channel, timeout, duplicate-ID guard).
2. Add hosted eye-test: `correctness.yml` macOS step + Windows step that calls `kiri.menu.set(["quit","about"])` via `window.kiri.send` and asserts `getMenu().length==2` + `invoke` reaches host without deadlock; Linux soft-probe documents unsupported. — DONE: `examples/menu-smoke` + `correctness.yml:130` smoke, host enforces `menu_smoke` failure (`host_cross.rs:538`, `host_windows.rs:953`).
3. Update `ARCHITECTURE_MENU.md:68` checklist to green and flip `service_unavailable` gate. — DONE: `docs/ARCHITECTURE_MENU.md:60` documents dispatcher handle as production runner.

**Exit:** hosted correctness on macOS+Windows shows native menu installed, `invoke` routes to owning window, unknown ID rejected, concurrent `set` no partial state.

### Phase 2 — Stabilize T009 three-way (2–3 days) — ✅ DONE `e9dc2c2`

Problem: `baselines/wry-tao` `warmup timeout at 20 s` on `windows-latest` (`controlled-performance.yml:128`).

Steps:
1. Reproduce locally on Windows: add `--verbose` warmup logging in `benchmark/harness.py:40` + dump `tao` init error to artifact.
2. Fix options (in order): raise Windows Wry warmup to `45 s` symmetric with macOS, or pin `tao 0.36` `EventLoop` to `RunReturn` warmup path; if still flake, document Wry/Tao as `not comparable on Windows hosted` and change table semantics to `Kiri vs Tauri` only (explicit, not silent `incomplete`). — DONE: symmetric `3/45s` both runners, `Tauri 25s` symmetric.
3. Remove asymmetric caching: either cache in both workflows or neither (`correctness.yml:44` vs `controlled-performance.yml:44`).
4. Make `controlled-performance.yml` trigger on `pull_request` as well (not only `main`). — DONE: `on: pull_request` added, Kiri/Tauri hard gates, Wry soft.

**Exit:** one hosted run `controlled-performance` artifact contains `startup-kiri / startup-tauri / startup-wrytao` all `status: complete` on both runners, or explicit doc that Wry/Tao Windows is not a hosted comparison.

### Phase 3 — Scaffolder + packaging parity (2 days) — ✅ DONE `1abad96`

- Unify SHA verification to use `sha256sum`/`Get-FileHash` both logging hex lower-case, add `.ps1` local `examples/starter` fallback when `KIRI_REPO` checkout present (mirror `create-kiri-app.sh:28`). — DONE: `create-kiri-app.ps1:13` arch, `48` local fallback, SHA log.
- Reconcile `make-app.sh` vs `package.sh:112` icon path (single `assets/kiri.png → sips/iconutil` helper). — DONE: `tools/packaging/lib-icon.sh` shared.
- Add second frontend template (plain HTML + Vite) under `examples/starter-vite` and scaffold flag `--template {starter,blank}`; add `docs/TEMPLATE_MIGRATION_TAURI.md` minimal mapping. — DONE: `examples/starter-vite/` + `docs/TEMPLATE_MIGRATION_TAURI.md`.

**Verify:** `correctness.yml:60` scaffold smoke asserts `bin/kiri-host` or `.exe` + `frontend/index.html` + `run.*` on all three OSes; PowerShell `ParseFile` still clean.

### Phase 4 — Updater UX wiring (2 days, behind feature flag) — ✅ DONE `8e07e12`

`crates/kiri-core/src/update.rs` + `updater_surface.rs:61` + `update_feed.rs` already verify Ed25519 with pinned key. — DONE: `examples/demo/index.html:410` auto-check (1.2 s) + banner, `host_cross.rs:128` + `host_windows.rs:666` `onUpdateAvailable` bridge hook. Host polling (`KIRI_UPDATE_CHECK=0` disable) remains next iteration but demo already exercises pinned-key flow without terminal.

Wire: `host_cross.rs`/`host_windows.rs` poll `RELEASES.json` on timer (host policy allowlist, signed URL), emit `kiri:update-available` event to frontend, frontend `examples/demo` shows banner "Update 0.1.7 available — restart". Add `KIRI_UPDATE_CHECK=0` to disable.

**Exit:** headless test `cargo test -p kiri-core update::` still passes, hosted smoke not affected, demo shows banner with mock feed.

### Phase 5 — Evidence hygiene + release rehearsal (1 day) — ✅ DONE `9dc93cc` (partial) + `e9dc2c2`

- Replace narrative scattered medians in `COMPETITIVE_ANALYSIS.md:139-326` with single table `current hosted (run ID, commit, runner, p50/p95)` + `historical` collapsed. — DONE minimal: header `Current hosted scoreboard: run 32730288110` added; full collapse deferred to keep history auditable.
- Change `controlled-performance.yml` `continue-on-error: true` to `false` for Kiri/Tauri, keep `warn` only for Wry/Tao if documented not comparable; require `startup-kiri.json` artifact exists or job fails (`if-no-files-found: error`). — DONE: `e9dc2c2` hard gates.
- Run local `tools/packaging/package.sh --out /tmp/kiri-out` + `KIRI_UPDATE_SIGNING_KEY_HEX` redacted rehearsal, verify `RELEASES.json` Ed25519 against pinned public key (`cargo test -p kiri-core update`). — Deferred to CI `unsigned-release.yml` (requires secret).

**Exit:** `STATUS.md:50` public checklist adds `T009 stable three-way OR documented not comparable` line, `PRODUCT.md:52` scoreboard points to single hosted run artifact.

### Phase 6 — Pre-1.0 cleanup (1 day) — ✅ DONE `9dc93cc`

- `cargo clippy --workspace --all-targets -- -D warnings` but keep existing `host_windows.rs` `cfg(target_os="windows")` exclusion via per-target invocation (`AGENTS.md:18` gate).
- Remove dead `DisabledWs`/`DisabledMenu` no-ops once native paths green or keep as `service_unavailable` fallback with explicit error code. — Kept as explicit fallback (honest boundary).
- `rust-toolchain.toml:2` pin `channel = "1.97"` to match `AGENTS.md:9` claim or update AGENTS to `stable`. — DONE: `rust-toolchain.toml:2` `1.97`.

## Non-goals (stay honest, per `PRODUCT.md:32`)

- No OS store signing (Apple notarization / Authenticode) — stays out of scope, noted `GAP_MATRIX.md:25` blocked on certs.
- No mobile (`G-1`) — desktop first.
- No plugin-count race (`G-2` breadth) — keep security-axis win, not catalog size.
- No render-performance claim — same WebView engine (`COMPETITIVE_ANALYSIS.md:9`).

## Execution order & owner

1. Phase 0 → Phase 1 + Phase 2 in parallel → Phase 3 → Phase 4 → Phase 5 → Phase 6.
2. Each phase lands as one commit with green `correctness` on `ubuntu-latest + macos-latest + windows-latest` before merging.
3. Track queue in `kiri-agent-execution-corpus/agent/task_queue.json` (task IDs T012 menu, T009 stable, T_scaffold parity).

## How to verify "finished"

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
gh run watch --workflow correctness --workflow controlled-performance
```

plus hosted artifacts contain `status: complete` for Kiri, Tauri, (and Wry/Tao or documented exclusion), menu, and scaffold.

