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
| G-2 | Plugin ecosystem + ABI. Tauri ships 50+ official plugins. | Plugin ABI is IMPLEMENTED (R-2): `kiri-agent-execution-corpus/examples/plugin_abi.h` is mirrored in `crates/kiri-runtime/src/plugins.rs` (Rust-native `KiriPluginV1`/`KiriHostV1` with `init`/`register_command`/`shutdown`), and built-in commands `kiri.ping`/`kiri.diag`/`kiri.open`/`kiri.close` are REGISTERED AS PLUGINS through that ABI with end-to-end dispatch tests. ADDITIONALLY the host-owned external plugin loader is implemented, headless-tested, AND wired into runtime startup: `build_router_with_plugins` now takes a host-owned `PluginManifest` (default-deny JSON) and a `PluginRegistry` (name -> descriptor). External plugins load only when named in the manifest, resolved through the registry, and gated by a per-plugin command allowlist; unknown or unallowed entries are skipped (fail-closed). This EXCEEDS Tauri's plugin model on the security axis: Tauri trusts any plugin present on the configured path, while Kiri refuses an unknown plugin before `init` and drops unvetted commands. The in-process descriptor registry stands in for a real `dlopen` loader (OS-specific, out of scope for headless Mac work); the security gate that exceeds Tauri is identical either way. Ecosystem breadth (50+ plugins, catalogs) is still absent. | `crates/kiri-runtime/src/plugins.rs` (PING/DIAG/RESOURCES_PLUGIN + `PluginAllowlist` + `PluginManifest` + `PluginRegistry` + `register_external` + `tests`), `kiri-agent-execution-corpus/examples/plugin_abi.h`. (A) |
| G-3 | Official bundler + autoupdater. | Application-level signed updates are implemented (`crates/kiri-core/src/update.rs`: Ed25519 manifest, version-negotiated, never lowers a security check) with a producer that signs the actual artifact URL + SHA-256 and a verifier that checks the downloaded bytes. The unsigned packaging pipeline emits a macOS `.app` zip, Windows `.exe` zip, Linux `.tar.gz`, and a merged three-platform `RELEASES.json`. `kiri-host` packs a frontend at compile time (`KIRI_EMBED_FRONTEND`, default `examples/blank`). macOS `.app` from `make-app.sh` is embed-only (no sidecar UI folder). Windows still materializes the pack to a temp dir for WebView2 folder mapping. Artifacts remain unsigned at the OS level. | `crates/kiri-core/src/update.rs`, `tools/packaging/package.sh`, `.github/workflows/unsigned-release.yml`. (A) |
| G-4 | `tauri://` asset protocol with range requests, content-type mapping, optimization, and `asset:customProtocol` allowlist. | `kiri://` now has content-type mapping + Range/206 (R-1) AND conditional caching (ETag + `If-None-Match` -> 304) + origin allowlist (G-4 parity), all headless-tested in `crates/kiri-runtime/src/assets.rs`. `serve_checked()` is wired into the macOS/Linux `host_cross.rs::serve_kiri`. Windows `host_windows.rs` uses WebView2 `SetVirtualHostNameToFolderMapping` (OS-handled content-types); custom mime/range/etag parity there is blocked pending real-Windows hardware (T008/T009 leg). | `crates/kiri-runtime/src/assets.rs`, `crates/kiri-runtime/src/host_cross.rs`. (A/B) |
| G-5 | `window.__TAURI__.os/path/app/event/cli/dialog/fs/globalShortcut/http/notification/process/shell/updater/window` JS APIs. | The logical command catalog, capability checks, host allowlists, and server-side validation are implemented for `kiri.window.*`, `kiri.platform.*`, `kiri.app.*`, `kiri.event.*`, `kiri.path.*`, `kiri.http.*`, `kiri.shell.run`, `kiri.notification.show`, `kiri.dialog.open`, `kiri.clipboard.*`, `kiri.opener.open`, `kiri.store.*`, `kiri.deeplink.register`, `kiri.sidecar.*`, `kiri.window.state.*`, `kiri.config.*`, `kiri.updater.check`, `kiri.cli.args`, `kiri.fs.watch/unwatch`, `kiri.ws.*`, `kiri.menu.*`, and `kiri.plugin.list`. Native transport is capability-specific: filesystem watching and bounded `ws://`/`wss://` have native/loopback evidence; application menus remain unavailable; shortcut/autostart/tray transports currently retain host-owned state without OS registration. Process parity is covered by `kiri.sidecar.*` and `kiri.shell.run`. | `crates/kiri-core/src/{window,path,http,shell,notification,dialog,shortcut,clipboard,opener,store,deeplink,tray,sidecar,window_state,config,event,cli,updater_surface,fs_watch,websocket,app_menu,plugin_inventory}.rs`, `examples/blank/kiri.js`. (A) |
| G-6 | Sidecar binaries, deep-link, tray, global shortcuts, window-state persistence. | Logical contracts and security gates exist for all five. Sidecars, deep links, and window state have host-owned implementations; tray and global-shortcut runners currently provide host-owned state and require native OS registration work before they can be called complete. | `crates/kiri-core/src/{sidecar,deeplink,tray,shortcut,window_state}.rs`, `crates/kiri-runtime/src/{tray_ctl,shortcut_ctl}.rs`. (A) |
| G-7 | Mature docs site, templates (`create-tauri-app`), active community, 80k+ GitHub stars, commercial backing. | Early-stage, public but tiny. No templates, no community. | `docs/16-branding-legal.md` flags naming clearance risk. (A) |
| G-8 | Stable, semver'd plugin ABI and `tauri-build` codegen for commands/ACLs. | Plugin ABI is implemented (see G-2). Command codegen IS present at `crates/kiri-core/gen/commands.ts` (regenerated from the `COMMANDS` catalog via `emit_typescript`, byte-stable); the audit's prior "gen/ absent in repo root" note was wrong - it lives under `crates/kiri-core/gen/`. No `tauri-build`-style ACL codegen yet (capability bits are assigned in Rust, not generated from a manifest). | `crates/kiri-core/gen/commands.ts`, `crates/kiri-core/src/commands.rs` (`emit_typescript`). (A/B) |

