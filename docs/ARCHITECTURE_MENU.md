# Native Application Menu Architecture

This document defines the implementation boundary for Kiri's application-menu
surface. The command contract and security policy are implemented in
`kiri-core`. The runtime now contains the thread-affine `muda` adapter and
bounded dispatcher; host lifecycle wiring and native event delivery remain
the integration task.

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
IDs via `muda 0.19.3` (`native_menu.rs` on macOS/Linux, `native_menu_windows.rs` on Windows) and a bounded `MenuDispatcher` (`menu_dispatch.rs:32` queue 32, 2 s timeout). Both backends wire the dispatcher on the event-loop thread (`host_cross.rs:427` `MenuDispatcher::new()` + `host_cross.rs:636` drain + `host_windows.rs:358` wnd_proc drain) and forward `muda::MenuEvent` to `window.kiri.onMenuAction`. The production `MenuRunner` is the dispatcher handle (`MenuDispatcherHandle: MenuRunner`), not `DisabledMenu` — the command surface (`kiri.menu.set` id 72 / `invoke` id 73) is capability-gated and allowlist-enforced in `kiri_core::app_menu.rs:115`.

It does not claim replacement-safe reinstallation, event forwarding, or
support for submenus, checkboxes, radio items, roles, icons, and accelerators;
each requires separate acceptance tests per platform.

## Acceptance evidence

The feature is complete only when all of the following are demonstrated:

- a real native menu is visible on macOS, Windows, and Linux;
- an allowlisted item invokes its host action and reaches the correct window;
- an unknown ID cannot create or invoke an item;
- concurrent `set` and `invoke` calls cannot observe a partially applied menu;
- shutdown and event-loop failure return bounded errors without a deadlock;
- the same logical menu produces equivalent action IDs across platforms;
- manual keyboard navigation and screen-reader checks cover the native menu;
- hosted correctness tests exercise native menu creation and activation on each
  OS rather than only compiling the adapter.

Current status: dispatcher + adapters are implemented and wired. Unit tests cover bounded queue, timeout, duplicate-ID, unknown-ID, and thread-affine `replace`/`install` (`menu_dispatch.rs:150`, `kiri_core::app_menu::tests`, `host_cross.rs:771` production-router catalog). Hosted native smoke (visible menu + activation) remains the open proof. Until that hosted eye-test is green on `macos-latest` and `windows-latest`, the scoreboard in `CROSS_PLATFORM_STATUS.md` carries the `incomplete` caveat.
