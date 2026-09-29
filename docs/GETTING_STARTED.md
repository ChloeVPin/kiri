# Getting started

Kiri is a native desktop runtime for **Linux, macOS, and Windows**: a small
native host plus a packed web UI. JavaScript can only reach what the host named
twice: a capability bit **and** an allowlist (host, command, path, template,
channel, or scheme). That contract is defined in [`PRODUCT.md`](PRODUCT.md).

**Status (honest):** the latest **published** host is
**[v0.1.6](https://github.com/ChloeVPin/kiri/releases/tag/v0.1.6)**. Workspace
`Cargo.toml` may already say `0.1.7`; that is source, not a download. Until OS
notarization / Authenticode, double-click onboarding without a cheatsheet,
no-clone embed-from-CI, and a kept-current public scoreboard all land, treat
this as an **early runtime + demo**, not a finished Tauri replacement
([`PRODUCT.md`](PRODUCT.md), [`VS_TAURI.md`](VS_TAURI.md)).

## What you can do today

| Goal | Path | Needs terminal? |
|------|------|-----------------|
| Try a window fast | Scaffold below → open `.app` / `run.cmd` / `run.sh` | Scaffold yes; macOS can be GUI after Gatekeeper |
| Own UI without rebuild | Edit `frontend/` next to the host | Re-run launcher |
| Pack UI into the binary | `KIRI_EMBED_FRONTEND=… cargo build` **from a clone** | Yes + Rust toolchain |
| CI zip of host + UI | Copy `templates/ship-app.yml` (sidecar frontend, not embed) | CI |
| One binary, UI inside | Copy `templates/embed-ipc-app/` + `templates/embed-ipc-app.yml` | CI + Rust |

## Platforms in the latest RELEASES.json

Only these assets exist in [v0.1.6](https://github.com/ChloeVPin/kiri/releases/tag/v0.1.6):

- `darwin-aarch64` (Apple Silicon)
- `linux-x86_64`
- `windows-x86_64`

Intel Mac and Linux ARM are **not** published; the scaffolder will error with a
missing platform URL.

## Platform prerequisites

- macOS: system WebView (Apple Silicon for published archives).
- Windows: Evergreen WebView2 runtime.
- Linux: GTK 3 and WebKit2GTK 4.1. Debian/Ubuntu:
  `sudo apt install libgtk-3-0 libwebkit2gtk-4.1-0`. Other distros: install the
  equivalent packages.

Kiri's public archives are **application-level** Ed25519 signed (`RELEASES.json`)
but **unsigned by the OS**. macOS may show an unidentified-developer
(Gatekeeper) warning; Windows may show SmartScreen. Suitable for evaluation and
development; notarization, Authenticode, and distro package signing remain
separate release work.

## 1. Scaffold an app (no git tree required)

There is no crates.io / npm / brew package yet. The evaluation path downloads
the latest **published** release host:

```sh
curl -fsSL https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.sh | bash -s ~/Desktop/my-kiri-app
```

Prefer not to pipe to bash? Clone this repo and run
`./tools/create-kiri-app.sh ~/Desktop/my-kiri-app` instead (same script; can use
local templates when present).

On Windows PowerShell:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.ps1))) "$HOME\Desktop\my-kiri-app"
```

Or, from a clone:

```powershell
& .\tools\create-kiri-app.ps1 "$HOME\Desktop\my-kiri-app"
```

### Run what you just created

- **macOS (Apple Silicon):** `open ~/Desktop/my-kiri-app/my-kiri-app.app`
  - First launch: if Gatekeeper blocks, right-click the app → **Open** (or
    System Settings → Privacy & Security).
  - Archives are **not** notarized. Integrity is Ed25519 via `RELEASES.json`.
- **Windows x86_64:** double-click `run.cmd`, or from PowerShell: `.\run.cmd`
  - SmartScreen may warn (no Authenticode). If the window never appears, install
    [WebView2 Evergreen](https://developer.microsoft.com/microsoft-edge/webview2/).
- **Linux x86_64:** `./run.sh` (no `.desktop` launcher yet).
  - Install GTK 3 + WebKit2GTK 4.1 first (see prerequisites).

Edit `frontend/` in that folder and run the same launcher again. Your UI
overrides the packed default at runtime.

## 2. Build from source

```sh
git clone https://github.com/ChloeVPin/kiri.git
cd kiri

# build the host (native to your OS)
cargo build -p kiri-runtime --bins

# run the smoke test (exit 0 + 9 startup markers)
./target/debug/kiri-host --smoke --frontend examples/blank

