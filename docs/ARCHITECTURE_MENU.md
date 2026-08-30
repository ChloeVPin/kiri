# Native Application Menu Architecture

This document defines the implementation boundary for Kiri's application-menu
surface. The command contract and security policy are implemented in
`kiri-core`. The runtime contains the thread-affine `muda` adapter and
bounded dispatcher, and both hosts wire it on the event-loop thread and
forward native selection to `window.kiri.onMenuAction`; the remaining work
is manual keyboard/screen-reader verification.

## Contract

`kiri.menu.set` accepts only item IDs from the host-owned `MenuAllowlist`.
Labels and actions are resolved by the host. `kiri.menu.invoke` performs the
same resolution before activation. Capability checks happen before either
handler runs. Unknown IDs, ungranted capability, oversized values, and
backend failures are explicit errors.

The frontend must never provide arbitrary native labels, shell commands, menu
roles, accelerators, or submenu structure. A host application may expose a
richer schema later, but every field must remain host-approved and bounded.

## Threading model

The `MenuRunner` trait is currently `Send + Sync` because command dispatch can
be reached from the control-plane worker. Native menu objects are generally
owned by the platform event-loop thread and must not be moved behind a mutex.
The runtime therefore needs a dispatcher adapter rather than an unsafe global
or an `Arc` around a thread-affine object.

The adapter should:

1. receive an immutable, validated menu snapshot from the command router;
2. enqueue a bounded operation onto the host event-loop thread;
3. apply the snapshot on that thread and return a completion result;
4. translate native item selection into a bounded host action event;
5. route that event to the owning window without evaluating arbitrary script.

The dispatcher must define behavior for shutdown, a closed event loop, a
second `set` while the first is pending, duplicate IDs, and a timeout. The
last successfully applied snapshot should remain the authoritative state for
diagnostics, but it must not be presented as native state after an apply error.

This boundary is required by the native menu library itself: `muda::Menu` is
not `Send` or `Sync`, macOS menu operations must run on the main thread,
Windows menu accelerators depend on the native message loop, and Linux menu
installation requires a GTK window. See the maintained [`muda::Menu` API]
and its [platform notes].

[`muda::Menu` API]: https://docs.rs/muda/latest/muda/struct.Menu.html
[platform notes]: https://docs.rs/muda/latest/muda/#platform-specific-notes

## Platform adapters

- macOS: map the validated snapshot to the application menu on the main
  event-loop thread and retain stable native item identifiers for selection.
- Windows: map it to the native application-menu owner associated with the
  window, preserving accelerator and command routing on the UI thread.
- Linux: select the supported desktop backend explicitly. GTK and other
  desktop environments do not have identical menu semantics, so unsupported
  roles must be rejected rather than silently approximated.

The current adapter supports ordinary host-owned clickable items and stable
IDs via `muda 0.19.3` (`native_menu.rs` on Linux/macOS, `native_menu_windows.rs` on Windows) and a bounded `MenuDispatcher` (`menu_dispatch.rs:11` queue 32, 2 s timeout). Both backends wire the dispatcher on the event-loop thread (`host_cross.rs:428` `MenuDispatcher::new()` + `host_cross.rs:648` drain + `host_windows.rs:358` wnd_proc drain) and forward `muda::MenuEvent` to `window.kiri.onMenuAction` (`host_cross.rs:669`, `host_windows.rs:360`). The production `MenuRunner` is the dispatcher handle (`MenuDispatcherHandle: MenuRunner`), not `DisabledMenu` — the command surface (`kiri.menu.set` id 72 / `invoke` id 73) is capability-gated and allowlist-enforced in `kiri_core::app_menu.rs:115`. `replace` is replacement-safe: it builds the new menu off-thread-local state first, then removes the old OS menu before installing the new one, handles empty-set as clear (`native_menu.rs:65`, `native_menu_windows.rs:39`), and treats `invoke` as validation without reinstall.

It does not claim support for submenus, checkboxes, radio items, roles, icons, and accelerators;
each requires separate acceptance tests per platform.

## Acceptance evidence

The feature is complete only when all of the following are demonstrated:

- a real native menu is visible on Linux, macOS, and Windows;
- an allowlisted item invokes its host action and reaches the correct window;
- an unknown ID cannot create or invoke an item;
- concurrent `set` and `invoke` calls cannot observe a partially applied menu;
- shutdown and event-loop failure return bounded errors without a deadlock;
- the same logical menu produces equivalent action IDs across platforms;
- manual keyboard navigation and screen-reader checks cover the native menu;
- hosted correctness tests exercise native menu creation and activation on each
  OS rather than only compiling the adapter.

Current status: dispatcher + adapters are wired and smoke-tested. Unit tests cover bounded queue (32), 2 s timeout, closed-queue, concurrent set/invoke serialization, duplicate-ID, unknown-ID, and thread-affine `replace`/`install` (`menu_dispatch.rs:150`, `kiri_core::app_menu::tests`, `host_cross.rs:771` production-router catalog). Hosted through-webview smoke exercises `kiri.menu.set` (72) + `kiri.menu.invoke` (73) via the real bridge and fails the run on `menu_smoke.ok == false` (`examples/menu-smoke/index.html:32`, `host_cross.rs:541`, `host_windows.rs:955`); it is wired in `correctness.yml` on `macos-latest` and `windows-latest` (`correctness.yml:134`, `correctness.yml:161`). What remains is the manual human eye-test: visible native menu, keyboard navigation, and screen-reader announcement per platform. Until that manual check is recorded, `CROSS_PLATFORM_STATUS.md` marks menu as wired-but-unseen.
