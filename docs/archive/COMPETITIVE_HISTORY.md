# Competitive History (superseded)

> **Archived:** historical scoreboards superseded by the single current table in `docs/COMPETITIVE_ANALYSIS.md` (hosted run `32730288110`, `macos-latest` + `windows-latest`, 20 runs, 3 warmups/45 s). Do not cite this file for current claims; retained for audit trail only.

This file preserves the historical T009 tables that were collapsed from `COMPETITIVE_ANALYSIS.md:139-326` into `<details><summary>Historical (superseded)</summary>`. The authoritative startup medians, IPC batch-means, and binary sizes are now in `COMPETITIVE_ANALYSIS.md` current hosted scoreboard section.

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

Hosted through-webview IPC batch means:

| payload | Win Kiri | Win Tauri | Mac Kiri | Mac Tauri |
|---------:|---------:|----------:|---------:|----------:|
| 0 B | 0.260 | 1.720 | 0.600 | 1.600 |
| 64 B | 0.175 | 1.600 | 0.750 | 0.900 |
| 1 KiB | 0.225 | 1.655 | 0.400 | 0.600 |
| 16 KiB | 0.780 | 2.350 | 0.800 | 0.850 |
| 256 KiB | 4.870 | 10.385 | 3.250 | 2.400 |
| ~1 MiB | 19.480 | 38.080 | 4.300 | 5.100 |

Unstripped release binary sizes from this run:

| runner | Kiri | Wry/Tao | Tauri |
|--------|-----:|--------:|------:|
| macOS | 2,779,824 | 1,664,528 | 10,058,640 |
| Windows | 1,963,008 | 901,120 | 8,663,552 |

The subsequent bounded-queue verification run `32730288110` also completed
both hosted jobs. Its startup medians were Kiri 851 ms versus Tauri 1,856 ms
on macOS, and Kiri 845 ms versus Tauri 884 ms on Windows; Wry/Tao again
produced an incomplete Windows warmup after 20 seconds. These materially
different macOS medians demonstrate hosted-run variance, so neither run is a
universal startup claim.

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

The same run recorded release binary sizes:

| runner | Kiri | Wry/Tao | Tauri |
|--------|-----:|--------:|------:|
| macOS | 2,411,552 | 1,661,248 | 10,043,664 |
| Windows | 1,453,056 | 926,208 | 8,640,000 |

The Windows Kiri IPC artifact reports 20 shared-buffer replies at both 256
KiB and approximately 1 MiB, with zero fallbacks.

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

The hosted `c0a9120` artifact predates async `kiri://` and measured Kiri
losing end-to-end process time to Tauri's embedded frontend. It remains a
valid pre-fix diagnostic, not a current claim.
