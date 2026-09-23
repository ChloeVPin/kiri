# Scoreboard proof harness

`benchmark/scoreboard_gate.py` is the public scoreboard gate for IPC
performance claims. An artifact that feeds `docs/COMPETITIVE_ANALYSIS.md`
(or any published chart) must pass `check` before its numbers are quoted.
The gate exists to prove claims or refuse them honestly. A losing
measurement with honest numbers is a success; a winning chart built on
in-process numbers is a failure this gate is designed to catch.

## Kill criteria

The gate prints `REFUSED` with reasons and exits non-zero when an artifact:

- is not metric class `through-webview-ipc`. `bulk_bench`,
  `ordinary-message-bulk-path`, and any other in-process router microbench
  are refused outright. They never touch a WebView and cannot be published
  as IPC an application feels.
- records a bench `error` or has no `results`.
- lacks provenance: no run id (`run.id` or `run_id`) and no local host id
  (`run.host_id` or `host_id`), no runner/OS label (`run.os`, `run.runner`,
  `os`, or `environment.platform`), or no `commit` (`"unknown"` counts as
  missing).
- lacks the measurement shape: no positive `runs` iteration count, no
  `warmup` count, no payload sizes, or a result row with neither
  `mean_from_batch_ms` (batch-mean) nor a numeric `rtt_ms` distribution.
- asserts a claim it cannot prove. A `shared-buffer` claim requires
  per-size `shared_buffer_used` / `shared_buffer_hits` counts plus
  top-level `shared_buffer.replies_ok` and `shared_buffer.replies_fallback`
  counts. A `ring-zerocopy` claim requires `transport: "ring_zerocopy"`,
  a top-level `ring` block with `replies_ok` / `replies_fallback` /
  `send_fallbacks` counts, and per-size `ring_slot_hits`. A `zero-copy`
  claim is proved by the ring contract when the artifact ran the ring
  transport or recorded ring traffic, and by the shared-buffer contract
  otherwise. A claim with zero observed replies on its owning path, or
  with missing proof fields, is refused. Fallbacks are legal but must be
  reported.
- is labeled `fixture` or `example`. Example data validates shape only under
  `--allow-fixtures` and can never reach a published table.
- asserts a claim with no registered proof contract. Unknown claims are
  refused by default.

## Artifact contract

Produced today by `kiri-host --ipc-bench` and by the Tauri baseline under
`KIRI_IPC_BENCH=1`. Required fields:

| field | meaning |
|-------|---------|
| `schema_version` | `1` |
| `name` / `metric_class` | `through-webview-ipc` |
| `target` | producing host, for example `kiri-host` or `tauri-baseline` |
| `commit` | measured commit; `"unknown"` is refused |
| `run.id` or `run.host_id` | hosted run id, or local machine identity |
| `run.os` / `run.runner` | runner/OS label for the numbers |
| `runs` | measured iterations per size (positive) |
| `warmup` | discarded warmup iterations per size |
| `sizes_bytes` | payload sizes measured |
| `results[]` | per size: `size_bytes`, `rtt_ms` distribution and/or `mean_from_batch_ms`, plus `shared_buffer_hits` / `shared_buffer_used` proof counts; ring runs also record `ring_slot_hits` / `ring_send_fallbacks` |
| `transport` | `default` (JSON plus T008 shared buffers) or `ring_zerocopy` (slot-arena ring); required for ring claims |
| `shared_buffer` | `threshold_bytes`, `replies_ok`, `replies_fallback` counts |
| `ring` | `replies_ok`, `replies_fallback`, `send_fallbacks` counts; required for ring claims |
| `claims` | optional list of asserted claims; `ring-zerocopy` is the canonical ring claim (`ring`, `ring_zerocopy` are aliases); evidence of shared-buffer or ring traffic also counts as an implicit claim |

Both producers stamp `commit`, `runs`, `warmup`, `sizes_bytes`, and a `run`
provenance block automatically. The `run` block is filled from the measuring
machine's environment (`GITHUB_RUN_ID`, `GITHUB_SERVER_URL`,
`GITHUB_REPOSITORY`, `RUNNER_NAME`, `RUNNER_OS`, `RUNNER_ARCH`,
`KIRI_BENCH_HOST_ID`, `HOSTNAME`/`COMPUTERNAME`); hosted CI runs therefore
satisfy the contract with no extra steps.

## Running the gate

```bash
# validate one or more artifacts; exit 1 if any is refused
python3 benchmark/scoreboard_gate.py check artifacts/ipc-kiri.json artifacts/ipc-tauri.json

# machine-readable verdict (status: accepted | refused)
python3 benchmark/scoreboard_gate.py check artifacts/ipc-kiri.json \
  --verdict-out artifacts/scoreboard-verdict.json

# shape-check example fixtures (tests only, never for publishing)
python3 benchmark/scoreboard_gate.py check --allow-fixtures benchmark/fixtures/*.example.json
```

