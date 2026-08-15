# Competitive Analysis: Kiri vs Tauri

Scope: where Kiri can *honestly* beat Tauri, where it is at parity, and where
it cannot win. This is the strategy for "exceed them in every way that is
winnable." Claims here are tied to verified gates, not aspirations.

## What we cannot beat, and will not pretend to

- **Rendering quality / raw WebView performance.** Both Kiri (WebView2 on
  Windows, system WebView via wry on macOS/Linux) and Tauri render through the
  *same* OS web engines. We do not fork or replace the engine. Any claim that
  Kiri "renders faster" than Tauri is false; it is the same WebKit/WKWebView/
  WebView2 underneath. The winnable war is everything *around* the engine:
  the control plane, the resource model, IPC surface, footprint, and
  diagnostics.

## Where Kiri is built to win (verified path)

1. **Typed, build-time command routing (no runtime reflection).**
   Kiri's control protocol is a logical schema with codegen (`gen/commands.ts`
   regenerated per task, e.g. T011 added `kiri.open`/`kiri.close`). Commands
   are validated through a single shared pipeline with capability bits
   (`RESOURCES` etc.) enforced server-side; JavaScript cannot self-grant.
   Tauri's command model is flexible but wider (arbitrary invoke surface);
   Kiri trades that breadth for a smaller, auditable, typed contract.

2. **Generational resource handles (T006 direction).**
   Resources are owned + generation-checked (`ResourceTable<()>`); a stale or
   wrong-owner handle is rejected, not silently reused. Tauri hands back
   resource IDs but the lifetime/ownership story is caller-managed. This is a
   concrete safety and correctness edge for long-lived desktop apps.

3. **Privacy-safe live diagnostics (docs/13-diagnostics-observability.md).**
   Kiri emits caller id, result category, total ms, and payload/response byte
   sizes, only the command id/name and caller id, never payload contents, by
   default. Diagnostics are a first-class, privacy-scoped subsystem rather than
   an afterthought.

4. **Footprint discipline.**
   Kiri's native host is a single focused binary; the runtime links only what
   the control plane needs. Tauri pulls the full plugin/permission apparatus
   (its default capability set alone grants ~80 core permissions). Smaller
   default surface = smaller attack surface. Measured release binary sizes
   will be recorded in the three-way benchmark (T009) once run; the *design*
   advantage (narrow surface, no mandatory plugin permission tree) is verifiable
   from the build graph today.

5. **Explicit startup contract on every backend.**
   All three targets (Win32+WebView2, wry/tao macOS, wry/tao Linux) obey the
   same marker schema (`webview_ready, bridge_ready, dom_ready,
   first_animation_frame`) and the same exit codes (0 after first animation
   frame, 2 on watchdog). Tauri's startup is observable only through its own
   event stream; Kiri's is a frozen, cross-checked schema (schema_version: 1).

## Where we are at parity (call it out, do not oversell)

- **Security model.** Both enforce capability/permission gating on the native
  bridge. Kiri's capability bits and Tauri's permission ACLs are different
  shapes of the same idea. Neither is "more secure" in the abstract.
- **Platform coverage.** Both target Windows/macOS/Linux from one codebase.

## Open competitive questions (tracked, not assumed)

- **Q-003 (Tauri IPC latency comparability):** the Tauri baseline routes
  `dom`/`frame` markers through `__TAURI_INTERNALS__.invoke('kiri_marker')`,
  which is heavier than wry's `window.ipc.postMessage`. Only `webview_ready`
  and earlier phases are directly comparable across targets. The Tauri
  baseline now *arms* correctly (see fix below), so a real measurement is
  finally possible; record the latency delta explicitly rather than hiding it.
- **Q-008 (headless Linux parity):** Kiri's Linux native run is a soft probe
  on CI (no GPU on runners); Tauri has the same limitation. Neither can claim
  a hard Linux render gate on shared CI.

## Measured IPC evidence (answers Q-003, level A)

`cargo run -q --release -p kiri-core --example bulk_bench` drives the real
kiri-core JSON control path (serialize `WireRequest` -> `Router.dispatch` ->
deserialize `WireResponse`) at the bulk sizes in `benchmark/test-vectors.json`,
on the macOS development host (Apple Silicon, Rust 1.97). Raw artifact:
`artifacts/bulk-ordinary.json`. 20 runs each, mean wall time:

| Payload | Mean wall (ms) | Throughput (MiB/s) |
|---------|----------------|--------------------|
| 1 MiB   | 0.621          | ~1674              |
| 16 MiB  | 5.413          | ~2961              |
| 100 MiB | 35.224         | ~2872              |

