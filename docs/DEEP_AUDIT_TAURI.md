# Deep Competitive Audit: Kiri vs Tauri

Scope: what Tauri does that Kiri does not, where Tauri is faster or better,
where Kiri already wins, and a ranked roadmap for Kiri to exceed Tauri on every
winnable dimension. Written for an overnight headless work loop; every claim is
tied to either A/B evidence (our code, Tauri source/docs) or explicitly marked
as inference (D).

Evidence levels (AGENTS.md): A = vendor docs/source/standard or measured local
result; B = maintained implementation source; D = inference.

---

## 1. What Tauri does that Kiri does NOT (capability gaps)

| # | Tauri capability | Status in Kiri | Evidence |
|---|------------------|----------------|----------|
| G-1 | Mobile targets (iOS, Android) via `tauri::mobile_entry_point` | Absent. Desktop only (Windows/macOS/Linux). | AGENTS.md env facts; `host_windows.rs`/`host_cross.rs` are the only backends. (A) |
| G-2 | Plugin ecosystem (50+ official plugins: http, fs, shell, dialog, process/sidecar, notification, updater, sqlite/sql, store, websocket, global-shortcut, clipboard, opener, os, path, window-state, nfc, barcode-scanner, biometrics, ble, ...) | Only 5 control-plane commands exist: `kiri.ping` (id 1), caller diag, `kiri.diag`, `kiri.open` (id 3), `kiri.close` (id 4). No plugin ABI implementation (only a `.h` stub). | `crates/kiri-core/src/dispatch.rs`; `kiri-agent-execution-corpus/examples/plugin_abi.h` is a header only, no host registration. (A/B) |
| G-3 | Official bundler + autoupdater (MSI/AppImage/dmg/nsis/deb; `tauri::updater`, Sparkle/WIx/NSIS, signed). | None. No packaging pipeline beyond `cargo build --bins`. | AGENTS.md "verification gates" list no bundling; `docs/14-packaging-linking.md` is a research doc, not an implementation. (A) |
| G-4 | `tauri://` asset protocol with range requests, content-type mapping, optimization, and `asset:customProtocol` allowlist. | `kiri://` now has content-type mapping + Range/206 (R-1) AND conditional caching (ETag + `If-None-Match` -> 304) + origin allowlist (G-4 parity), all headless-tested in `crates/kiri-runtime/src/assets.rs`. `serve_checked()` is wired into the macOS/Linux `host_cross.rs::serve_kiri`. Windows `host_windows.rs` uses WebView2 `SetVirtualHostNameToFolderMapping` (OS-handled content-types); custom mime/range/etag parity there is blocked pending real-Windows hardware (T008/T009 leg). | `crates/kiri-runtime/src/assets.rs`, `crates/kiri-runtime/src/host_cross.rs`. (A/B) |
| G-5 | `window.__TAURI__.os`, `path`, `app`, `event`, `cli`, `dialog`, `fs`, `globalShortcut`, `http`, `notification`, `process`, `shell`, `updater`, `window` JS APIs. | `kiri.window.*` (ids 14-22) is IMPLEMENTED and exceeds Tauri on the security axis: every op is authorized by the central capability authority (bit `WINDOW`) and routed through a host-owned `WindowController`, so JS can never reach the native handle. `kiri.platform.*`/`kiri.app.*`/`kiri.event.*` ship as the OS/app/event surface. Remaining Tauri JS surface still missing: `path`, `dialog`, `fs` (partial), `http`, `shell`, `notification`, `globalShortcut`, `updater` JS bindings. | `crates/kiri-core/src/window.rs`, `crates/kiri-runtime/src/window_ctl.rs`, `examples/blank/kiri.js`. (A) |
| G-6 | Sidecar binaries (ship + spawn companion executables), deep-link handling, tray icon, global shortcuts, window state persistence. | None of these. | AGENTS.md env facts. (A) |
| G-7 | Mature docs site, templates (`create-tauri-app`), active community, 80k+ GitHub stars, commercial backing (Tauri Studio / Committed Bits). | Early-stage, public but tiny. No templates, no community. | `docs/16-branding-legal.md` flags naming clearance risk. (A) |
| G-8 | Stable, semver'd plugin ABI and `tauri-build` codegen for commands/ACLs. | Plugin ABI is a header stub (G-2). Command codegen is numeric (`gen/commands.ts` referenced in COMPETITIVE_ANALYSIS, but `gen/` dir is absent in repo root — verify). | `ls gen/` returned empty. (B) |

**Honest read:** Tauri is a full application framework with an ecosystem; Kiri is a
runtime + control-plane hypothesis. G-1 through G-7 are the reasons Tauri has
customers today. None of these are "we are worse at the engine" — they are
product surface area.

