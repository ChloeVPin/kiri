# Research Notes

This directory links the evidence collected during implementation. The
authoritative corpus lives at `../../kiri-agent-execution-corpus` (gitignored;
do not commit its contents).

## Markers schema

`markers-schema.md` — the startup marker contract shared by the direct host
and both baselines (derived from corpus `docs/12-benchmarks.md`).

## Sources

`SOURCES.md` — the corpus source catalog is the starting point; re-open
primary sources whenever an API version matters.

- webview2-com 0.39.1 / webview2-com-sys 0.39.1 / webview2-com-macros 0.8.1
  (crates.io, docs.rs build 2026-06-28)
- windows 0.62.2 / windows-core 0.62.2 (crates.io)
- wry 0.56.1, tao 0.36.0, tauri 2.11.5 (crates.io, docs.rs)

## Evidence links

Each task record in the corpus `agent/task_queue.json` carries its own
evidence links; `docs/DECISIONS.md` summarizes the decision-level evidence.