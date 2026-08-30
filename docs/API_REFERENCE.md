# Kiri API reference

The Kiri frontend API (`kiri.js`) exposes 74 control-plane commands (ids
1–74) organized by surface. Every command is double-gated: a capability bit
AND a host allowlist. JavaScript never supplies the capability mask.

The API is served over `kiri://localhost` and works identically on Linux
(wry/tao), macOS (wry/tao), and Windows (direct Win32 + WebView2).

## How to call

Load `kiri.js`, then use `window.kiri`:

```js
api.app.version().then(function (v) { console.log(v); });
```

Each method returns a `Promise`. Errors reject with an `Error` whose
`.message` carries the host's error category (e.g. `"Unauthorized"`,
`"protocol_error"`).

## Platform and app (ids 5–7)

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.platform.os()` | 5 | platform | `{ os: string }` |
| `api.platform.arch()` | 6 | platform | `{ arch: string }` |
| `api.app.version()` | 7 | app | `{ version: string }` |

## Events (ids 8–9, 56–58)

| Method | Id | Capability | Notes |
|--------|----|------------|-------|
| `api.event.emit(event, payload)` | 8 | event | legacy emit |
| `api.event.listen(event, handler)` | 9 | event | legacy listen |
| `api.event.publish(channel, payload)` | 56 | event | channel-allowlisted |
| `api.event.subscribe(channel)` | 57 | event | channel-allowlisted |
| `api.event.channels()` | 58 | event | list approved channels |

## Filesystem (ids 10–13, 67–68)

Scoped to a host-owned `PathScope` sandbox. Paths outside the scope are
rejected.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.fs.read(path)` | 10 | fs | base64 string |
| `api.fs.write(path, base64, createNew)` | 11 | fs | bytes written |
| `api.fs.exists(path)` | 12 | fs | boolean |
| `api.fs.remove(path)` | 13 | fs | boolean |
| `api.fsWatch.watch(path, kind)` | 67 | fs | `{ watchId, path }` |
| `api.fsWatch.unwatch(watchId)` | 68 | fs | `{ unwatched }` |

## Window (ids 14–22, 49–50)

JS never reaches the native window handle. All operations route through a
host-owned controller.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.window.title()` | 14 | window | current title |
| `api.window.setTitle(title)` | 15 | window | new title |
| `api.window.show()` | 16 | window | — |
| `api.window.hide()` | 17 | window | — |
| `api.window.minimize()` | 18 | window | — |
| `api.window.maximize()` | 19 | window | — |
| `api.window.restore()` | 20 | window | — |
| `api.window.close()` | 21 | window | — |
| `api.window.focus()` | 22 | window | — |
| `api.window.state.save(geometry)` | 49 | window_state | persisted geometry |
| `api.window.state.load()` | 50 | window_state | saved geometry |

## Clipboard (ids 23–24)

Host-owned `ClipboardController`; JS never touches the OS clipboard
directly.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.clipboard.read()` | 23 | clipboard | text |
| `api.clipboard.write(text)` | 24 | clipboard | written count |

## Path and OS (ids 25–37)

Pure path math plus read-only OS directory discovery. No env vars or
filesystem roots exposed.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.path.dirname(path)` | 25 | path | dirname |
| `api.path.basename(path)` | 26 | path | basename |
| `api.path.extname(path)` | 27 | path | extension |
| `api.path.stem(path)` | 28 | path | stem |
| `api.path.join(base, segments)` | 29 | path | joined path |
| `api.path.isAbsolute(path)` | 30 | path | boolean |
| `api.os.homedir()` | 31 | path | home dir |
| `api.os.tempdir()` | 32 | path | temp dir |
| `api.os.appConfigDir()` | 33 | path | app config dir |
| `api.os.appDataDir()` | 34 | path | app data dir |
| `api.os.appCacheDir()` | 35 | path | app cache dir |
| `api.os.documentDir()` | 36 | path | document dir |
| `api.os.appDir()` | 37 | path | app dir |

## HTTP (ids 38, 62–65)

Double-gated: the HTTP capability AND a host allowlist. An unapproved host
is denied. Responses are bulk-capped like `kiri.fs`.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.http.get(url, maxBytes)` | 38 | http | `{ status, headers, base64, bytes }` |
| `api.http.post(url, body, maxBytes)` | 62 | http | same |
| `api.http.put(url, body, maxBytes)` | 63 | http | same |
| `api.http.patch(url, body, maxBytes)` | 64 | http | same |
| `api.http.del(url, maxBytes)` | 65 | http | same |

## Shell (id 39)

