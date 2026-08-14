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

`test-vectors.json` holds the fixed seed/expectation data used by the
harness's self-check mode.
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
