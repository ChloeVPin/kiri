# Roadmap

This document contains planned work and product gaps. It is deliberately kept
out of the README so the README describes the stable product rather than the
current task queue.

## Product direction

The product goal and acceptance criteria are defined in
[`PRODUCT.md`](PRODUCT.md). The priorities are:

1. Preserve the shared capability and host-allowlist security boundary.
2. Keep Linux, macOS, and Windows as equal desktop targets.
3. Publish reproducible startup, IPC, and footprint measurements.
4. Improve packaging, updates, and application scaffolding without weakening
   the runtime contract.

## Current planned work

- Complete the remaining hosted performance comparison and retain platform and
  environment details with every result.
- Publish replacement releases as launcher, native-capability, and
  cross-platform workflow changes pass their platform gates. v0.1.6 is the
  current published release.
- Improve signed packaging and platform-native distribution when signing
  credentials and release policy are available.
- Continue evaluating ecosystem breadth only where it preserves Kiri's
  host-owned authorization model.
- Complete native application-menu rendering and activation routing without
  weakening the event-loop/thread-affinity boundary.
- Add approved signed `wss://` host-policy entries after certificate and trust
  policy review; the transport already uses native certificate roots.

## Tracking documents

- [`OPEN_QUESTIONS.md`](OPEN_QUESTIONS.md) records unresolved technical questions
  and the evidence required to close them.
- [`GAP_MATRIX.md`](GAP_MATRIX.md) records functional gaps against Tauri.
- [`CROSS_PLATFORM_STATUS.md`](CROSS_PLATFORM_STATUS.md) is the authoritative per-OS verification record.
- [`COMPETITIVE_ANALYSIS.md`](COMPETITIVE_ANALYSIS.md) is the single current scoreboard; historical tables are collapsed there.
- Archived: `archive/COMPETITIVE_HISTORY.md`, `archive/DEEP_AUDIT_TAURI.md` and `archive/EXCEED_TAURI_PLAN.md` are superseded (retained for history).
- The task queue in `kiri-agent-execution-corpus/agent/task_queue.json` is the
  execution-level source of truth and is not part of the public README.