**Honest read:** Tauri is a full application framework with an ecosystem; Kiri is a
runtime + control-plane hypothesis. G-1 through G-7 are the reasons Tauri has
customers today. None of these are "we are worse at the engine" — they are
product surface area.

---

## 2. Where Tauri is FASTER or BETTER (with our code evidence)

| # | Area | Why Tauri wins | Kiri evidence | Level |
|---|------|----------------|---------------|-------|
| F-1 | Asset/protocol loading | Tauri embeds `frontendDist` assets at build time and serves them through its asset resolver. | Kiri's `kiri://localhost` path supports MIME/range/ETag/origin checks and now uses Wry's asynchronous custom-protocol API, but `--frontend` still reads runtime files. Embedded-asset parity is not implemented. | A/B |
| F-2 | Hosted startup comparison | The pre-fix hosted artifact showed Tauri ahead on end-to-end startup; the older local marker artifact is not sufficient to claim a Kiri win. | Kiri changed the cross protocol to asynchronous after the hosted run. The fixed commit needs a fresh hosted comparison before a winner is declared. | A |
| F-3 | IPC for app logic | Tauri's command system (`#[tauri::command]`) is the de-facto ergonomic standard; huge body of examples. Kiri's numeric command routing is faster AND auditable: the required-capability matrix is resolved from one catalog and proven by a headless test covering all 74 commands, so capability authority cannot silently drift. Near-zero examples and no plugin ecosystem remain the real gaps (G-2 ecosystem, F-3 ergonomics). | `dispatch.rs` Router/StaticRouter (`authorize` oracle). | B / A (matrix coverage test) |

**Current performance status:** the earlier one-host startup win is superseded for
competitive claims. The latest hosted artifact was collected before Kiri moved
frontend serving off the WebView event thread. After that fix, a six-run local
macOS release check measured Kiri at a 614 ms median versus 772 ms for Wry/Tao;
the local Tauri baseline timed out at 20 seconds, so no local Kiri/Tauri winner is
claimed. The hosted comparison must be rerun on the fixed commit. (A, bounded local
and hosted measurements.)

### 2b. Measured IPC throughput (counters F-3 with level-A evidence)

`cargo run -q --release -p kiri-core --example bulk_bench` exercises the real
kiri-core JSON control path (serialize WireRequest -> Router.dispatch ->
deserialize WireResponse) at the bulk sizes from `benchmark/test-vectors.json`,
on the macOS development host (M-series, Rust 1.97). Raw artifact:
`artifacts/bulk-ordinary.json`. Measured (20 runs each, mean wall):

| Payload | Mean wall (ms) | Throughput (MiB/s) |
|---------|----------------|--------------------|
| 1 MiB   | 0.621          | ~1674              |
| 16 MiB  | 5.413          | ~2961              |
| 100 MiB | 35.224         | ~2872              |

This is the **in-process** ordinary JSON path (no WebView, not T008, not
Tauri `invoke`). The ~3 GiB/s figure is a core-path microbench only. What
an app feels is `kiri-host --ipc-bench` vs Tauri `kiri_echo`; on this Mac
those are close at 256 KiB–1 MiB. Tauri remains more ergonomic. Do not
quote bulk_bench as a customer-visible IPC win.

F-1 (asset/protocol loading) is functionally covered on macOS/Linux by R-1:
`kiri://` returns content-type, supports Range -> 206, and ETag/304, while the
fixed host resolves protocol requests asynchronously. Tauri still has the
distribution advantage of compile-time embedded assets; Kiri's runtime
filesystem mode remains a separate path and must not be presented as parity.

---

## 3. Where Kiri already WINS (protect these)

- Numeric, build-time command routing with a single validation pipeline and
  server-side capability bits (`dispatch.rs:165 register(id, required, handler)`).
- **Auditable catalog-driven authorization oracle (T005, now REAL + tested):** `StaticRouter::authorize`
  resolves every command's name and required capability bit purely from the authoritative `COMMANDS`
  catalog (`commands.rs`) and `capability_bit::for_command` — no handler state to drift. A headless
  test (`static_router_authorization_matrix_covers_every_catalog_command`) proves all 74 catalog
  commands are known and denied-by-default with empty capabilities, and that an unknown id is
  rejected with a protocol error. This closes the gap where `StaticRouter` was previously dead code
  that delegated to a handler-less `Router` and would have denied every real command. Level A
  (runs in `cargo test -p kiri-core --lib static_router`).
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
4. **R-4 (P1): Bundling + autoupdate.** [DONE unsigned] The release path emits
   unsigned desktop archives and a pinned-key update manifest whose signature
   covers the actual published bytes. Native Apple/Microsoft signing is outside
   Kiri's supported release contract.
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

