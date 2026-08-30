# Getting started

Kiri is a native desktop app runtime. A Kiri app is a small native host
plus a packed web UI. JavaScript can only reach what the host named twice:
a capability bit **and** an allowlist (host, command, path, template,
channel, or scheme).

This guide takes you from zero to a running app without cloning this repo.

## Platform prerequisites

- macOS: macOS with its system WebView runtime.
- Windows: Windows with the Evergreen WebView2 runtime.
- Linux: GTK 3 and WebKit2GTK 4.1 runtime libraries. Debian/Ubuntu users
  can install them with `sudo apt install libgtk-3-0 libwebkit2gtk-4.1-0`.

Kiri's current public archives are application-level signed but unsigned by
the operating system. macOS may show an unidentified-developer warning, and
Windows may show SmartScreen. These archives are suitable for evaluation and
development; native notarization, Authenticode signing, and distro package
signing remain separate release work.

## 1. Scaffold an app (no git tree required)

```sh
curl -fsSL https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.sh | bash -s ~/Desktop/my-kiri-app
```

On Windows PowerShell, use the native scaffold script:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.ps1))) "$HOME\Desktop\my-kiri-app"
```

The script accepts a destination as its first argument when invoked directly:

```powershell
& .\create-kiri-app.ps1 "$HOME\Desktop\my-kiri-app"
```

This downloads the latest release host and starter UI, then assembles a
runnable app:

- macOS: `open ~/Desktop/my-kiri-app/my-kiri-app.app`
- Linux: `~/Desktop/my-kiri-app/run.sh`
- Windows: `my-kiri-app\run.cmd`

Edit `frontend/` in that folder and run again. Your UI overrides the packed
default.

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

The dev machine is macOS aarch64. The host runs natively on every desktop
platform: wry/tao on Linux and macOS, and Win32 + WebView2 on Windows.

## 3. Ship your own UI

The host packs a frontend at compile time (same idea as Tauri
`frontendDist`). Point `KIRI_EMBED_FRONTEND` at your UI folder:

```sh
KIRI_EMBED_FRONTEND="/path/to/my-ui" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host
```

Your UI folder needs an `index.html`. The host serves it over
`kiri://localhost/index.html`.

On macOS, package a double-clickable `.app`:

```sh
./tools/packaging/make-app.sh --frontend /path/to/my-ui
open artifacts/Kiri.app
```

The host also looks at `KIRI_FRONTEND` and a `frontend/` folder next to the
binary at runtime, so you can ship the binary and a `frontend/` folder
without recompiling.

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
    .then(function () { console.log("allowed — that is a bug"); })
    .catch(function (e) { console.log("denied (correct):", e.message); });
</script>
```

## 5. The security model

Every native call is double-gated:

1. **Capability bit** — assigned by native code only. JavaScript never
   supplies the capability mask. The trusted frontend gets a fixed set of
   bits; unknown or ungranted commands return `Unauthorized`.
2. **Host allowlist** — even with the capability granted, the host refuses
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
