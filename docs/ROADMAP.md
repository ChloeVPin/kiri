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
- Publish replacement releases as the launcher, native capability, and
  cross-platform workflow changes are validated; v0.1.4 is the current public
  launcher-bearing release and later runtime changes remain unreleased until
  their platform gates pass.
- Improve signed packaging and platform-native distribution when signing
  credentials and release policy are available.
- Continue evaluating ecosystem breadth only where it preserves Kiri's
  host-owned authorization model.
- Complete native application-menu rendering and activation routing without
  weakening the event-loop/thread-affinity boundary.
- Add pinned TLS WebSocket transport after certificate and trust policy review.

## Tracking documents

- [`OPEN_QUESTIONS.md`](OPEN_QUESTIONS.md) records unresolved technical questions
  and the evidence required to close them.
- [`GAP_MATRIX.md`](GAP_MATRIX.md) records functional gaps against Tauri.
- The task queue in `kiri-agent-execution-corpus/agent/task_queue.json` is the
  execution-level source of truth and is not part of the public README.