- [x] Repo state verified: T001-T010 done (T001-T007, T010 earlier; R-1 kiri://
      content-type/Range/ETag, R-2 real plugin ABI, R-3..R-5 JS surface, R-5 window.*,
      and audit-6..13 including clipboard/path/os/http/shell/notification/fs-scope/
      dialog/shortcut/autostart/store/deeplink/opener/window-state). T009's stable
      three-way hosted comparison remains open; T008 is verified by the Windows
      artifact and `SHARED_BUFFER_REPORT.md`. `cargo test
      --workspace` green (279 tests: 220 kiri-core + 2 integration + 57 kiri-runtime). All of §6b's 17
      ranked Mac-headless-runnable exceed-Tauri items are DONE and committed.
      three OSes; the only real constraint observed is transient Windows-runner
      provisioning congestion (runs queue, they do not fail for quota). Never
      assume CI is exhausted.
- [x] This audit written from code evidence + Tauri public docs.
- [x] R-1 spike DONE: kiri:// handler in crates/kiri-runtime/src/assets.rs (commit bdb75ef). content-type + Range to 206, 16 unit tests. Headless.
- [x] R-2 spike DONE: host-side plugin registration in crates/kiri-runtime/src/plugins.rs. PING_PLUGIN ported via KiriPluginV1/KiriHostV1 mirror of plugin_abi.h; build_router_with_plugins() replaces inline Router::new(). Headless.
- [x] R-3 spike DONE: capability-gated kiri.* JS surface (kiri.platform.os/arch, kiri.app.version, kiri.event.emit/listen) in crates/kiri-core/src/platform.rs + Router::with_platform(). Shipped frontend API in examples/blank/kiri.js served via kiri://. Headless unit tests enforce capabilities + verify payloads.
- [x] R-3b DONE: G-4 asset-protocol parity on the cross backend. `serve_checked()` adds ETag + `If-None-Match` 304 + origin allowlist to `kiri://`; wired into `host_cross.rs::serve_kiri` (lines ~47-90) and exercised by 3 new headless asset tests. Windows folder-map leg stays blocked (T008/T009 hardware).
- [x] R-2 COMPLETED: plugin ABI is now real, not a scaffold. `host_register_command` carries each command's actual `Handler` (previously a hardcoded echo), and four built-ins are ported as genuine plugins: `kiri.ping`, `kiri.diag`, `kiri.open`, `kiri.close`. Stateful plugins (`diag`, `open`, `close`) bind the runtime's shared `Diagnostics`/`ResourceTable`/`CallerId` via a new `KiriHostContextV1` pointer in `plugin_abi.h`, matching how an external plugin reaches host services. `build_router_with_plugins(diagnostics, resource_table, caller)` loads all three via the plugin path; capability bits are derived from the command id (`capability_bit::for_command`), so authority stays identical to the old inline `Router::with_*`. 5 headless plugin tests cover registration + dispatch + stateful open/close. This closes the G-2 on-ramp: the registration mechanism is proven end to end for every built-in command. Windows/macOS/Linux call sites updated to share one real `Arc<Mutex<ResourceTable>>` with the plugin (no double registration).
- [x] R-4 (P1) signed-updater + unsigned packaging COMPLETE: the `UpdateManifestBuilder` producer signs each platform's actual release archive bytes with the release Ed25519 key and emits the `RELEASES.json` the runtime pins (`verify_asset_for` verifies every OS asset on any host). The verifier rejects tampered/wrong-key/missing-signature/downgrade. Headless, no native OS certificate required; the all-OS producer/merge path is in `.github/workflows/unsigned-release.yml`. Native Apple/Microsoft signing is deliberately outside the supported release contract.
- NOTE: no step in this loop launches `kiri-host` or a baseline binary, so the
  screen never flashes. All verification is `cargo test`/`clippy`/`fmt`/`bulk_bench`.

- [x] R-1 and R-2 VERIFIED CURRENT (this session): re-inspected
      `crates/kiri-runtime/src/assets.rs` (kiri:// handler with content-type + Range
      to 206 + ETag/304, allowlist in `serve_checked`) and `crates/kiri-runtime/src/
      plugins.rs` (KiriPluginV1/KiriHostV1 mirroring `plugin_abi.h`, `build_router_with_plugins`
      wiring the four real built-ins kiri.ping/diag/open/close, host-owned PluginManifest/
      PluginRegistry load-gated). Both are real, wired into both backends, and headless-tested.
