# Production Goal — Long-running

**One sentence:** Kiri is production-ready when a person who has never cloned the repo can `create-kiri-app`, build with `KIRI_EMBED_FRONTEND`, and publish a signed `RELEASES.json` that any host verifies with the pinned Ed25519 key, while `docs/COMPETITIVE_ANALYSIS.md` shows a single honest hosted scoreboard and `muda` menus are visible on all three OSes.

**Exit gates (all must be green on `main`):**
1. `cargo test --workspace` 279 + `cargo fmt --check` + `cargo clippy -p kiri-runtime --all-targets -- -D warnings` + `cargo check --target x86_64-pc-windows-msvc` + `cargo check --manifest-path baselines/{wry-tao,tauri}/Cargo.toml` on `ubuntu/macos/windows-latest` (`correctness.yml`).
2. `controlled-performance` 20 runs / 3 warmups / 45s timeout `status: complete` for Kiri + Tauri on `macos-latest` + `windows-latest`; Wry/Tao `status: complete` or explicit `not comparable` doc; artifacts `startup-kiri.json`, `ipc-kiri.json`, `binary-sizes.json` uploaded (`if-no-files-found: error` for Kiri/Tauri).
3. `Native menu smoke` hard gate on `macos-latest` + `windows-latest` (`kiri.menu.set` 72 + `invoke` 73 via `window.kiri.send` through `examples/menu-smoke`, `menu_smoke` ok, `muda::MenuEvent` → `window.kiri.onMenuAction`).
4. `tools/create-kiri-app.sh` (`--template starter|starter-vite|blank`) + `.ps1` (`ValidateSet`, `ARM64`, local fallback) both verify `sha256` lower-case and emit `run.sh`/`run.cmd`; `tools/packaging/package.sh` + `lib-icon.sh` emit `kiri-<version>-<os>-<arch>.*` + `RELEASES.json` verified vs pinned `333d58ae...` (rehearsal `0707...` only with `KIRI_ALLOW_TEST_UPDATE_KEY=1`).
5. `docs/COMPETITIVE_ANALYSIS.md` single current hosted table (run `32730288110`), historical collapsed in `<details>` + `docs/archive/COMPETITIVE_HISTORY.md`; no `252 pass` or `Windows-first` outside `docs/archive` (`correctness.yml:80` lint).

**Subagents (parallel, long-running):**
- **A — Packaging & Signing:** `tools/packaging/*` + `unsigned-release.yml` + `emit_release_manifest.rs` + `RELEASE_CHECKLIST.md` — DONE `4ed83e7` (stub `APPLE_CERT`/`WINDOWS_CERT`, `KIRI_ALLOW_UNSIGNED`, rehearsal).
- **B — Native Menu:** `menu_dispatch.rs` 32/2s + `native_menu.rs:65` `native_menu_windows.rs:39` hardened `replace` + `correctness.yml:130` smoke + `ARCHITECTURE_MENU.md:82` — DONE `4ed83e7` (manual keyboard/screen-reader remains human).
- **C — Evidence Single Page:** `COMPETITIVE_ANALYSIS.md:3` header + `CROSS_PLATFORM_STATUS.md:96` + `STATUS.md:31` + `FINISH_PLAN.md:49` — DONE `4ed83e7`.
- **D — Scaffolder Parity:** `create-kiri-app.sh:10` `--template` + `create-kiri-app.ps1:13` arch/local + `starter-vite` + `TEMPLATE_MIGRATION_TAURI.md` — DONE `1abad96`.
- **E — Hygiene:** `docs/archive/*` + `FINISH_PLAN.md` phases 0-6 + `RELEASE_CHECKLIST.md` — DONE `731d806..99dbddb`.

**Remaining human / hardware-blocked (not code):**
- Apple Developer `APPLE_CERT` + Windows Authenticode `WINDOWS_CERT` provisioning (certs are the blocker per `GAP_MATRIX.md:25`); pipeline is ready to wire `codesign`/`signtool` where `package.sh:86` logs stub.
- Physical eye-test: visible native menu + VoiceOver/NVDA/Orca (cannot automate).
- Next hosted `controlled-performance` run to flip Wry/Tao `incomplete` → `complete` (or document `not comparable`).

**Non-goals:** `G-1` mobile, `G-2` 50+ plugin breadth, render speed — per `PRODUCT.md:32`.

**How to verify `production ready`:**
```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
./target/debug/kiri-host --smoke --frontend examples/blank --markers-out /tmp/kiri-startup.json
./target/debug/kiri-host --smoke --frontend examples/menu-smoke --markers-out /tmp/kiri-menu.json
./target/debug/kiri-host-stress --frontend examples/blank --cycles 3
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
gh run watch --workflow correctness --workflow controlled-performance --workflow unsigned-release
```
