# Competitive Analysis: Kiri vs Tauri

> **Current hosted scoreboard:** `controlled-performance` run `32730288110` (commit `0d7e9a6`, `macos-latest` + `windows-latest`, 20 runs, 3 warmups/45 s for Wry/Tao) is the single current source. Historical tables below are retained but superseded; do not cherry-pick. See `CROSS_PLATFORM_STATUS.md:98` for Wry/Tao Windows caveat.

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
   default surface = smaller attack surface. The *design* advantage (narrow
   surface, no mandatory plugin permission tree) is verifiable from the build
   graph today. No binary-size winner has been measured.

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
- **Platform coverage.** Both target Linux, macOS, and Windows from one codebase.

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

## Measured IPC evidence (two different benches — do not mix them)

### In-process router only (not what an app feels)

`cargo run -q --release -p kiri-core --example bulk_bench` drives
serialize `WireRequest` → `Router.dispatch` → deserialize `WireResponse`
with **no WebView**. It is a useful core-path microbench. It is **not** a
Tauri `invoke` comparison and must not be quoted as IPC latency.

| Payload | Mean wall (ms) | Throughput (MiB/s) |
|---------|----------------|--------------------|
| 1 MiB   | 0.621          | ~1674              |
| 16 MiB  | 5.413          | ~2961              |
| 100 MiB | 35.224         | ~2872              |

### Through-webview ping/echo (what an app feels)

`kiri-host --ipc-bench` and the Tauri baseline `kiri_echo` command now run
the same payload sizes through a live page: Kiri uses `window.kiri.send` +
host `evaluate_script(onResponse)`; Tauri uses `__TAURI_INTERNALS__.invoke`.
Harness: `python3 benchmark/compare.py`. WKWebView `performance.now()` is
often 1 ms coarse, so the comparable figure is batch-mean (total batch
time / N), not the per-call median of 0/1 ms samples.

Local macOS aarch64, release, 30 iterations after 5 warmups (this host):

| Payload content | Kiri batch-mean (ms) | Tauri batch-mean (ms) | kiri/tauri |
|-----------------|---------------------:|----------------------:|-----------:|
| 0 B             |                0.133 |                 0.267 |       0.50 |
| 64 B            |                0.133 |                 0.700 |       0.19 |
| 1 KiB           |                0.500 |                 0.233 |       2.14 |
| 16 KiB          |                0.133 |                 0.300 |       0.44 |
| 256 KiB         |                1.000 |                 1.067 |       0.94 |
| ~1 MiB          |                2.767 |                 3.133 |       0.88 |

Honesty: sub-millisecond rows are inside timer noise and flip winners
(see 1 KiB). At 256 KiB and ~1 MiB the two paths are close, with Kiri
slightly ahead on this host. This is **not** a router's 3 GiB/s claim.
Windows through-webview vs Tauri is measured in the hosted T009 run. T008's
shared-buffer path and crossover evidence are documented in
[`SHARED_BUFFER_REPORT.md`](SHARED_BUFFER_REPORT.md).

Latest bounded probe after the cross-host IPC completion fix: the Kiri
one-size/one-run path completed and wrote its artifact, but the same local run
failed all startup marker watchdogs and produced no Tauri IPC artifact. It is
useful regression evidence for benchmark completion, not a Kiri-versus-Tauri
winner and does not replace the table above.

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

## T009 three-way comparison: historical macOS marker leg (SUPERSEDED)

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

This five-run debug artifact predates the asynchronous `kiri://` protocol fix
and is retained as historical evidence only. It must not be used to claim a
current Kiri startup advantage.

**Honesty notes (carried from Q-003):**
- `dom_ready` / `app_ready` / `first_animation_frame` for the Tauri baseline run
  through `__TAURI_INTERNALS__.invoke('kiri_marker')`, which is heavier than the
  wry `window.ipc.postMessage` path. Those phases are reported but flagged
  non-comparable across targets; Kiri and the wry/tao baseline share the lighter
  path and are directly comparable there.
- This is the **macOS leg only** and is superseded by the later hosted artifact
  described below. Do not generalize it to Windows/Linux.
- Linux is a documented headless soft probe (no GPU on runners), so no hard
  Linux comparison is claimed.

**Baseline fix made during this measurement:** the Tauri baseline hardcoded
`ProcessSpawnRequested` and `NativeEntry` to 0, which corrupted its `t0`
reference and collapsed every early marker to ~0ns. Corrected to record
`NativeEntry` with a real `now_ns()` sample (matching the wry/tao baseline), so
Tauri's early phases are now honestly measured (was ~84ns, now ~156ms).