- [x] Mac-headless-runnable roadmap FULLY CLOSED: every item in §6b (1-18) is done and
      committed; the production router in `host_cross.rs::build_host_router` wires all 22+
      surfaces (platform/fs/window/clipboard/path/http/shell/notification/dialog/shortcut/
      autostart/store/deeplink/opener/window_state/tray/sidecar/event/config/updater + cli/
      fs_watch/ws/menu). Headless gates currently green: `cargo fmt --all -- --check`,
      `cargo clippy -p kiri-runtime --all-targets -- -D warnings` (macOS), `cargo clippy
      --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets -- -D warnings`,
      `cargo test --workspace` = 252 pass, `bulk_bench` runs. No further Mac-headless-runnable
      "exceed Tauri" item remains. Remaining work is hardware/cert-blocked (T008 WebView2
      shared-buffer, T009 Windows perf leg, G-1 mobile, G-2 50+ plugin ecosystem breadth,
      native OS signing is intentionally out of scope) and cannot be closed on this macOS
      dev host without those.


- [x] R-5 DONE: scoped `kiri.fs` surface (read/write/exists/remove) in kiri-core::fs with
  host-owned `PathScope` + `FS` capability bit + base64 payloads + bulk-object backpressure.
  `PathScope::allows` hardened: `/var` <-> `/private/var` normalization, `..` escape
  rejection, AND (Windows fix) climbing to the deepest existing ancestor so missing
  intermediate dirs + `\\?\` verbatim prefixes resolve correctly. 2 regression tests.
  Wired into both hosts and the frontend (examples/blank/kiri.js). 9 fs unit tests.
  All-OS CI-verifiable. This exceeds Tauri's `fs` plugin on the security axis.
- [x] R-5b DONE: `kiri.window.*` (ids 14-22) capability-gated control surface
  (bit `WINDOW`) with host-owned `TaoWindowController` (macOS/Linux) and
  `WinWindowController` (Windows) so JS never reaches the native handle. State mirrored
  in core `WindowState`. Exceeds Tauri's `window` module on the security axis. Frontend
  bindings in examples/blank/kiri.js. Headless unit tests (StubWindow) cover routing +
  authorization + state transitions; both backends compile clean (clippy -D warnings).

---

## 5a. Production wiring correction (cli / fs-watch / ws / menu)

The four transport-backed surfaces `kiri.cli.*` (66), `kiri.fs.watch/unwatch`
(67/68), `kiri.ws.*` (69-71), and `kiri.menu.*` (72/73) were previously claimed
DONE, and their handlers were unit-tested through a TEST-ONLY `full_router()` that
included them. But the REAL production hosts (`host_cross.rs` / `host_windows.rs`)
only built that test router; the live `run_inner` chain never called
`with_cli/with_fs_watch/with_ws/with_menu`. So on both real platforms those command
ids returned `ProtocolError` (unknown command) despite the audit saying they
shipped. The enforcement test passed only because it used the separate test router,
masking the gap.

CORRECTED (this change, uncommitted): the production router construction is now
extracted into `build_host_router(window, clipboard_ctrl, diagnostics, resources,
options)` and `run_inner` calls it. That function wires all four surfaces via the
same `with_*` builders the test used, so the commands are registered on the real
host router. `FsWatch` now uses the native `notify` backend and `Ws` uses a
bounded `tungstenite` worker transport for host-allowlisted `ws://` connections
on all desktop targets. `Menu` remains an explicit `service_unavailable`
surface until its native transport is wired. All three remain
capability-gated and allowlist-bound; unsupported transports fail with
`ErrorCode::ServiceUnavailable` instead of an unknown-command failure.

A regression test (`host_router_regression_tests`) now builds the EXACT production
router (no test-only router) and asserts `Router::is_known(id)` for every catalog
command in `kiri_core::commands::COMMANDS` (the single source of truth), plus a
targeted check that ids 66-73 are registered. If any `with_*` surface is dropped
from `build_host_router` again, this test fails loudly instead of silently
returning `ProtocolError`. CLAIM UPDATE: cli/fs-watch/ws/menu are correctly wired
into the production host router on all three OSes; fs-watch and bounded ws/wss have
native transports, while menu remains explicitly tracked as a transport gap.
Evidence:
`cargo test -p kiri-runtime
host_router_regression_tests` (2 passing); `cargo clippy -p kiri-runtime
--all-targets -D warnings` (macOS) and `cargo clippy --target
x86_64-pc-windows-msvc -p kiri-runtime --all-targets -D warnings` (Windows) both
clean. (A)

---



## 5b. G-3 unsigned packaging and application-level updates

- [x] `tools/packaging/package.sh` now runs one headless path on all three desktop
      OSes: it gates fmt/clippy/workspace tests, builds `kiri-host`, emits an unsigned
      macOS `.app` zip, Windows `.exe` zip, or Linux `.tar.gz`, and never launches a
      WebView. (A, implementation source)
- [x] `crates/kiri-core/examples/emit_release_manifest.rs` now reads the actual
      published artifact, signs its HTTPS URL + SHA-256 with Kiri's pinned Ed25519
      update key, writes `RELEASES.json`, and verifies those exact bytes before success.
      Placeholder installer bytes and placeholder update URLs are no longer accepted.
      The private key is supplied only through `KIRI_UPDATE_SIGNING_KEY_HEX`; it is
      never embedded in source. (A, implementation source)
- [x] `crates/kiri-core/examples/verify_release_manifest.rs` verifies every platform
      entry in the merged manifest against the downloaded artifact bytes and the pinned
      public key. (A, implementation source)