---

## 2. Where Tauri is FASTER or BETTER (with our code evidence)

| # | Area | Why Tauri wins | Kiri evidence | Level |
|---|------|----------------|---------------|-------|
| F-1 | Asset/protocol loading | `tauri://` is a registered custom protocol with content-type + allowlist + in some configs optimized reads. | `kiri://localhost` does a synchronous `std::fs::read` per navigation with no mime/range/cache (`host_cross.rs:45`). For a multi-asset app this is strictly slower and less correct (wrong/omitted content-type). | B |
| F-2 | Cold-start is comparable, Tauri's mature builder avoids redundant windowing work | Our macOS `platform_initialized` ~129ms vs Tauri ~157ms (Kiri faster here, see COMPETITIVE_ANALYSIS T009 leg) — so on FIRST paint Kiri currently leads. But Tauri's startup is battle-tested across 1000s of apps; ours is measured on one host. | `artifacts/compare-macos.json` (gitignored). | A (macOS only) |
| F-3 | IPC for app logic | Tauri's command system (`#[tauri::command]`) is the de-facto ergonomic standard; huge body of examples. Kiri's numeric command routing is faster/auditable but has near-zero examples and no plugin surface. | `dispatch.rs` Router/StaticRouter. | B |

**Where we already measured a win:** on macOS startup phases (platform_initialized,
webview_ready) Kiri native is ~0.75-0.82x the wry/tao and Tauri baselines
(18-25% faster) — but ONLY on the engine-independent phases, and ONLY measured on
one macOS host. Do not generalize. (A, macOS-only.)

---

## 3. Where Kiri already WINS (protect these)

- Numeric, build-time command routing with a single validation pipeline and
  server-side capability bits (`dispatch.rs:165 register(id, required, handler)`).
- Generational resource handles (`resources.rs`, T006): stale/wrong-owner handles
  rejected, not reused. Tauri returns raw resource IDs with caller-managed lifetime.
- Privacy-scoped diagnostics (`diagnostics.rs`): never logs payload contents;
  emits caller id, result category, total ms, byte sizes.
- Frozen cross-platform startup marker schema (schema_version: 1) obeyed by all
  three targets with identical exit codes.
- Narrow default capability surface vs Tauri's ~80 core permissions by default.

These are real and verified. The competitive job is to make them matter to
customers, which requires closing G-1..G-8.

---

## 4. Ranked roadmap to EXCEED Tauri (winnable dimensions)

Priority is by (impact on "take their customers") x (feasibility from macOS now).

1. **R-1 (P0): Asset protocol parity/faster.** [DONE headless] Replace `std::fs::read` per request
   with a registered `kiri://` protocol handler that sets correct content-type,
   supports range requests, and serves a pre-read bundle. This closes F-1 and is
   Mac-runnable (headless testable via the protocol handler unit + a no-window
   fetch harness). Highest ROI: it is both a real perf gap AND a correctness bug.
2. **R-2 (P0): Plugin ABI implementation.** [DONE headless] `plugin_abi.h` exists but nothing
   registers plugins. Implement host-side `register_command` + a loader, then port
   `kiri.open`/`kiri.close`/`kiri.diag` as the first plugins. This is the on-ramp
   to an ecosystem (G-2) and is pure Rust, Mac-runnable.
3. **R-3 (P1): JS surface parity (os/path/app/event).** Expose a minimal,
   capability-gated `kiri.*` JS API mirroring Tauri's most-used modules. Mac-runnable.
4. **R-4 (P1): Bundling + autoupdate.** Even a thin `cargo-bundle`-style step +
   a signed-update check closes G-3. Requires release signing; Mac-runnable to
   build, but distribution signing needs certs.
5. **R-5 (P1): Scoped `kiri.fs` surface (DONE headless).** `kiri.fs.read|write|exists|remove`
   close the Tauri `fs` plugin parity gap (G-2) and exceed it on the security axis: central
   `FS` capability authority + host-owned `PathScope` allowlist + base64 payloads + bulk-object
   backpressure. `PathScope::allows` hardened for the macOS `/var` symlink and `..` escapes.
   Headless, Mac-runnable, 9 fs tests + 2 scope regression tests, all-OS CI-verifiable.
5. **R-5 (P2): Mobile (iOS/Android).** G-1. Largest Tauri-customer segment we
   lack. Requires a mobile backend (wry has mobile support in 0.40+). NOT
   Mac-headless-runnable for verification; parked until a device/CI is available.
