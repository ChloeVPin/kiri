# Kiri vs Tauri - Gap Matrix and Exceed Roadmap

Last updated: 2026-08-15. Honest, evidence-tied list of what Tauri ships that
Kiri does not, ranked by winnability and value. Levels: A = Tauri/Kiri
source or docs; B = maintained impl; D = inference.

## Where Kiri already exceeds Tauri (keep sharpening)

1. Double-gating security axis (proven). All 61 control-plane command ids are
   denied with an empty capability set and granted only with the exact
   host-assigned bit. Verified end-to-end headlessly in
   crates/kiri-core/tests/full_router_capability_enforcement.rs.
2. Host-allowlisted every native surface (shell, http, notification, dialog,
   shortcut, autostart, store, deeplink, opener, tray, sidecar, event, config,
   window-state) even when the capability is granted.
3. Shared, frozen startup contract (schema_version 1) on all three backends.
4. Privacy-scoped diagnostics as a first-class subsystem.

## Functional gaps (Tauri has it, Kiri does not)

| # | Tauri capability | Kiri status | Win level |
|---|------------------|-------------|-----------|
| G-1 | Mobile (iOS/Android) | Absent (desktop only) | Hard / long |
| G-2 | 50+ official plugins + ecosystem | ABI implemented (R-2) + host-owned external plugin loader with per-plugin command allowlist (fail-closed): third-party plugins can only load when host-approved by name and only expose allowlisted commands. Exceeds Tauri's plugin model (trusts any plugin on path) on the security axis. Ecosystem breadth (50+ plugins) still absent. | Medium |
| G-3 | Bundler + autoupdater | Signed-update verifier done; no packaging/signing (needs certs) | Medium / blocked on certs |
| G-4 | tauri:// full protocol (range, mime, cache) | kiri:// mime+range+ETag+origin allowlist on macOS/Linux; Windows parity pending real hardware | Medium |
| G-5 | JS API breadth (cli, process) | DONE: kiri.cli.args (id 66) structured + allowlist-scoped, exceeds Tauri process.argv; process partially covered by shell/sidecar | Easy (cli) / Medium (process) |
| G-9 | HTTP verbs beyond GET | DONE: kiri.http.post/put/patch/delete with body + method allowlist (exceeds Tauri) | Easy/Medium |
| G-10 | fs watch | DONE: kiri.fs.watch/unwatch (ids 67/68) host-allowlisted path inside PathScope (exceeds Tauri) | Medium |
| G-11 | WebSocket / protocol upgrade | DONE: kiri.ws.connect/send/close (ids 69/70/71) host-allowlisted URL (exceeds Tauri) | Medium |
| G-12 | App menu (not just tray) | DONE: kiri.menu.set/invoke (ids 72/73) host-owned item allowlist (tray shape, exceeds Tauri) | Medium |
| G-13 | Updater JS binding | DONE: kiri.updater.check (id 61) wired backend + JS binding + tests (audit-18) | Easy |
| G-7 | Docs, templates, community, brand | Early-stage, tiny | Process |

## Where Tauri is currently better (do not fake)

- F-1 Asset loading maturity: tauri:// is a registered OS protocol with
  optimization paths; Kiri kiri:// on macOS/Linux does per-request file reads
  (now mime/range/ETag) but is younger; Windows WebView2 path is OS-handled and
  not yet feature-parity-checked (no real hardware).
- F-3 Ergonomics/examples: Tauri #[tauri::command] and plugin ecosystem are the
  de-facto standard with huge example coverage; Kiri numeric routing is
  faster/auditable but has near-zero examples.

## Ranked exceed roadmap (next concrete work)

1. ~~G-9 HTTP verbs~~ DONE: kiri.http.post/put/patch/delete with body + method
   allowlist. Highest ROI, fully headless-testable.
2. ~~G-13 Updater JS binding~~ DONE: kiri.updater.check (id 61) already wired (audit-18).
3. ~~G-5 cli~~ DONE: kiri.cli.args structured + allowlist-scoped (exceeds Tauri).
4. ~~G-10 fs watch~~ DONE: kiri.fs.watch host-allowlisted (exceeds Tauri).
5. ~~G-11 WebSocket~~ DONE: kiri.ws host-allowlisted URL (exceeds Tauri).
6. ~~G-12 App menu~~ DONE: kiri.menu host-owned item allowlist (exceeds Tauri).
7. G-3 Packaging - once signing certs exist, build MSI/dmg/AppImage and wire the
   signed-update verifier into release.
8. G-1 Mobile - out of scope until desktop dominant; record as hypothesis.

## Honest bottom line

Kiri cannot beat Tauri on ecosystem, docs, mobile, or community short term. It
CAN and DOES beat Tauri on the security axis, startup-contract rigor, and
control-plane auditability. Fastest path to exceed on every winnable dimension:
All headless-runnable surface gaps (G-9, G-13, G-5, G-10, G-11, G-12) are DONE; remaining: G-3 packaging (certs) and G-1 mobile (out of scope)
Mac and all preserve the security model.