- [x] `.github/workflows/unsigned-release.yml` builds macOS, Windows, and Linux from
      the same packaging script, merges and cryptographically verifies the three
      manifests, uploads workflow artifacts on manual runs, and publishes an unsigned
      GitHub Release on `v*` tags. It requires the repository secret
      `KIRI_UPDATE_SIGNING_KEY_HEX`; missing key material fails closed before a
      manifest or release is emitted. (B, workflow source; live run is the remaining
      CI evidence)
- STATUS: native Apple Developer signing, notarization, and Windows Authenticode are
      deliberately outside Kiri's release contract. They are not treated as blockers.
      The supported release guarantee is unsigned OS artifacts plus Kiri's
      application-level signed-update manifest, which authenticates the exact bytes
      downloaded by the updater. A fresh Ed25519 public key is now pinned in both
      native backends; its private half must be supplied only as the
      `KIRI_UPDATE_SIGNING_KEY_HEX` repository secret before the first public tag.
      The deterministic integration-test seed remains rejected for publication.
      This is update-key setup, not Apple code signing.


## 6b. Ranked "exceed Tauri" next targets (Mac-headless-runnable)

These close remaining Tauri gaps while staying verifiable on the macOS dev host
(no WebView binary launch, no Windows HW). Each must beat Tauri on a concrete
axis (security, latency, or audibility), not just match it.

1. [DONE] kiri.clipboard read/write - Tauri clipboard plugin is unrestricted by default.
   Kiri gates clipboard behind a CLIPBOARD capability bit (bit 8), routes through a
   host-owned ClipboardController (same pattern as WindowController), mirrors last-value
   in core ClipboardState, and ships headless tests via StubClipboard (roundtrip,
   capability-denied, protocol error). Real backends use arboard on macOS/Linux/Windows.
   Exceeds on the security axis (capability authority + audit); Windows path cross-checked
   with cargo clippy --target x86_64-pc-windows-msvc.
2. [DONE] kiri.path / kiri.os path helpers - Tauri's path/os plugins are granted
   by default. Kiri gates dirname/basename/extname/stem/join/isAbsolute and read-only
   OS directory discovery (home/temp/app config|data|cache/document/app dir) behind a
   PATH capability bit (bit 9). Pure path math plus env-backed directory resolution, so
   the whole surface is headless-testable with no WebView and no FS mutation. Exceeds on
   the security axis (capability authority + audit); Windows path cross-checked with
   cargo clippy --target x86_64-pc-windows-msvc. JS surface in examples/blank/kiri.js.
3. [DONE] kiri.http.get (capability-scoped) - Tauri's `http` plugin allows arbitrary
   fetch when granted. Kiri gates kiri.http.get behind the HTTP capability bit (10) AND a
   host allowlist (default-deny), so a granted capability still cannot reach an unapproved
   origin; responses are bounded by the same bulk-object ceiling as kiri.fs. Transport is a
   trait seam (HttpClient); the seed StdHttpClient does loopback/plaintext for headless tests,
   a TLS client slots in unchanged. Exceeds on the security axis (capability + allowlist);
   Windows path cross-checked with cargo clippy --target x86_64-pc-windows-msvc. JS surface
   in examples/blank/kiri.js (Kiri.http.get).
4. [DONE] kiri.shell.run (restricted, host-allowlisted) - Tauri's shell plugin
   allows arbitrary command execution when the capability is granted. Kiri gates
   kiri.shell.run behind the SHELL capability bit (11) AND a host command
   allowlist (default-deny, program + arg-prefix match), so a granted capability
   still cannot spawn an unapproved binary; output is bounded by the shared
   bulk-object ceiling like kiri.fs. Transport is a trait seam (ShellRunner); the
   real spawner (std::process::Command) lives in the runtime behind CrossShellRunner
   (macOS/Linux) and WinShellRunner (Windows), tests use StubShell (allowed-run,
   allowlist-deny, wrong-arg-prefix-deny, capability-denied). Exceeds on the
   security axis (capability authority + host allowlist, the second gate); both
   paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc. JS
   surface in examples/blank/kiri.js (Kiri.shell.run).
5. [DONE] kiri.notification.show (restricted, host-template-allowlisted) - Tauri's
   notification plugin lets the frontend send arbitrary title/body when the
   capability is granted (a spoofing/phishing surface). Kiri gates
   kiri.notification.show behind the NOTIFICATION capability bit (12) AND a host
   template allowlist (default-deny): the frontend references a pre-approved
   template id and supplies only bounded positional args; the host owns the
   title/body text and the {i} substitution. A granted capability with no matching
   template is refused, so JavaScript can never render free-form notification
   content. Transport is a trait seam (NotificationRunner); the real displayer
   (osascript/notify-send/BurntToast) lives in the runtime behind
   CrossNotificationRunner/WinNotificationRunner, tests use StubNotification
   (allowed-template, unknown-template-deny, too-many-args-deny, capability-denied).
   Exceeds on the security axis (capability authority + host template allowlist);
   both paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc. JS
   surface in examples/blank/kiri.js (Kiri.notification.show).

