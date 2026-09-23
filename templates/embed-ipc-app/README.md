# embed-ipc-app

Single-binary Kiri app. `cargo build --release` produces one host binary with
the UI in `frontend/` compiled in. The UI is a through-webview IPC demo: it
issues repeated `window.kiri.send` ping round-trips and reports RTT stats, so
a customer can see the IPC path working out of the shipped artifact.

## Build and run

```sh
cargo build --release
./target/release/kiri-ipc-app        # macOS, Linux
.\target\release\kiri-ipc-app.exe    # Windows
```

No `--frontend` flag and no `frontend/` folder next to the binary are needed
at runtime; `.cargo/config.toml` sets `KIRI_EMBED_FRONTEND` for the
kiri-runtime build script. During development, `--frontend DIR` or
`KIRI_FRONTEND` still override the packed UI so edits reload without a
rebuild.

Platform prerequisites are the same as `kiri-host`: GTK 3 + WebKit2GTK 4.1 on
Linux, WebView2 Evergreen on Windows. The binary is unsigned at the OS level;
macOS Gatekeeper and Windows SmartScreen may warn.

## Verify the embed

```sh
./target/release/kiri-ipc-app --smoke --markers-out /tmp/markers.json
```

Smoke mode exits 0 after `first_animation_frame` with the same markers as
`kiri-host`. On Windows the packed frontend is materialized to a temp dir at
startup because WebView2 virtual-host mapping requires a real directory; the
shipped artifact is still the single binary.

## Ship from CI

Copy `../embed-ipc-app.yml` into `.github/workflows/` in the app repo. It
builds the same `cargo build --release` on macOS, Windows, and Linux and
uploads just the one binary per OS.

## Zero-copy IPC

The demo talks the current `window.kiri.send` + `onResponse` control-plane
transport and counts WebView2 `sharedbufferreceived` replies. When the
zero-copy ring / shared-buffer transport merges into kiri main, this template
picks it up through the git dependency; the frontend keeps calling
`window.kiri.send` and only the reply path changes underneath.
