# Benchmark Harness

`harness.py` (copied from the execution corpus) runs an external command
repeatedly and writes a JSON result containing samples and environment
metadata. It is deliberately generic so each baseline can expose the same
scenario command.

Example:

```bash
python benchmark/harness.py   --name startup-kiri   --runs 30   --output artifacts/startup-kiri.json   -- ./target/release/kiri-host --smoke --frontend examples/blank
python benchmark/harness.py   --name startup-wry   --runs 30   --output artifacts/startup-wry.json   -- ./baselines/wry-tao/target/release/wry-tao-baseline
python benchmark/harness.py   --name startup-tauri --runs 30   --output artifacts/startup-tauri.json -- ./baselines/tauri/target/release/tauri-baseline
```

## Scenario commands

All three targets expose the same smoke contract:

- exit code `0` after a clean startup + exit, printing one JSON document
  (schema version 1) with `markers[]` on stdout,
- exit code `2` when the ready watchdog fires,
- a unique `--markers-out <file>` mode on `kiri-host` that also writes the
  JSON to a file (needed for the startup-result acceptance check).

## Rules

- compile all compared targets before timed runs unless measuring build time
- keep power mode and background-load policy fixed
- record OS, CPU, memory, WebView version, compiler, project commit, and
  frontend commit where possible
- retain raw samples
- do not gate wall-clock regressions on a shared hosted runner
- treat any non-zero or timed-out benchmark sample as a failed benchmark after
  writing its diagnostic artifact; invalid comparison data must not look green

## Asset-delivery comparability

The Tauri baseline uses `frontendDist`, which `tauri-build` embeds into the
application binary for production builds. Kiri's `--frontend` mode serves the
same checked frontend directory at runtime. These are intentionally different
product paths and must be reported separately: an embedded-asset result is not
evidence that Kiri's runtime filesystem path is faster.

The cross-platform Kiri host now registers `kiri://` through Wry's asynchronous
custom-protocol API. Frontend reads and response construction run away from the
WebView event thread while the existing path, origin, MIME, range, and cache
checks remain in force.

The hosted comparison at commit `c0a9120` was collected before that change and
showed Kiri's synchronous protocol path losing end-to-end startup time to the
Tauri embedded baseline. It is a regression artifact, not a current Kiri
performance claim. A six-run local macOS release check after the change measured
Kiri at a 614 ms median versus 772 ms for the Wry/Tao baseline; the local Tauri
baseline did not complete within its 20-second bound, so no local Kiri/Tauri
winner is claimed.

`test-vectors.json` holds the fixed seed/expectation data used by the
harness's self-check mode.

## Through-webview IPC (Kiri vs Tauri)

`bulk_bench` is in-process and is **not** a Tauri comparison. The comparable
IPC bench goes through a live WebView:

```bash
# official three-way: startup markers + through-webview IPC + binary size
python3 benchmark/compare.py --profile release --startup-runs 5 --ipc-runs 30

# Kiri only
./target/release/kiri-host --frontend examples/blank --ipc-bench \
  --ipc-bench-runs 30 --ipc-bench-out artifacts/ipc-kiri.json

# Tauri only (same payload sizes)
KIRI_IPC_BENCH=1 KIRI_IPC_BENCH_RUNS=30 \
  KIRI_IPC_BENCH_OUT=artifacts/ipc-tauri.json \
  ./baselines/tauri/target/release/tauri-baseline
```

Kiri measures `window.kiri.send` → host router → `evaluate_script(onResponse)`.
Tauri measures `__TAURI_INTERNALS__.invoke('kiri_echo')`. Payload content
sizes match `control_payload_bytes` except the last size is 1_048_574 so the
JSON string stays under the 1 MiB control ceiling. Report **batch-mean**
(total batch time / N); per-call `performance.now()` on WKWebView is often
0 or 1 ms.
## T007 ordinary-message bulk-path benchmark

`crates/kiri-core/examples/bulk_bench.rs` measures the kiri-core JSON control
path (`WireRequest` serialize -> `Router::dispatch` -> `WireResponse`
deserialize) at the bulk payload sizes from `test-vectors.json`
(`bulk_payload_bytes`: 1 MB / 16 MB / 100 MB). It records, per iteration:

- wall clock (ms)
- CPU time (process user + sys via `getrusage`, ms)
- peak RSS (bytes, `ru_maxrss`)
- throughput (MiB/s)

It is fully Mac/Linux-runnable: it exercises kiri-core only, with no WebView,
so it can run on the macOS development machine. The emitted artifact retains
the full raw sample arrays plus a per-size summary and environment metadata.

Run it (release build; the corpus fairness rule requires a fixed
release/debug posture):

```bash
benchmark/run_bulk.sh                 # runs=20, release, artifacts/bulk-ordinary.json
```

Or directly:

```bash
cargo build --release --example bulk_bench -p kiri-core
KIRI_BULK_RUNS=20 KIRI_BULK_OUT=artifacts/bulk-ordinary.json \
  ./target/release/examples/bulk_bench
```

This is the "ordinary message" path. The WebView2 read-only shared-buffer
fast path (T008) and the macOS/Linux custom-scheme streaming experiment are
separate, later work; the bulk ordinary-message numbers here are the baseline
the shared-buffer path must beat to justify its added complexity.