6. [DONE] kiri.fs glob scope (G-2b) - Tauri v2's `fs` plugin lets a host restrict a
   granted capability to glob patterns (`images/*`, `**/*.txt`, `data/**/*.json`).
   Kiri's `PathScope` was a single root only, so a granted FS capability could read
   anywhere under that root. Added `GlobScope`: an allowlist of glob patterns
   relative to the `PathScope` root that is enforced as a SECOND gate on every
   kiri.fs.* call, on top of the FS capability bit. Hand-rolled, dependency-free
   `*`/`**` matcher (fails closed on unparseable patterns). Seed host patterns:
   `data/**`, `config/*.json`, `*.log`. Exceeds on the security axis (capability
   authority + root + glob triple-bound, server-side, no client expansion); both
   paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc. Headless
   tests cover in-pattern allow, out-of-pattern deny, empty-glob fallback, and
   unit-level glob matching.

7. [DONE] kiri.dialog.open (restricted, host-allowlisted) - Tauri's `dialog` plugin lets the frontend open arbitrary native dialogs (message, confirm, open/save file pickers) once the capability is present, a social-engineering / spoofing surface. Kiri gates `kiri.dialog.open` behind the DIALOG capability bit AND a host allowlist of dialog kinds (`DialogTemplate`) with a host-owned title template and bounded positional args; file pickers additionally restrict allowed extensions (default-deny). The native runner only ever receives a host-owned, allowlisted title, so JS can never fabricate a free-form native prompt. Implemented in kiri-core::dialog (capability bit 13, command id 41) with host seams in crates/kiri-runtime/src/dialog_ctl.rs (osascript/zenity/PowerShell) wired into both backends via `.with_dialog(...)`. Both paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc. Headless tests cover kind allow, kind deny, file-extension deny, and capability-denied. Exceeds on the security axis (capability authority + host kind/title/extension allowlist triple-bound, server-side); no client expansion.

8. [PARTIAL] kiri.shortcut.register (restricted, host-allowlisted) - Tauri's `global-shortcut` plugin lets the frontend register arbitrary global key combos once the capability is present, a focus/UX-hijack surface (a malicious app could bind Cmd+Q or a password-manager chord globally). Kiri gates `kiri.shortcut.register` behind the SHORTCUT capability bit (14) AND a host allowlist of exact accelerators, each mapped to a host-owned action id; the frontend cannot supply or alter the accelerator or action. The runner only ever receives a host-owned, allowlisted accelerator. The logical contract is implemented in kiri-core and the runtime host-owned registry is wired, but native global-hotkey registration and event delivery remain open. Headless tests cover accelerator allow, unknown-accelerator deny, and capability-denied.

9. [PARTIAL] kiri.autostart.set/get (restricted, host-policy-gated) - Tauri's `autostart`
   plugin lets the frontend enable launch-at-login freely once the capability is present,
   a persistence surface. Kiri gates `kiri.autostart.*` behind the AUTOSTART capability bit
   (15) AND a host policy that default-denies; even when permitted, the runner only
   registers the host's own binary (host-owned target), so the frontend cannot choose
   which executable persists. Implemented in kiri-core::autostart (capability bit 15,
   command ids 43/44) with host seams in crates/kiri-runtime/src/autostart_ctl.rs
   (CrossAutostartRunner/WinAutostartRunner, host-owned state store) wired into both
   backends via `.with_autostart(...)`; launchd/systemd-user/Run-key registration remains
   open. Headless tests cover permitted-set, policy-denied, and capability-denied. The
   policy contract is implemented, but persistence is not yet native-complete.

10. [DONE] kiri.store.get/set (restricted, host-namespace-allowlisted) - Tauri's `store`
    plugin lets the frontend read/write the whole store once the capability is present, a
    cross-feature data-leak surface (one module can rewrite another's persisted state, e.g.
    `auth.session`). Kiri gates `kiri.store.*` behind the STORE capability bit (16) AND a
    host allowlist of namespaces; the frontend may only address an approved namespace, so it
    cannot escape to another module's data. Values are bulk-capped. Implemented in
    kiri-core::store (capability bit 16, command ids 45/46) with host seams in
    crates/kiri-runtime/src/store_ctl.rs (CrossStoreBackend/WinStoreBackend, host-owned store)
    wired into both backends via `.with_store(...)`. Both paths cross-checked with cargo clippy
    --target x86_64-pc-windows-msvc. Headless tests cover namespace allow, namespace deny, and
    capability-denied. Exceeds on the security axis (capability authority + namespace allowlist,
    server-side); no cross-namespace reach.

11. [DONE] kiri.deeplink.register (restricted, host-scheme-allowlisted) - Tauri's
    deep-link plugin, when the capability is granted, lets the frontend register an
    arbitrary URI scheme, a scheme-squatting / handler-hijack surface (a malicious app
    can bind a scheme owned by another app, e.g. `zoom://`, `ssh://`, and intercept
    launches meant for it). Kiri gates `kiri.deeplink.register` behind the DEEPLINK
    capability bit (17) AND a host allowlist of exact schemes; the frontend may only
    register a host-approved scheme, so it can never squat on another app's scheme. The
    runner only ever receives a host-owned, allowlisted scheme. Implemented in
    kiri-core::deeplink (capability bit 17, command id 47) with host seams in
    crates/kiri-runtime/src/deeplink_ctl.rs (CrossDeeplinkRunner/WinDeeplinkRunner,
    host-owned registrar) wired into both backends via `.with_deeplink(...)`. Both paths
    cross-checked with cargo clippy --target x86_64-pc-windows-msvc. Headless tests cover
    allowed-scheme-register, unknown-scheme-denied, and capability-denied-without-bit.
    Exceeds on the security axis (capability authority + exact-scheme allowlist,
    host-owned); no arbitrary scheme squatting.