6. **R-6 (P2): Ecosystem/docs/templates.** G-7. Marketing/community, not code.
7. **R-7 (P3): Sidecar / deep-link / tray / global-shortcut.** G-6. Nice-to-have
   differentiators; implement after R-1..R-4.

---

## 5. Immediate workstream (this loop, headless only)

- [x] Repo state verified: T001-T007, T010 done; T008 + T009-Windows blocked on
      Windows/perf HW; CI exhausted. `cargo test --workspace` green (98 tests).
- [x] This audit written from code evidence + Tauri public docs.
- [x] R-1 spike DONE: kiri:// handler in crates/kiri-runtime/src/assets.rs (commit bdb75ef). content-type + Range to 206, 16 unit tests. Headless.
- [x] R-2 spike DONE: host-side plugin registration in crates/kiri-runtime/src/plugins.rs. PING_PLUGIN ported via KiriPluginV1/KiriHostV1 mirror of plugin_abi.h; build_router_with_plugins() replaces inline Router::new(). Headless.
- [x] R-3 spike DONE: capability-gated kiri.* JS surface (kiri.platform.os/arch, kiri.app.version, kiri.event.emit/listen) in crates/kiri-core/src/platform.rs + Router::with_platform(). Shipped frontend API in examples/blank/kiri.js served via kiri://. Headless unit tests enforce capabilities + verify payloads.
- [x] R-3b DONE: G-4 asset-protocol parity on the cross backend. `serve_checked()` adds ETag + `If-None-Match` 304 + origin allowlist to `kiri://`; wired into `host_cross.rs::serve_kiri` (lines ~47-90) and exercised by 3 new headless asset tests. Windows folder-map leg stays blocked (T008/T009 hardware).
- [x] R-2 COMPLETED: plugin ABI is now real, not a scaffold. `host_register_command` carries each command's actual `Handler` (previously a hardcoded echo), and four built-ins are ported as genuine plugins: `kiri.ping`, `kiri.diag`, `kiri.open`, `kiri.close`. Stateful plugins (`diag`, `open`, `close`) bind the runtime's shared `Diagnostics`/`ResourceTable`/`CallerId` via a new `KiriHostContextV1` pointer in `plugin_abi.h`, matching how an external plugin reaches host services. `build_router_with_plugins(diagnostics, resource_table, caller)` loads all three via the plugin path; capability bits are derived from the command id (`capability_bit::for_command`), so authority stays identical to the old inline `Router::with_*`. 5 headless plugin tests cover registration + dispatch + stateful open/close. This closes the G-2 on-ramp: the registration mechanism is proven end to end for every built-in command. Windows/macOS/Linux call sites updated to share one real `Arc<Mutex<ResourceTable>>` with the plugin (no double registration).
- [x] R-4 (P1) signed-updater COMPLETE: the `UpdateManifestBuilder` producer signs each platform's installer bytes with the release Ed25519 key and emits the `RELEASES.json` the runtime pins (kiri-core::update::update::verify_asset_for verifies every OS asset on any host). The verifier rejects tampered/wrong-key/missing-signature/downgrade. Headless, no certs, proven on all three OSes via the `updater` CI job (cargo test -p kiri-core update::); that job previously caught a macOS-biased fixture (signature only on the darwin asset) that failed on Linux/Windows and was fixed. Signed distribution still needs Apple Developer + Windows code-sign certs, but the build+verify loop is closed and CI-verified.
- NOTE: no step in this loop launches `kiri-host` or a baseline binary, so the
  screen never flashes. All verification is `cargo test`/`clippy`/`fmt`/`bulk_bench`.

- [x] R-5 DONE: scoped `kiri.fs` surface (read/write/exists/remove) in kiri-core::fs with
  host-owned `PathScope` + `FS` capability bit + base64 payloads + bulk-object backpressure.
  `PathScope::allows` hardened for `/var` <-> `/private/var` normalization and `..` escape
  rejection (2 regression tests). Wired into both hosts and the frontend (examples/blank/kiri.js).
  9 fs unit tests. All-OS CI-verifiable. This exceeds Tauri's `fs` plugin on the security axis.

---

## 6. Threats to the "exceed Tauri" claim (call them out)

- Tauri v2 is stable, funded, and shipping mobile. Kiri cannot "win" on ecosystem
  overnight; it wins on control-plane discipline + (measured, narrow) startup edge.
- The startup edge is macOS-only and one-host. Must be reproduced on Windows
  (T009 Windows leg, blocked) before any external claim.
- Branding: `KIRI` is not unique (crates.io package, KIRI Engine). Legal clearance
  needed before any "take their customers" campaign (`docs/16-branding-legal.md`).