For artifacts produced outside the instrumented producers (for example an
older local file missing provenance), `stamp` injects publish metadata
without touching measured numbers:

```bash
python3 benchmark/scoreboard_gate.py stamp artifacts/ipc-kiri.json \
  --out artifacts/ipc-kiri.stamped.json \
  --run-id "$GITHUB_RUN_ID" --runner "$RUNNER_NAME" --os "$RUNNER_OS"
# claims are asserted at publish time and are then proof-checked:
python3 benchmark/scoreboard_gate.py stamp artifacts/ipc-kiri.json \
  --claim shared-buffer --out artifacts/ipc-kiri.stamped.json
```

## CI wiring

The `controlled-performance` workflow produces `artifacts/ipc-kiri.json` and
`artifacts/ipc-tauri.json` on hosted runners, where the provenance
environment variables are already set, so produced artifacts satisfy the
contract. The recommended gate step runs after the IPC artifact upload and
fails the job when a produced artifact is unpublishable; a missing artifact
is an incomplete run, already surfaced by the bench steps:

```yaml
      - name: Scoreboard proof gate (through-webview IPC artifacts)
        if: always()
        shell: bash
        run: |
          failed=0
          for f in artifacts/ipc-kiri.json artifacts/ipc-tauri.json; do
            if [ -f "$f" ]; then
              python3 benchmark/scoreboard_gate.py check "$f" || failed=1
            else
              echo "::warning::$f missing; no IPC artifact to validate"
            fi
          done
          exit $failed
```

The gate tests run via `python -m unittest benchmark/test_scoreboard_gate.py`;
wire it into the `correctness` workflow next to the existing
`benchmark/test_harness.py` step.

## Ring transport contract (`transport: "ring_zerocopy"`)

The zero-copy ring spike (`docs/ZEROCOPY_IPC_MOONSHOT.md`, one host-owned
slot arena posted read-write, raw payload bytes in slots, tiny JSON control
messages on `postMessage`) joins the same gate rather than bypassing it. A
ring artifact is a `through-webview-ipc` artifact that additionally carries:

| field | meaning |
|-------|---------|
| `transport` | `ring_zerocopy`; the artifact must declare the wire it ran. A ring claim on a `default` or missing transport is refused |
| `ring.replies_ok` | host-side replies served through ring slots (non-negative int) |
| `ring.replies_fallback` | host-side replies that fell back to the JSON/T008 wire (non-negative int) |
| `ring.send_fallbacks` | page-side sends that could not use a slot and fell back (non-negative int) |
| `results[].ring_slot_hits` | per-size count of replies actually read back through slots (non-negative int, required on every result) |
| `results[].ring_send_fallbacks` | per-size count of sends that fell back to the ordinary wire |

`ring-zerocopy` is the canonical claim name; `ring`, `ring_zerocopy`, and
`ringzerocopy` normalize to it. An artifact that declares
`transport: "ring_zerocopy"` or records ring traffic asserts the claim
implicitly, exactly like shared-buffer traffic does, so its proof is
checked whether or not a `claims` array is present. A run with zero ring
replies and zero slot hits proves nothing and is refused, even if the
fallback counters moved.

`zero-copy` is a dual-path claim. The ring contract proves it when
`transport` is `ring_zerocopy` or the artifact recorded ring traffic; the
WebView2 shared-buffer contract (T008) proves it otherwise. A `zero-copy`
claim with zero shared-buffer evidence and zero ring evidence is always
refused. An artifact that declares the ring transport must prove the ring
path for a `zero-copy` claim; a run that fell back entirely should be
republished with `transport: "default"` and a shared-buffer claim.

When a further transport lands, it plugs in the same way:

1. Emit `name` (or `metric_class`) for the class, the same provenance
   block (`run`, `commit`, `runs`, `warmup`, `sizes_bytes`), and per-size
   results.
2. Emit per-size proof counts showing the mechanism was actually exercised
   plus top-level fallback counts.
3. Register the metric class in `ACCEPTED_METRIC_CLASSES` and a proof
   checker in `PROOF_CHECKERS` in `benchmark/scoreboard_gate.py`. Until a
   claim has a registered proof contract, artifacts asserting it are
   refused. This is the intended behavior: unprovable claims do not ship.

`crates/kiri-runtime/examples/ring_cost.rs` is an in-process serialization
cost model for the ring spike. It never touches a WebView, it is not a
`through-webview-ipc` artifact, and its output must never feed the
scoreboard or this gate.

## What this gate does not cover

Startup marker and binary-size artifacts follow the `benchmark/harness.py`
schema and the frozen marker schema (`docs/research/markers-schema.md`).
This gate covers IPC artifacts only. `bulk_bench` remains a useful
in-process microbench; it is simply never publishable as IPC evidence.

Related: `docs/COMPETITIVE_ANALYSIS.md` (scoreboard), `docs/SHARED_BUFFER_REPORT.md`
(T008 shared-buffer evidence), `benchmark/README.md` (harness rules).
