# Product goal

Kiri is the desktop runtime you use when a granted capability must not be
enough.

A Kiri app is a small native host plus a packed web UI. JavaScript can only
reach what the host named twice: a capability bit **and** an allowlist
(host, command, path, template, channel, or scheme). Tauri often treats the
capability as sufficient. That difference is the product.

We do not compete on pixels. The window is the OS webview. If someone wants
a prettier page, they write a prettier page.

## Done looks like this

A person who has never cloned this repo can:

1. Download a current release for their OS, or scaffold an app in one
   command from a published Kiri package.
2. Open a window that talks to the host (version, OS, a real native
   action) without a terminal cheatsheet.
3. Ship their own UI by pointing `KIRI_EMBED_FRONTEND` at a folder and
   producing a Mac, Windows, and Linux artifact from CI.
4. Read a single page of published Kiri-vs-Tauri numbers that we would
   accept: through-webview IPC, embedded startup, binary size. No
   in-process bench dressed up as user-facing IPC. No disk-frontend
   startup dressed up as an embedded win.

Until those four are true, Kiri is a working runtime with a demo, not a
finished product.

## What we refuse

- Claiming we render faster than Tauri. Same engine.
- Quoting `bulk_bench` as IPC.
- Calling unsigned builds “ready for App Store / Microsoft Store.”
- Mobile. Desktop first; iOS/Android is a different product.
- Matching Tauri’s plugin count. Breadth is not the bet.

## What we will add, in order

1. **Trust at the OS gate.** Notarized macOS. Signed Windows. Still no
   lie about what that is: it gets past SmartScreen/Gatekeeper, it is
   not a store listing.
2. **A package people can depend on.** `create-kiri-app` and a crate or
   binary that does not require owning this git tree.
3. **Updates a normal person would run.** The Ed25519 verifier already
   exists; wire it to “there is a new build” in the host, with the
   host-pinned key, not a JS-supplied one.
4. **The published scoreboard, kept current.** Embedded startup vs Tauri
   on hosted Mac and Windows, every release.

## How we know we are losing

If a competent team can get the same security story from Tauri with less
work, or if wry/tao alone is as fast and simpler than the Kiri host, we
write that down and switch. The hypothesis is not a brand.
