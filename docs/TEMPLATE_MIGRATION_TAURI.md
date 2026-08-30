# Tauri → Kiri migration

Minimal mapping for the common Tauri surface. Kiri reuses the same OS webviews; the difference is the control plane is double-gated (capability bit + host allowlist).

| Tauri | Kiri | Notes |
|---|---|---|
| `invoke('my_cmd', args)` | `window.kiri.send(WireRequest)` via `kiri.js` `api.*` | Typed ids 1–74, check `docs/API_REFERENCE.md` |
| `tauri.conf.json` `frontendDist` | `KIRI_EMBED_FRONTEND` at build, `kiri://localhost` at runtime | `cargo build` packs `index.html` |
| `tauri::command` + `#[tauri::command]` | `kiri_core::dispatch::Router::with_*` + `CapabilityBits` | Host owns capability mask; JS cannot self-grant |
| `fs` plugin (unscoped) | `kiri.fs.*` + `PathScope` + `GlobScope` | Host allowlist + sandbox |
| `http` plugin | `kiri.http.*` + `HostAllowlist` | Exact host allowlist, bulk-capped |
| `shell` plugin | `kiri.shell.run` + `ShellAllowlist` | Exact program + arg prefix |
| `globalShortcut` | `kiri.shortcut.register` + `ShortcutAllowlist` | Exact accelerator allowlist |
| `notification` | `kiri.notification.show` + template allowlist | No free-form title/body from JS |
| `store` | `kiri.store.*` + namespace allowlist | One namespace per module |
| `deepLink` | `kiri.deeplink.register` + scheme allowlist | Exact scheme |
| `opener` | `kiri.opener.open` + scheme/extension allowlist | Host-allowlisted |
| `tray` | `kiri.tray.*` + item-id allowlist | Host owns label/action |
| `create-tauri-app` | `./tools/create-kiri-app.sh [--template starter|starter-vite|blank] DIR` | POSIX + PowerShell, verifies Ed25519 `RELEASES.json` |

## Vite template

`examples/starter-vite` is a Vite project that builds to `dist/`. Point the host at `dist`:

```sh
npm --prefix examples/starter-vite install
npm --prefix examples/starter-vite run build
KIRI_EMBED_FRONTEND="$PWD/examples/starter-vite/dist" cargo build --release -p kiri-runtime --bin kiri-host
```

Or scaffold directly:

```sh
./tools/create-kiri-app.sh --template starter-vite ~/Desktop/my-kiri-vite
./tools/create-kiri-app.ps1 --template starter-vite $HOME\Desktop\my-kiri-vite
```

Tauri's `tauri-plugin-*` breadth is not replicated; Kiri exceeds on the security axis with smaller default surface. See `docs/GAP_MATRIX.md`.