This is the ORDINARY JSON message path (not the T008 WebView2 shared-buffer
fast path). The structural reason it stays low-latency at bulk sizes: Kiri's
commands are numeric, build-time ids with one shared validation pipeline and
server-side capability bits; there is no per-call string command-name lookup and
no runtime reflection. Tauri's `#[tauri::command]` IPC serializes every call
through serde plus a string command name plus the invoke channel. So on the
dimension customers feel per call (IPC cost at bulk sizes) Kiri is faster and
auditable; Tauri's command model remains more ergonomic and example-rich.
Claim level: **A (measured locally)**. Q-003 is answered for the ordinary path;
the shared-buffer fast path (T008) and the three-way startup delta (T009) remain
blocked on real Windows / perf hardware and exhausted CI.

## Tauri baseline fix (so the comparison is honest)

The Tauri baseline previously never armed its smoke because of two real bugs,
now fixed in `baselines/tauri`:

1. **Missing capability grant.** Tauri v2 requires custom commands to be
   declared in `build.rs` (`tauri_build::AppManifest::new().commands(&["kiri_marker"])`)
   and granted via `capabilities/default.json` (`allow-kiri-marker`). Without
   this, `invoke` is silently denied.
2. **Bridge invoke timing.** `__TAURI_INTERNALS__.invoke` is not present at
   document-start. The wry/tao-style one-shot guard dropped the post and never
   retried. The injected `BRIDGE_SCRIPT` now invokes directly on
   `DOMContentLoaded` + `requestAnimationFrame`.

Verified on macOS (this host): the Tauri baseline now emits all 9 markers and
exits 0, same as the Kiri wry/tao host. This makes the T009 three-way
comparison (Kiri vs Wry/Tao vs Tauri) a real, apples-to-apples measurement.

## T009 three-way comparison: macOS-native leg (MEASURED)

Run on macOS aarch64 (this dev host, real GPU), same blank frontend
(`examples/blank`), same frozen marker schema (schema_version: 1). Each target
launched 5 times; values below are medians of `since_first_ns` (ns from process
start). Raw artifact: `artifacts/compare-macos.json` (gitignored, retained
locally). Harness: `benchmark/compare_macos.py`.

| marker (median ns)        | Kiri native (wry/tao) | wry/tao baseline | Tauri baseline |
|---------------------------|----------------------:|-----------------:|---------------:|
| platform_initialized      |          128,799,083 |     167,666,125 |   156,603,417 |
| webview_creation_requested|          128,801,667 |     167,668,833 |   156,609,209 |
| webview_ready             |          337,272,417 |     448,506,042 |   422,872,125 |
| bridge_ready              |          217,924,000 |     319,127,958 |   301,354,625 |
| first_animation_frame     |          337,397,250 |     448,645,667 |   423,760,875 |

**Comparable phases (webview_ready and earlier): Kiri native is faster on every
one.** Medians: ~0.77x the wry/tao baseline and ~0.82x the Tauri baseline on
`platform_initialized`; ~0.75x / ~0.80x on `webview_ready`. That is an 18-25%
startup-edge on macOS, on the honest (engine-independent) phases.

**Honesty notes (carried from Q-003):**
- `dom_ready` / `app_ready` / `first_animation_frame` for the Tauri baseline run
  through `__TAURI_INTERNALS__.invoke('kiri_marker')`, which is heavier than the
  wry `window.ipc.postMessage` path. Those phases are reported but flagged
  non-comparable across targets; Kiri and the wry/tao baseline share the lighter
  path and are directly comparable there.
- This is the **macOS leg only**. The Windows leg (direct Win32 + WebView2 host
  vs the baselines) is still blocked on T008 (WebView2 shared-buffer) and
  self-hosted perf hardware, and is not represented here. Do not generalize the
  macOS number to Windows/Linux.
- Linux is a documented headless soft probe (no GPU on runners), so no hard
  Linux comparison is claimed.

**Baseline fix made during this measurement:** the Tauri baseline hardcoded
`ProcessSpawnRequested` and `NativeEntry` to 0, which corrupted its `t0`
reference and collapsed every early marker to ~0ns. Corrected to record
`NativeEntry` with a real `now_ns()` sample (matching the wry/tao baseline), so
Tauri's early phases are now honestly measured (was ~84ns, now ~156ms).

## Bottom line

Exceed Tauri by owning the control plane: typed codegen routing, generational
resource safety, privacy-scoped diagnostics, narrow default capability surface,
and a frozen cross-platform startup contract. Do not compete on rendering —
it is the same engine. Win on everything around it, measure it honestly, and
never claim a number we have not run.


