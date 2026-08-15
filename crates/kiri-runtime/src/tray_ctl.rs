//! Host-side `TrayRunner` implementations that bridge the core
//! `kiri.tray.*` command surface to real OS tray backends.
//!
//! The backend is the ONLY place that materializes the native tray; the core has
//! already enforced the `TRAY` capability and the host item-id allowlist before
//! any call reaches here, so the frontend can never choose a label, an action, or
//! an arbitrary native menu. That inverts Tauri's tray, which lets the frontend
//! build the native menu freely once the capability is present.
//!
//! The cross/win backends record the menu in a host-owned map in this headless
//! build; a real backend would bind the OS tray icon and menu behind the same
//! `TrayRunner` trait. The cfg split keeps each target compiling only its own
//! dependency set, mirroring the other controllers.

use std::sync::{Arc, Mutex};

use kiri_core::error::Result;
use kiri_core::tray::{TrayItem, TrayRunner};

/// Host-owned tray state. The core allowlist is the authority on which items
/// exist, but the store itself is host-owned and never addressed directly by JS.
#[derive(Debug, Default)]
pub struct HostTray {
    menu: Mutex<Vec<TrayItem>>,
    last_invoked: Mutex<Option<String>>,
}

impl HostTray {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl TrayRunner for HostTray {
    fn set_menu(&self, items: &[TrayItem]) -> Result<()> {
        *self.menu.lock().unwrap() = items.to_vec();
        Ok(())
    }

    fn invoke(&self, id: &str, action: &str) -> Result<()> {
        *self.last_invoked.lock().unwrap() = Some(id.to_string());
        let _ = action;
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub mod cross_tray {
    use super::*;

    pub struct CrossTrayBackend {
        inner: Arc<HostTray>,
    }

    impl Default for CrossTrayBackend {
        fn default() -> Self {
            Self { inner: HostTray::new() }
        }
    }

    impl CrossTrayBackend {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostTray>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostTray> {
            self.inner.clone()
        }
    }

    impl TrayRunner for CrossTrayBackend {
        fn set_menu(&self, items: &[TrayItem]) -> Result<()> {
            self.inner.set_menu(items)
        }

        fn invoke(&self, id: &str, action: &str) -> Result<()> {
            self.inner.invoke(id, action)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_tray {
    use super::*;

    pub struct WinTrayBackend {
        inner: Arc<HostTray>,
    }

    impl Default for WinTrayBackend {
        fn default() -> Self {
            Self { inner: HostTray::new() }
        }
    }

    impl WinTrayBackend {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostTray>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostTray> {
            self.inner.clone()
        }
    }

    impl TrayRunner for WinTrayBackend {
        fn set_menu(&self, items: &[TrayItem]) -> Result<()> {
            self.inner.set_menu(items)
        }

        fn invoke(&self, id: &str, action: &str) -> Result<()> {
            self.inner.invoke(id, action)
        }
    }
}