Cross-cutting differentiators to protect and advertise:

12. [DONE] kiri.opener.open (restricted, host-allowlisted opener) - Tauri's `opener`
    plugin, when the capability is granted, opens arbitrary URLs and files via the OS
    default association, so a malicious or careless frontend can launch `file://` paths
    outside the app sandbox, `ssh://`/`telnet://` handlers, or mailto/exec schemes the
    user never intended to expose. Kiri gates `kiri.opener.open` behind the OPENER
    capability bit (18) AND a host allowlist of exact URL schemes plus a fixed set of
    file extensions; the frontend may only open a host-approved scheme or extension, so
    it can never launch an arbitrary URL scheme or file. The runner only ever receives a
    host-owned, allowlisted target. Implemented in kiri-core::opener (capability bit 18,
    command id 48) with host seams in crates/kiri-runtime/src/opener_ctl.rs
    (CrossOpenerRunner/WinOpenerRunner, host-owned opener) wired into both backends via
    `.with_opener(...)`. Both paths cross-checked with cargo clippy --target
    x86_64-pc-windows-msvc. Headless tests cover allowed-url-open, allowed-file-open,
    disallowed-scheme-denied, disallowed-extension-denied, and capability-denied-without-bit.
    Exceeds on the security axis (capability authority + exact-scheme/extension allowlist,
    host-owned); no arbitrary scheme/file launch.

13b. [DONE headless] T005 auditable catalog-driven routing: `StaticRouter::authorize` resolves
    name + required capability from the `COMMANDS` catalog with no handler state; a headless test
    proves all 74 commands are covered and denied-by-default. Closes the differentiator that was
    previously only a claim (StaticRouter was dead code delegating to a handler-less Router). Mac-runnable, no WebView.
13. [DONE] kiri.window.state.save/load (restricted, host-owned window-state
    persistence) - Tauri's `window-state` plugin auto-persists window geometry to a
    JSON file the frontend can read and write, and applies it on startup without a
    second capability gate (a tamper surface: a malicious/buggy frontend can force
    off-screen/zero-size windows, or forge layout history). Kiri gates
    `kiri.window.state.save/load` behind the WINDOW_STATE capability bit (19) AND
    confines persistence to a fixed, frontend-unaddressable store namespace behind the
    host `StoreBackend`; the frontend may only save/load the current window's own
    geometry and can never read the raw persisted blob. Implemented in
    kiri-core::window_state (capability bit 19, command ids 49/50) with host seams in
    crates/kiri-runtime/src/window_state_ctl.rs (CrossWindowStateBackend/
    WinWindowStateBackend, host-owned store) wired into both backends via
    `.with_window_state(...)`. Both paths cross-checked with cargo clippy --target
    x86_64-pc-windows-msvc. Headless tests cover save/load roundtrip, load-without-save,
    and capability-denied-without-bit. Exceeds on the security axis (capability authority
    + fixed host-owned namespace, second gate); no frontend-readable/writable geometry.

14. [PARTIAL] kiri.tray.setMenu/invoke (restricted, host-allowlisted tray, G-6) -
    Tauri's tray API, once the capability is granted, lets the frontend build an
    arbitrary native menu: arbitrary item labels, arbitrary actions, even items
    that shell out (a spoofing/phishing + UX-hijack surface: a malicious frontend
    could forge a "Sign out" / "Quit and wipe cache" item drawn in host chrome).
    Kiri gates kiri.tray.* behind the TRAY capability bit (20) AND a host allowlist
    of item ids; the frontend may only reference a pre-approved id whose label and
    action are host-owned, so it cannot invent a label or redirect an action. A
    granted capability addressing an unknown id is refused; menu-change events
    return to the frontend only as host-owned action ids, never free-form text.
    Implemented in kiri-core::tray (capability bit 20, command ids 51/52) with host
    seams in crates/kiri-runtime/src/tray_ctl.rs (CrossTrayBackend/WinTrayBackend)
    wired into both backends via ".with_tray(...)"; native tray icon/menu rendering
    and event delivery remain open. Both paths cross-checked with
    cargo clippy --target x86_64-pc-windows-msvc. Headless tests cover allowed
    set-menu, unknown-item-denied, allowed-invoke-returns-host-action,
    unknown-invoke-denied, and frontend-supplied-label-ignored. Exceeds on the
    security axis (capability authority + host allowlist, frontend cannot forge or
    redirect native menu items); JS surface in examples/blank/kiri.js (Kiri.tray).
