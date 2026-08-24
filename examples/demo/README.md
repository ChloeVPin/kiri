# Kiri demo

A live host demo that exercises the real capability-gated control plane. It
reads version, OS, arch, and home directory through the bridge; writes and
reads a file in the host sandbox; demonstrates path math, clipboard,
window controls, the double-gated HTTP allowlist (an unapproved host is
denied), host-owned store persistence, and the host-pinned-key updater
check.

Every action flows through the real bridge to the native Router. No
terminal, no cheatsheet.

```sh
# from the Kiri repo
KIRI_EMBED_FRONTEND="$PWD/examples/demo" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host
```

Or package a double-clickable Mac app:

```sh
./tools/packaging/make-app.sh --frontend examples/demo
open artifacts/Kiri.app
```

The demo posts the same startup ready markers as `examples/blank`, so smoke
and the marker schema stay valid.

## What the demo proves

| Surface | Command IDs | Security property demonstrated |
|---------|------------|-------------------------------|
| Platform/app info | 5-7 | read-only facts, no env exposure |
| Window control | 14-22 | JS never reaches the native handle |
| Clipboard | 23-24 | host-owned controller, not direct OS access |
| Path/Os | 25-37 | capability-gated, no filesystem roots |
| File sandbox | 10-13 | scoped PathScope, host-owned sandbox root |
| HTTP allowlist | 38 | double-gated: capability AND host allowlist |
| Store | 45-46 | namespace-allowlisted, cross-module isolation |
| Updater | 61 | host-pinned Ed25519 key, never JS-supplied |
