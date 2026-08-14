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