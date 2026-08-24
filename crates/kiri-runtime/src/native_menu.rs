//! Thread-affine `muda` application-menu adapter.
//!
//! This type is intentionally UI-thread-only. `MenuDispatcher` owns the
//! sendable command boundary; the host event loop owns this adapter and calls
//! `apply` while draining that dispatcher.

#![cfg(not(target_os = "windows"))]

use std::collections::HashMap;

use kiri_core::app_menu::MenuItem;
use kiri_core::error::{Error, Result};
use muda::{IsMenuItem, Menu, Submenu};

use crate::menu_dispatch::OperationKind;

pub struct NativeMenu {
    menu: Option<Menu>,
    item_ids: HashMap<String, muda::MenuId>,
    installed: bool,
}

impl NativeMenu {
    pub fn new() -> Self {
        Self { menu: None, item_ids: HashMap::new(), installed: false }
    }

    /// Install the current native menu on the host window or application.
    /// Must be called on the tao event-loop thread.
    pub fn install(&mut self, window: &tao::window::Window) -> Result<()> {
        let menu = self.menu.as_ref().ok_or_else(|| {
            Error::service_unavailable("kiri.menu cannot install an empty native menu")
        })?;
        #[cfg(target_os = "macos")]
        {
            let _ = window;
            menu.init_for_nsapp();
        }
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;
            let container = window.default_vbox();
            menu.init_for_gtk_window(window.gtk_window(), container).map_err(|e| {
                Error::service_unavailable(format!("kiri.menu GTK install failed: {e}"))
            })?;
        }
        self.installed = true;
        Ok(())
    }

    /// Rebuild and, when already installed, replace the native menu.
    pub fn apply(&mut self, operation: OperationKind<'_>) -> Result<()> {
        match operation {
            OperationKind::Set(items) => self.set_items(items),
            OperationKind::Invoke { id, .. } => {
                if self.item_ids.contains_key(id) {
                    Ok(())
                } else {
                    Err(Error::service_unavailable("kiri.menu item is not installed"))
                }
            }
        }
    }

    fn set_items(&mut self, items: &[MenuItem]) -> Result<()> {
        let native_items: Vec<muda::MenuItem> = items
            .iter()
            .map(|item| muda::MenuItem::with_id(item.id.clone(), item.label.clone(), true, None))
            .collect();
        let refs: Vec<&dyn IsMenuItem> =
            native_items.iter().map(|item| item as &dyn IsMenuItem).collect();

        #[cfg(target_os = "macos")]
        let menu = {
            let submenu =
                Submenu::with_id_and_items("kiri.app", "Kiri", true, &refs).map_err(|e| {
                    Error::service_unavailable(format!("kiri.menu macOS build failed: {e}"))
                })?;
            Menu::with_items(&[&submenu as &dyn IsMenuItem]).map_err(|e| {
                Error::service_unavailable(format!("kiri.menu macOS root failed: {e}"))
            })?
        };
        #[cfg(target_os = "linux")]
        let menu = Menu::with_items(&refs)
            .map_err(|e| Error::service_unavailable(format!("kiri.menu GTK build failed: {e}")))?;

        self.menu = Some(menu);
        self.item_ids = items
            .iter()
            .zip(native_items.iter())
            .map(|(item, native)| (item.id.clone(), native.id().clone()))
            .collect();
        // A later host integration will remove the old native attachment
        // before installing this replacement. Do not claim replacement here.
        self.installed = false;
        Ok(())
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }
}

impl Default for NativeMenu {
    fn default() -> Self {
        Self::new()
    }
}