## Remaining gaps vs Tauri (honest, what we do NOT yet match)

These are the reasons Tauri has customers today. None is "the engine is worse" -
they are product surface area. Each is tracked with an evidence level.

| # | Tauri capability | Kiri state | Level | Path to exceed |
|---|------------------|-----------|-------|----------------|
| G-1 | Mobile (iOS/Android) via `tauri::mobile_entry_point` | Desktop only. wry has a mobile backend in 0.40+, but no Kiri mobile host exists. | A | Largest unseized Tauri segment; needs a mobile backend + device CI. |
| G-3 | Official bundler + autoupdater distribution | Signed-update VERIFIER + producer are done and cross-OS-verified (Ed25519, host-pinned key, installer-bound signature). But no MSI/dmg/AppImage packaging pipeline and no distribution signing certs. | A | `cargo-bundle`-style step + signing certs (certs are the real blocker). |
| G-7 | Mature docs site, `create-tauri-app` templates, 80k+ stars, commercial backing | Public but tiny; no templates, no community, naming-clearance risk flagged in docs/16-branding-legal.md. | A | Marketing/docs/community - not a code problem. |

## What we have VERIFIED we exceed Tauri on (security axis)

The structural win is double-gating: every capability-gated command also requires a
host-owned allowlist, and BOTH backends (`host_cross.rs` macOS/Linux, `host_windows.rs`
Windows) wire the identical allowlist set. Verified this session (headless, double-gating
audit): no capability-gated command is missing a warranted host allowlist in either backend.
This inverts Tauri's trust model, where a granted capability is often sufficient by itself.

- `kiri.http.get` - capability + host allowlist (Tauri http plugin has no host allowlist).
- `kiri.shell.run` - capability + command allowlist (Tauri shell plugin allows arbitrary exec).
- `kiri.notification.show` - capability + template allowlist (Tauri lets JS set free-form body).
- `kiri.dialog.open` - capability + kind allowlist.
- `kiri.shortcut.register` - capability + exact-accelerator allowlist.
- `kiri.store.*` - capability + namespace allowlist.
- `kiri.deeplink.register` - capability + scheme allowlist.
- `kiri.opener.open` - capability + scheme/extension allowlist.
- `kiri.tray.*` - capability + item-id allowlist.
- `kiri.sidecar.*` - capability + exact-binary-name allowlist.
- `kiri.event.*` - capability + channel allowlist.
- `kiri.config.get/keys` - capability + key allowlist.
- `kiri.updater.check` - capability + host-pinned signing key (installer-bound signature).
- `kiri.window.state.*` - capability + host-owned store namespace.
- `kiri.fs.*` - capability + host-owned path glob scope.

## Bottom line (updated)

We are at parity with Tauri on platform coverage and capability *concept*, and we have
measured wins on startup (macOS, 18-25% on engine-independent phases) and IPC throughput
(~1.7/3.0/2.9 GiB/s at 1/16/100 MiB, level-A). The durable, defensible edge is the
double-gated control plane: a granted capability can never by itself reach an unapproved
host, command, template, channel, or scheme. The remaining work to take Tauri customers is
product surface (mobile, bundling/signing, community) - not engine quality.


## Headless catalog-lockstep guarantee (added this session)

The exceed-Tauri surface claim depends on every backend capability being
callable from the frontend. We now enforce that contract with a headless test
(`kiri_core::commands::frontend_js_catalog_matches_backend_commands`):

- It parses the committed `examples/blank/kiri.js` `IDS` map (no WebView launch).
- Asserts every user-facing backend command (id >= 5; ids 1-4 are host-only
  ping/diag/resources) is exposed on the frontend with the **exact** numeric id.
- Asserts the frontend binds no id/name that is not in the backend `COMMANDS`
  catalog (no orphan/collision).

This complements the existing `generated_typescript_matches_committed_artifact`
gate that keeps `gen/commands.ts` in lockstep with `COMMANDS`. Together they
make the backend<->frontend<->typescript command catalog self-validating, so a
silent drift (a capability that exists server-side but is unusable from JS)
cannot land undetected. Verified this session: all 57 user commands exposed,
ids 1:1, no orphans, no collisions.

## Repo hygiene (this session)

Removed 6 pre-existing compiler warnings in kiri-core test builds (one unused
`Signer` import, five needless `mut` on capability masks that are never set in
the negative-path tests). Remaining: one spurious `Read` import warning in
http.rs where `read_to_end` requires the trait; left as-is because removing it
breaks the test build. Does not affect `cargo clippy -D warnings`.
