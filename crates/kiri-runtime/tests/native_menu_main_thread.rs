//! Host accelerator coverage for the native menu adapters, on the real main thread.
//!
//! `muda` builds menu children through `objc2`'s `MainThreadMarker`, which
//! asserts it is running on the process main thread. The default libtest
//! harness runs every `#[test]` on a spawned thread, so a `#[test]` in
//! `native_menu` can never construct a `muda` `Menu` on macOS. This target
//! sets `harness = false` (see `Cargo.toml`), so `main()` below *is* the
//! process main thread and the real construction path runs on all platforms.
//!
//! What it asserts, per platform adapter:
//!   1. A host-owned accelerator string is parsed and accepted: applying the
//!      menu succeeds and the item id is registered (a later `Invoke` for that
//!      id resolves, an unknown id still does not).
//!   2. The adapter is not installed until `install`/`replace` runs.
//!   3. An invalid host accelerator still fails the whole apply closed.

use std::process;

use kiri_core::app_menu::MenuItem;
use kiri_core::error::ErrorCode;
use kiri_runtime::menu_accel::parse_accelerator;
use kiri_runtime::menu_dispatch::OperationKind;

#[cfg(not(target_os = "windows"))]
use kiri_runtime::native_menu::NativeMenu as Adapter;
#[cfg(target_os = "windows")]
use kiri_runtime::native_menu_windows::NativeMenuWindows as Adapter;

fn item(id: &str, accelerator: Option<&str>) -> MenuItem {
    MenuItem {
        id: id.into(),
        label: id.to_string(),
        action: id.into(),
        accelerator: accelerator.map(str::to_string),
    }
}

fn check(name: &str, f: impl FnOnce() + std::panic::UnwindSafe) {
    match std::panic::catch_unwind(f) {
        Ok(()) => {}
        Err(_) => {
            eprintln!("FAIL: {name}");
            process::exit(1);
        }
    }
}

fn main() {
    // Parsing a host accelerator is platform independent and is what turns the
    // host string into the muda Accelerator the adapters hand to muda.
    check("parse_accelerator maps CmdOrCtrl+Q to KeyQ", || {
        let accel = parse_accelerator(&item("quit", Some("CmdOrCtrl+Q"))).unwrap().unwrap();
        assert_eq!(accel.key(), muda::accelerator::Code::KeyQ);
    });
    check("parse_accelerator rejects a malformed host string", || {
        let err = parse_accelerator(&item("quit", Some("NotAModifier+Q"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    });

    // The real thing: build the native menu through the platform adapter,
    // on this thread, which is the process main thread.
    check("set_items accepts a host accelerator and registers the id", || {
        let mut menu = Adapter::new();
        menu.apply(OperationKind::Set(&[item("quit", Some("CmdOrCtrl+Q")), item("show", None)]))
            .expect("valid host accelerator must be accepted");

        // The id is installed, so Invoke for it resolves...
        menu.apply(OperationKind::Invoke { id: "quit", action: "quit" })
            .expect("registered id must resolve");
        // ...and an id that was never installed still does not.
        let err = menu
            .apply(OperationKind::Invoke { id: "nope", action: "nope" })
            .expect_err("unregistered id must stay denied");
        assert_eq!(err.code, ErrorCode::ServiceUnavailable, "{err:?}");
    });

    check("an invalid host accelerator fails the whole apply closed", || {
        let mut menu = Adapter::new();
        let err = menu
            .apply(OperationKind::Set(&[item("quit", Some("NotAModifier+Q"))]))
            .expect_err("invalid accelerator must fail the apply");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    });

    check("applying a menu does not install it", || {
        let mut menu = Adapter::new();
        menu.apply(OperationKind::Set(&[item("quit", Some("CmdOrCtrl+Q"))])).unwrap();
        #[cfg(not(target_os = "windows"))]
        assert!(!menu.is_installed(), "install() must be a separate event-loop-thread step");
    });

    println!("native_menu main-thread accelerator checks passed");
}