Host-allowlisted command execution. The host refuses any program/arg-prefix
not on its explicit allowlist.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.shell.run(program, args)` | 39 | shell | `{ program, exitCode, stdout, stderr, bytes }` |

## Notification (id 40)

Host-template-allowlisted. The frontend may only trigger a pre-approved
template id with bounded args; it cannot render free-form title/body.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.notification.show(template, args)` | 40 | notification | `{ templateId, title, body }` |

## Dialog (id 41)

Host-allowlisted dialog kinds with a host-owned title.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.dialog.open(kind, args, ext)` | 41 | dialog | `{ kind, title, confirmed, paths }` |

## Shortcut (id 42)

Host-allowlisted exact accelerators mapped to host-owned actions.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.shortcut.register(accelerator)` | 42 | shortcut | `{ accelerator, action }` |

## Autostart (ids 43–44)

Host-policy-gated (default-deny). Only the host's own binary may be
toggled.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.autostart.set(enabled)` | 43 | autostart | `{ enabled, managed }` |
| `api.autostart.get()` | 44 | autostart | `{ enabled, managed }` |

## Store (ids 45–46)

Host-namespace-allowlisted. One module cannot reach another's persisted
state.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.store.get(namespace, key)` | 45 | store | value |
| `api.store.set(namespace, key, value)` | 46 | store | value |

## Deeplink (id 47)

Host-scheme-allowlisted. Only a host-approved exact scheme may register.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.deeplink.register(scheme)` | 47 | deeplink | `{ scheme }` |

## Opener (id 48)

Host-allowlisted URL schemes and file extensions.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.opener.open(target)` | 48 | opener | `{ target }` |

## Tray (ids 51–52)

Host-allowlisted item ids. The host owns every item's label and action.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.tray.setMenu(ids)` | 51 | tray | `{ items }` |
| `api.tray.invoke(id)` | 52 | tray | `{ id, action }` |

## Sidecar (ids 53–55)

Host-allowlisted sidecar processes. The frontend cannot name an arbitrary
binary.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.sidecar.spawn(name, args)` | 53 | sidecar | `{ handle, name, exitCode, stdout, stderr }` |
| `api.sidecar.stop(handle)` | 54 | sidecar | `{ stopped }` |
| `api.sidecar.list()` | 55 | sidecar | `{ names }` |

## Config (ids 59–60)

Key-allowlisted. The frontend may only read pre-approved config key paths.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.config.get(key)` | 59 | config | `{ key, value }` |
| `api.config.keys()` | 60 | config | `{ keys }` |

## Updater (id 61)

Host-pinned Ed25519 key. The frontend cannot substitute a key to accept
an attacker-signed release.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.updater.check(manifest)` | 61 | updater | `{ available, version, platform, notes }` |

## CLI (id 66)

Structured, allowlist-scoped argv. Exceeds Tauri's raw `process.argv`.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.cli.args(full)` | 66 | cli | `{ raw, positionals, flags, options }` |

## WebSocket (ids 69–71)

Host-allowlisted URL. A granted capability cannot reach an unapproved origin.
The current seed host policy permits local `ws://127.0.0.1:8765` and
`ws://localhost:8765` connections. `wss://` is supported with native
certificate roots, but remains unavailable unless an exact secure URL is
added to the signed host policy. Network I/O runs off the WebView dispatch
path and inbound messages are drained through a bounded host queue.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.ws.connect(url)` | 69 | ws | `{ connId, url }` |
| `api.ws.send(connId, message)` | 70 | ws | `{ sent, connId }` |
| `api.ws.close(connId)` | 71 | ws | `{ closed, connId }` |

## Menu (ids 72–73)

Host-owned item allowlist (tray shape). The frontend may only pick
host-owned item ids.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.menu.set(ids)` | 72 | menu | `{ items }` |
| `api.menu.invoke(id)` | 73 | menu | `{ id, action }` |

## Plugin inventory (id 74)

Host-owned external-plugin inventory. The only discovery surface; the
frontend cannot enumerate or reach an unvetted plugin command.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| `api.plugin.list()` | 74 | plugin | `{ plugins }` |

## Diagnostics (id 2)

Privacy-safe runtime snapshot: backend, runtime version, open-resource
count, recent-request latency waterfall. No payload content is stored.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| (internal) | 2 | diag | `{ schema_version, runtime_version, backend, open_resources, recent_requests }` |

## Resources (ids 3–4)

Caller-owned generational resource handles. Stale or wrong-owner handles
are rejected.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| (internal) | 3 | resources | resource id |
| (internal) | 4 | resources | closed |

## Ping (id 1)

Liveness and latency probing.

| Method | Id | Capability | Returns |
|--------|----|------------|---------|
| (internal) | 1 | ping | echo of payload |