## T009 current evidence: local release, async `kiri://`, 5-run markers

Local macOS aarch64, **release** binaries, 5 launches each, same blank
frontend, frozen marker schema. Harness: `benchmark/compare.py`. Raw:
`artifacts/compare-macos.json` (gitignored).

| marker (median ns)         | Kiri native | wry/tao baseline | Tauri baseline | notes |
|----------------------------|------------:|-----------------:|---------------:|-------|
| platform_initialized       |  92,681,875 |       92,564,917 |     93,997,250 | comparable |
| webview_creation_requested |  92,682,542 |       92,565,583 |     93,997,583 | comparable |
| webview_ready              | 290,252,834 |      274,833,292 |    312,477,667 | comparable |
| first_animation_frame      | 290,348,625 |      275,014,083 |    312,824,000 | Tauri via invoke |

On this host, all three are clustered. wry/tao is fastest to `webview_ready`
(~275 ms). Kiri is ~5% behind that baseline (~290 ms) and ~7% ahead of
Tauri (~312 ms). That is **not** an 18–25% product claim. Earlier debug
and pre-async-protocol artifacts are historical only.

Unstripped release binary sizes from the same run:

| binary | bytes |
|--------|------:|
| wry-tao-baseline | 1,662,592 |
| kiri-host | 2,709,024 |
| tauri-baseline | 10,041,504 |

Kiri is ~3.7× smaller than the Tauri baseline and ~1.6× larger than the
thin wry/tao baseline (Kiri carries the control plane). Footprint vs Tauri
is a real, measured edge.

## T009 current hosted evidence: macOS + Windows (commit `0d7e9a6`, run `32696370579`)

This is the first complete run after the Kiri-first artifact ordering and
bounded-failure changes. Each startup target used 20 measured launches with
three warmups; IPC used 20 batch iterations at six payload sizes. Values below
are process wall-clock medians/p95s for startup and batch means in milliseconds
for IPC. GitHub-hosted runners are directional evidence, not a universal
hardware claim.

| runner | Kiri startup p50/p95 | Tauri startup p50/p95 | Wry/Tao status |
|--------|----------------------:|----------------------:|----------------|
| macos-latest | 1,819.6 / 1,952.1 | 1,641.4 / 1,919.6 | complete: 1,730.4 / 1,893.8 |
| windows-latest | 831.2 / 850.7 | 826.5 / 858.2 | incomplete: warmup timeout at 20 s |

Kiri does not win startup in this run: Tauri is faster on both runners within
the observed distributions, with Windows effectively close at the reported
precision. This is evidence against an unconditional Kiri startup advantage.

Hosted through-webview IPC batch means:

| payload | Win Kiri | Win Tauri | Mac Kiri | Mac Tauri |
|---------:|---------:|----------:|---------:|----------:|
| 0 B | 0.260 | 1.720 | 0.600 | 1.600 |
| 64 B | 0.175 | 1.600 | 0.750 | 0.900 |
| 1 KiB | 0.225 | 1.655 | 0.400 | 0.600 |
| 16 KiB | 0.780 | 2.350 | 0.800 | 0.850 |
| 256 KiB | 4.870 | 10.385 | 3.250 | 2.400 |
| ~1 MiB | 19.480 | 38.080 | 4.300 | 5.100 |

Kiri is faster for the largest payload on both runners and for most smaller
payloads, but macOS at 256 KiB is a counterexample. This supports a scoped IPC
throughput advantage, not a blanket performance claim. All six IPC sizes
completed for both Kiri and Tauri.

Unstripped release binary sizes from this run:

| runner | Kiri | Wry/Tao | Tauri |
|--------|-----:|--------:|------:|
| macOS | 2,779,824 | 1,664,528 | 10,058,640 |
| Windows | 1,963,008 | 901,120 | 8,663,552 |

Kiri is smaller than the Tauri baseline on both runners. Wry/Tao is smaller
because it does not include Kiri's control plane and native capability layer.
Raw artifacts are retained by the Actions run above.

The subsequent bounded-queue verification run `32730288110` also completed
both hosted jobs. Its startup medians were Kiri 851 ms versus Tauri 1,856 ms
on macOS, and Kiri 845 ms versus Tauri 884 ms on Windows; Wry/Tao again
produced an incomplete Windows warmup after 20 seconds. These materially
different macOS medians demonstrate hosted-run variance, so neither run is a
universal startup claim. The two runs together establish that the benchmark
workflow is repeatable and that IPC/startup conclusions must retain run,
runner, and distribution metadata.