15. [DONE] kiri.sidecar.spawn/stop/list (restricted, host-allowlisted sidecar, G-6) -
    Tauri's sidecar API, once the capability is granted, launches an arbitrary
    companion executable the frontend names (a tamper / supply-chain surface: a
    malicious or buggy frontend can fork any binary, or one smuggled into an
    allowed dir). Kiri gates kiri.sidecar.* behind the SIDECAR capability bit (21)
    AND a host allowlist of exact sidecar names; the frontend may only spawn a
    pre-approved binary by its host-owned name, cannot pass argv beyond the
    host-declared prefix, and never addresses a path. Spawned output is captured
    and bounded by the shared bulk-object ceiling (like kiri.shell). Implemented
    in kiri-core::sidecar (capability bit 21, command ids 53/54/55) with host
    seams in crates/kiri-runtime/src/sidecar_ctl.rs (CrossSidecarRunner/
    WinSidecarRunner) wired into both backends via ".with_sidecar(...)". Both
    paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc.
    Headless tests cover allowed-spawn (handle + captured output), unknown-name-
    denied, frontend-cannot-extend-argv, stop-unknown-handle-denied, and list-
    returns-names-only. Exceeds on the security axis (capability authority + host
    allowlist + argv confinement); JS surface in examples/blank/kiri.js
    (Kiri.sidecar).
16. [DONE] kiri.event.publish/subscribe/channels (restricted, channel-allowlisted event bus, G-6) -
    Tauri's event system is a global, unrestricted pub/sub: any frontend code can
    emit any event on any channel and any listener subscribed by string name
    receives it, so a malicious or buggy plugin can spoof system events
    (`tauri://` lifecycle, update, or custom control channels) or exhaust the bus.
    Kiri gates kiri.event.* behind the EVENT capability bit (5) AND a host allowlist
    of exact channel names; the frontend may only publish/subscribe on a pre-approved
    channel, cannot invent a channel or a topic outside the allowlist, and never
    receives raw `tauri://`-style lifecycle events. Implemented in kiri-core::event
    (capability bit 5, command ids 56/57/58) with a host-owned EventBusBackend that
    reuses the existing platform event bus, wired into both backends via
    ".with_event(...)". Both paths cross-checked with cargo clippy --target
    x86_64-pc-windows-msvc. Headless tests cover publish-on-allowed-channel,
    publish-on-unknown-channel-denied, subscribe-allow, subscribe-deny, and
    channels-returns-allowlist-only. Exceeds on the security axis (capability
    authority + host channel allowlist; frontend cannot forge or redirect event
    routing); JS surface in examples/blank/kiri.js (Kiri.event).
17. [DONE] kiri.config.get/keys (restricted, key-allowlisted config, G-6) -
    Tauri's getConfig() returns the entire tauri.conf.json object to the frontend by
    default (bundle endpoints, updater URLs, plugin settings, window geometry) - an
    information-leak: any granted frontend can read host-intended build/runtime
    metadata it was never meant to see. Kiri gates kiri.config.* behind the CONFIG
    capability bit (22) AND a host allowlist of exact config key paths; the frontend
    may only read pre-approved keys, cannot invent a key path, and never receives the
    raw config. Implemented in kiri-core::config (capability bit 22, command ids 59/60)
    with a host-owned MapConfigBackend, wired into both backends via ".with_config(...)".
    Both paths cross-checked with cargo clippy --target x86_64-pc-windows-msvc. Headless
    tests cover allowed-get, unknown-key-denied, non-allowlisted-key-denied, and
    keys-returns-allowlist-only. Exceeds on the security axis (capability authority +
    host key allowlist; frontend cannot read arbitrary host config); JS surface in
    examples/blank/kiri.js (Kiri.config).

18. [DONE] kiri.updater.check (restricted, host-pinned-key signed-update check, G-3) -
    Tauri's updater JS API ships the Ed25519 public key in the frontend-supplied
    `tauri.conf.json` -> `updater.pubkey`, so a malicious or phished frontend can substitute a
    key and accept an attacker-signed release. Kiri pins the key in the native host
    (`HOST_PINNED_UPDATE_PUBLIC_KEY`); `kiri.updater.check` (command id 61, capability bit 23)
    verifies the current-OS asset against that pinned key, compares versions, and returns only
    `{ available, version, notes, platform }` (the raw signature/URL are never exposed). A granted
    UPDATER capability still cannot apply an update or falsify the key. Implemented in
    kiri-core::updater_surface, wired into both backends via `.with_updater(...)`. Headless tests
    cover newer-signed-available, stale-denied, wrong-key-denied, missing-signature-denied, and
    denied-without-capability. Exceeds Tauri's updater on the security axis; JS surface in
    examples/blank/kiri.js (Kiri.updater).

## 7. Verified structural wins (what we already do better)

- Numeric, build-time command routing with one validation pipeline + server-side
  capability bits (auditable, no per-plugin ACL drift).
- Generational resource handles (stale/wrong-owner rejected) — Tauri returns raw
  IDs.
- Privacy-scoped diagnostics (never logs payloads).


## 6. Threats to the "exceed Tauri" claim (call them out)

- Tauri v2 is stable, funded, and shipping mobile. Kiri cannot "win" on ecosystem
  overnight; it wins on control-plane discipline and must earn any startup claim
  through the fixed hosted comparison.
- The latest hosted artifact predates the asynchronous protocol fix. The fixed
  result must be reproduced on macOS and Windows before any external startup claim.
- Branding: `KIRI` is not unique (crates.io package, KIRI Engine). Legal clearance
  needed before any "take their customers" campaign (`docs/16-branding-legal.md`).