# run the interactive demo
KIRI_EMBED_FRONTEND="$PWD/examples/demo" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host
```

The host runs natively on every desktop platform: wry/tao on Linux and macOS,
and Win32 + WebView2 on Windows.

## 3. Ship your own UI

There are **two** different mechanisms. Do not mix the names.

### A) Runtime folder (no recompile): what scaffold uses

```sh
# host loads ./frontend instead of the packed default
./bin/kiri-host --frontend ./frontend
# or: KIRI_FRONTEND=/path/to/ui ./kiri-host
```

Public CI template: [`templates/ship-app.yml`](../templates/ship-app.yml)
downloads a release host and **copies `frontend/` beside it**. It does **not**
set `KIRI_EMBED_FRONTEND`.

### B) Compile-time embed (`KIRI_EMBED_FRONTEND`): needs Kiri source + Rust

Same idea as Tauri `frontendDist`. Point `KIRI_EMBED_FRONTEND` at your UI folder
(needs `index.html`). The host serves it over `kiri://localhost/index.html`:

```sh
git clone https://github.com/ChloeVPin/kiri.git && cd kiri
KIRI_EMBED_FRONTEND="/path/to/my-ui" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host
```

On macOS, package an unsigned double-clickable `.app`:

```sh
./tools/packaging/make-app.sh --frontend /path/to/my-ui
open artifacts/Kiri.app
```

### Single-binary embed template

[`templates/embed-ipc-app/`](../templates/embed-ipc-app/) is a thin app repo:
one `main.rs` that calls `kiri_runtime::run_session`, a `frontend/` that is a
through-webview IPC demo (repeated `window.kiri.send` round-trips with RTT
stats), and a `.cargo/config.toml` that sets `KIRI_EMBED_FRONTEND` for the
kiri-runtime build script. `cargo build --release` yields one binary with the
UI inside; the kiri source arrives as a Cargo git dependency, so no clone of
this repo is needed. [`templates/embed-ipc-app.yml`](../templates/embed-ipc-app.yml)
builds it on macOS, Windows, and Linux and uploads just the binary.

Compared with `create-kiri-app` + `ship-app.yml` for shipping this IPC app:

- Sidecar: no Rust needed (CI downloads a signed release host), but the
  artifact is a folder, host plus `frontend/` sidecar, and the pair can drift.
- Embed: needs a Rust toolchain and a full runtime build in CI (minutes, not
  seconds), but the artifact is one file and the UI cannot be swapped
  post-build, which is the honest fit for shipping an IPC demo.

Two caveats. On Windows the packed bytes are still materialized to a temp dir
at startup (WebView2 virtual-host mapping needs a real directory), so "single
binary" describes the shipped artifact, not the runtime filesystem view. On
headless Linux the smoke run depends on WebKit2GTK compositor init, the same
soft-gate as the correctness workflow.

What remains open against [`PRODUCT.md`](PRODUCT.md) criterion #3: the
artifacts are unsigned at the OS level and there is no crates.io package, so
the git dependency tracks a branch or tag rather than a semver release.

## 4. Talk to the host from JavaScript

The host injects a bridge script at document start that installs
`window.kiri`. Every command flows through `window.kiri.send(WireRequest)`
to the native `Router::dispatch`, which validates, authorizes, and
executes. The response comes back via `window.kiri.onResponse`.

Your frontend loads `kiri.js` (the API shim) which wraps the bridge:

```html
<script src="kiri.js"></script>
<script>
  var api = window.kiri;

  // read host facts
  api.app.version().then(function (v) { console.log("Kiri", v); });
  api.platform.os().then(function (os) { console.log("OS", os); });

  // write to the clipboard (host-owned controller, not direct OS access)
  api.clipboard.write("hello").then(function () { console.log("copied"); });

  // double-gated HTTP: the capability AND a host allowlist
  api.http.get("https://evil.example.com/")
    .then(function () { console.log("allowed (that is a bug)"); })
    .catch(function (e) { console.log("denied (correct):", e.message); });
</script>
```

Starter allowlists are baked into the downloaded host. Authoring a custom
allowlist for a scaffolded (no-clone) binary is **not** documented yet; build
a host with your policy from this tree. See [`API_REFERENCE.md`](API_REFERENCE.md).

## 5. The security model

Every native call is double-gated:

1. **Capability bit**: assigned by native code only. JavaScript never
   supplies the capability mask. The trusted frontend gets a fixed set of
   bits; unknown or ungranted commands return `Unauthorized`.
2. **Host allowlist**: even with the capability granted, the host refuses
   any target not on its explicit allowlist: shell commands, HTTP hosts,
   notification templates, dialog kinds, shortcut accelerators, store
   namespaces, deep-link schemes, opener targets, tray items, sidecar
   names, event channels, config keys.

This is the product: "a granted capability must not be enough."

## 6. Verification gates

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo build -p kiri-runtime --bins
cargo clippy -p kiri-runtime --all-targets -- -D warnings
cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets
cargo check --manifest-path baselines/wry-tao/Cargo.toml
cargo check --manifest-path baselines/tauri/Cargo.toml
```

See `AGENTS.md` for the full verification contract.
