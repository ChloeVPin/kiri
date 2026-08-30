//! Thread-affine Windows application-menu adapter backed by `muda`.

#![cfg(target_os = "windows")]

use std::collections::HashMap;

use kiri_core::app_menu::MenuItem;
use kiri_core::error::{Error, Result};
use muda::{IsMenuItem, Menu, MenuId};
use windows::Win32::Foundation::HWND;

use crate::menu_dispatch::OperationKind;

pub struct NativeMenuWindows {
    menu: Option<Menu>,
    actions: HashMap<MenuId, (String, String)>,
    installed: bool,
}

impl NativeMenuWindows {
    pub fn new() -> Self {
        Self { menu: None, actions: HashMap::new(), installed: false }
    }

    pub fn apply(&mut self, operation: OperationKind<'_>) -> Result<()> {
        match operation {
            OperationKind::Set(items) => self.set_items(items),
            OperationKind::Invoke { id, .. }
                if self.actions.values().any(|(item, _)| item == id) =>
            {
                Ok(())
            }
            OperationKind::Invoke { .. } => {
                Err(Error::service_unavailable("kiri.menu item is not installed"))
            }
        }
    }

    pub fn replace(&mut self, hwnd: HWND, operation: OperationKind<'_>) -> Result<()> {
        match operation {
            OperationKind::Set(items) => {
                if items.is_empty() {
                    if self.installed {
                        let menu = self.menu.as_ref().ok_or_else(|| {
                            Error::service_unavailable("kiri.menu has no native menu to remove")
                        })?;
                        unsafe { menu.remove_for_hwnd(hwnd.0 as isize) }.map_err(|e| {
                            Error::service_unavailable(format!("kiri.menu removal failed: {e}"))
                        })?;
                        self.installed = false;
                    }
                    self.menu = None;
                    self.actions.clear();
                    self.installed = false;
                    return Ok(());
                }
                let mut pending = Self::new();
                pending.set_items(items)?;
                if self.installed {
                    let menu = self.menu.as_ref().ok_or_else(|| {
                        Error::service_unavailable("kiri.menu has no native menu to remove")
                    })?;
                    unsafe { menu.remove_for_hwnd(hwnd.0 as isize) }.map_err(|e| {
                        Error::service_unavailable(format!("kiri.menu removal failed: {e}"))
                    })?;
                    self.installed = false;
                }
                self.menu = pending.menu;
                self.actions = pending.actions;
                let menu = self.menu.as_ref().ok_or_else(|| {
                    Error::service_unavailable("kiri.menu cannot install an empty native menu")
                })?;
                unsafe { menu.init_for_hwnd(hwnd.0 as isize) }.map_err(|e| {
                    Error::service_unavailable(format!("kiri.menu installation failed: {e}"))
                })?;
                self.installed = true;
                Ok(())
            }
            OperationKind::Invoke { .. } => self.apply(operation),
        }
    }

    pub fn action_for(&self, id: &MenuId) -> Option<(&str, &str)> {
        self.actions.get(id).map(|(item, action)| (item.as_str(), action.as_str()))
    }

    fn set_items(&mut self, items: &[MenuItem]) -> Result<()> {
        let native_items: Vec<muda::MenuItem> = items
            .iter()
            .map(|item| muda::MenuItem::with_id(item.id.clone(), item.label.clone(), true, None))
            .collect();
        let refs: Vec<&dyn IsMenuItem> =
            native_items.iter().map(|item| item as &dyn IsMenuItem).collect();
        self.menu = Some(Menu::with_items(&refs).map_err(|e| {
            Error::service_unavailable(format!("kiri.menu Windows build failed: {e}"))
        })?);
        self.actions = items
            .iter()
            .zip(native_items.iter())
            .map(|(item, native)| (native.id().clone(), (item.id.clone(), item.action.clone())))
            .collect();
        self.installed = false;
        Ok(())
    }
}

impl Default for NativeMenuWindows {
    fn default() -> Self {
        Self::new()
    }
}
