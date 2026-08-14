# Diagnostics & observability (T010)

The host exposes a privacy-safe runtime diagnostics snapshot to a developer
panel. The snapshot is produced by `kiri-core::diagnostics::Diagnostics`, a
`TraceSink` that the control-plane `Router` feeds on every dispatched request.

## Contract

- A new built-in command `kiri.diag` (id `2`) returns a `DiagnosticsSnapshot`.
  It requires the `DIAGNOSTICS` capability (bit `1`), enforced by the same
  validation pipeline as every other command. The trusted frontend is granted
  `DIAGNOSTICS` in `trusted_frontend_capabilities`.
- The snapshot is schema `schema_version: 1` and contains:
  - `backend`: `"cross"` (wry/tao) or `"windows"` (Win32/WebView2)
  - `runtime_version`: `CARGO_PKG_VERSION`
  - `open_resources`: live count from the host's `ResourceTable`
  - `recent_requests`: bounded ring (max `MAX_RECENT = 64`) of
    `RequestTrace`, each with per-stage latency waterfall, command id/name,
    caller id, result category, total ms, and payload/response byte sizes.

## Privacy boundary

Per the trace spec, payload *content* is never stored. The sink retains only
sizes and result categories, and only the command id/name and caller id that
the pipeline already attaches to mid-pipeline trace events. No request body,
response body, or argument value is retained.

## Wiring

- `host_cross.rs` (wry/tao) and `host_windows.rs` (WebView2) both construct
  `Diagnostics`, register it via `Router::with_diagnostics`, and pass it as the
  `TraceSink` into `Router::dispatch`. After each dispatch they re-read the
  live `ResourceTable` length into `set_open_resources` so the panel stays
  honest about resource churn.
- The panel lives at `examples/panel/index.html`. It is served by the host
  when launched with `--frontend examples/panel`; the bridge script (injected
  at document start) provides `window.kiri.send`, and the panel issues
  `kiri.diag` (id `2`) plus periodic `kiri.ping` (id `1`) to keep the
  recent-requests ring populated end to end.

## Run

```sh
cargo build -p kiri-runtime --bin kiri-host
./target/debug/kiri-host --frontend examples/panel
```

## Known limitation

The host currently registers a single session resource for the native caller
and does not open further `kiri-core` resources from the panel, so
`open_resources` reports a static baseline (`1`) until a command that creates
resources (e.g. T008 file/stream handles) is wired into the panel path. The
metric plumbing is correct; only the observed count is currently flat.
