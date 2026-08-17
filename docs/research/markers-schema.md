# Startup Markers Schema (schema_version: 1)

Shared contract between the direct host (`kiri-host`), the Wry/Tao baseline
and the Tauri baseline, derived from corpus `docs/12-benchmarks.md`.

## Marker names (in expected order)

| # | name                    | recorded when                                            |
|---|-------------------------|----------------------------------------------------------|
| 1 | `process_spawn_requested` | process spawn is requested (harness side; t=0 reference) |
| 2 | `native_entry`          | process main entry point reached                         |
| 3 | `platform_initialized`  | windowing/platform init done (window created)            |
| 4 | `webview_creation_requested` | webview creation call issued                         |
| 5 | `webview_ready`         | first page load finished (NavigationCompleted / page load) |
| 6 | `bridge_ready`          | IPC bridge script installed & ready to receive          |
| 7 | `dom_ready`             | frontend DOMContentLoaded fired                          |
| 8 | `app_ready`             | application logic considers itself ready                |
| 9 | `first_animation_frame` | first requestAnimationFrame callback fired              |
| 10 | `first_invoke_dispatched` | first `window.kiri.send()` entered native dispatch (optional; omitted when the page never invokes) |
| 11 | `first_invoke_responded` | first control-plane response produced (optional) |

## JSON document (stdout, one line)

```json
{
  "schema_version": 1,
  "markers": [
    { "name": "process_spawn_requested", "timestamp_ns": 0, "since_first_ns": 0 },
    { "name": "native_entry", "timestamp_ns": 123, "since_first_ns": 123 }
  ]
}
```

- `timestamp_ns`: monotonic clock reading for this process
  (`QueryPerformanceCounter` on the direct host; `Instant` on baselines).
- `since_first_ns`: `timestamp_ns - first recorded timestamp_ns`; always ≥ 0.
- Missing markers are omitted; consumers must tolerate gaps.

## Smoke contract

- exit code `0` after `first_animation_frame` + a short grace period
  (`--exit-after-ready-ms`, default 250 ms),
- exit code `2` when the ready watchdog fires (`--watchdog-ms`, default
  30 000 ms),
- markers JSON printed to stdout; `kiri-host --markers-out <file>` also
  writes it to a file.