## T009 hosted macOS + Windows (commit `6e0c6ef`, workflow `controlled-performance`)

`windows-latest` and `macos-latest` after fixing WebView2 replies
(`PostWebMessageAsJson`). The Windows Wry/Tao baseline yielded one long
sample rather than a stable comparison set.
Harness wall-clock is process spawn → smoke exit, not `webview_ready`.
Tauri embeds its frontend; Kiri serves `examples/blank` at runtime.

Hosted **startup** medians (20 runs, process wall-clock; Actions run
`31988662774`):

| runner | Kiri | Tauri | wry/tao |
|--------|-----:|------:|--------:|
| windows-latest | 831 ms | 839 ms | one 20.7 s sample |
| macos-latest | 1502 ms | 1584 ms | 1716 ms |

Hosted **through-webview IPC** batch-means (20 runs, all 6 sizes succeeded):

| Payload | Win Kiri | Win Tauri | Mac Kiri | Mac Tauri |
|---------|---------:|----------:|---------:|----------:|
| 0 B | 0.18 | 1.59 | 0.35 | 1.55 |
| 64 B | 0.13 | 1.47 | 0.55 | 1.40 |
| 1 KiB | 0.12 | 1.93 | 0.75 | 1.50 |
| 16 KiB | 0.92 | 2.03 | 0.75 | 2.00 |
| 256 KiB | 4.75 | 11.08 | 2.40 | 10.70 |
| ~1 MiB | 22.46 | 40.52 | 4.40 | 40.40 |

On Windows, through-webview ping/echo is faster than Tauri invoke at every
payload size, and the current Kiri/Tauri process-level startup medians are
within measurement variance. Wry/Tao produced one 20.7 s Windows sample and
is not treated as a stable comparison set. Do not mix process startup and
through-webview IPC claims. Artifacts: `perf-windows-latest` /
`perf-macos-latest` on Actions run `31988662774`.

The same run recorded release binary sizes:

| runner | Kiri | Wry/Tao | Tauri |
|--------|-----:|--------:|------:|
| macOS | 2,411,552 | 1,661,248 | 10,043,664 |
| Windows | 1,453,056 | 926,208 | 8,640,000 |

The Windows Kiri IPC artifact reports 20 shared-buffer replies at both 256
KiB and approximately 1 MiB, with zero fallbacks. This is evidence for the
current shared-buffer path, not a claim that T008's separate crossover report
is complete.

From 0.1.3 the same workflow also records `startup-kiri-embed.json`:
`kiri-host --smoke` with **no** `--frontend`. Hosted medians, 20 runs,
commit `25b8898`:

| runner | Kiri disk `--frontend` | Kiri embedded | Tauri |
|--------|----------------------:|--------------:|------:|
| macos-latest | 1518 ms | **557 ms** | 546 ms |
| windows-latest | 2794 ms | 2788 ms | 830 ms |

Hosted `2761005` (in-memory Windows pack, 20 runs):

| runner | Kiri disk | Kiri packed | Tauri |
|--------|----------:|------------:|------:|
| macos-latest | 1597 ms | 1586 ms | 1443 ms |
| windows-latest | 2812 ms | 2899 ms | 938 ms |

Serving packed UI from memory on Windows did **not** close the gap.
The ~2.8 s is WebView2 environment + first navigation on the hosted VM,
not folder I/O. Next experiment: `--disable-gpu` + a persistent user-data
folder on that VM. Disk `--frontend` remains a diagnostic.

macOS runners are noisy (557 ms last time, 1586 ms this time). Treat
single-job hosted medians as directional, not a trophy.

The hosted `c0a9120` artifact predates async `kiri://` and measured Kiri
losing end-to-end process time to Tauri's embedded frontend. It remains a
valid pre-fix diagnostic, not a current claim.

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

We are at parity with Tauri on desktop platform coverage and capability
*concept*. The durable, defensible edge is the double-gated control plane
plus a smaller default binary. Through-webview IPC on this Mac is close
to Tauri's invoke at 256 KiB–1 MiB and too noisy to call at tiny
payloads. Startup on this Mac is a few percent around the wry/tao
baseline, not a blowout. Developer docs (`GETTING_STARTED.md`,
`API_REFERENCE.md`) and an interactive demo (`examples/demo`) now ship.
Remaining work that actually takes customers: mobile, bundling/signing,
and a Windows T009 vs Tauri.


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
