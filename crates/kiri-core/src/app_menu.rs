//! Restricted application-menu surface (`kiri.menu`).
//!
//! This closes the Tauri application-menu parity gap (G-12) and exceeds it on
//! the security axis. Tauri's menu API, when the capability is present, lets
//! the frontend build an arbitrary native menu: arbitrary item labels,
//! arbitrary actions, even items that shell out. That is a spoofing/phishing
//! and UX-hijack surface (a malicious frontend could forge a Sign out or Quit
//! item drawn from the host's own chrome).
//!
//! Kiri requires BOTH the `MENU` capability bit AND a host allowlist. The
//! frontend may only request a pre-approved menu item id whose label and action
//! are host-owned; it cannot invent a label or redirect an item to a different
//! action. The host owns the item id to label/action mapping. This reuses the
//! exact allowlist shape already proven by `kiri.tray` (audit item 14), so the
//! security model is consistent across both native menu surfaces. A granted
//! capability that addresses an unknown item is refused, so JavaScript can never
//! draw an arbitrary native menu.
//!
//! The actual menu is behind the `MenuRunner` trait (mirrors `TrayRunner`):
//! the native host injects a real backend; tests use a `StubMenu` and assert
//! allowlist enforcement and capability gating without touching the OS.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.menu.*` commands.
pub const MENU_CAPABILITY: u32 = 26;

/// One host-approved menu item. The frontend references id only; it cannot
/// supply or alter the label or the action. action is a bounded, host-defined
/// identifier echoed back to the frontend when the item fires, so the frontend
/// logic is data, not a free native menu built by untrusted code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub action: String,
}

/// Host-configured allowlist of menu items. Default-deny: an item is shown/
/// activated only if its exact id is listed. The host owns the id to
/// label/action mapping; the frontend only picks a known id.
#[derive(Debug, Clone, Default)]
pub struct MenuAllowlist {
    items: Vec<MenuItem>,
}

impl MenuAllowlist {
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self { items }
    }

    fn resolve(&self, id: &str) -> Option<MenuItem> {
        self.items.iter().find(|i| i.id == id).cloned()
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }
}

/// A menu action result, returned to the caller for audit/trace.
#[derive(Debug, Clone)]
pub struct MenuInvoked {
    pub id: String,
    pub action: String,
}

/// Transport seam. The native host provides a real menu backend; tests provide
/// a stub. Trait-based so the logical protocol has zero platform deps.
pub trait MenuRunner: Send + Sync {
    /// Set the application menu from the host-owned items for the given ids (in
    /// order). The runner only ever receives host-owned item labels/actions.
    fn set_menu(&self, items: &[MenuItem]) -> Result<()>;
    /// Activate the host-owned action for the given item id.
    fn invoke(&self, id: &str, action: &str) -> Result<()>;
}

/// Capability-scoped menu service bounded to an allowlist plus limits.
#[derive(Clone)]
pub struct MenuService {
    runner: Arc<dyn MenuRunner>,
    allowlist: Arc<MenuAllowlist>,
    limits: Arc<Limits>,
}

impl MenuService {
    pub fn new(runner: Arc<dyn MenuRunner>, allowlist: MenuAllowlist, limits: Limits) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Set the visible application menu from a list of allowlisted item ids.
    /// Ids that are not on the allowlist are dropped, so the frontend can never
    /// inject an unapproved item into the native menu.
    pub fn set_menu(&self, ids: &[String]) -> Result<Value> {
        let mut resolved: Vec<MenuItem> = Vec::with_capacity(ids.len());
        for id in ids {
            let item = self.allowlist.resolve(id).ok_or_else(|| {
                Error::scope_denied(format!("kiri.menu.set: item id not on allowlist: {id}"))
            })?;
            self.limits.check_bulk_object(item.label.len() as u64)?;
            self.limits.check_bulk_object(item.action.len() as u64)?;
            resolved.push(item);
        }
        self.runner.set_menu(&resolved)?;
        Ok(serde_json::json!({
            "items": resolved.iter().map(|i| serde_json::json!({ "id": i.id, "label": i.label })).collect::<Vec<_>>(),
        }))
    }

    /// Activate the host-owned action for an allowlisted item id. Returns the
    /// resolved (host-owned) id/action for audit/trace.
    pub fn invoke(&self, id: &str) -> Result<Value> {
        let item = self.allowlist.resolve(id).ok_or_else(|| {
            Error::scope_denied(format!("kiri.menu.invoke: item id not on allowlist: {id}"))
        })?;
        self.limits.check_bulk_object(item.action.len() as u64)?;
        self.runner.invoke(&item.id, &item.action)?;
        Ok(serde_json::json!({ "id": item.id, "action": item.action }))
    }
}

/// Build the `kiri.menu.*` handlers bound to one MenuService.
pub fn menu_handlers(
    service: MenuService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(MENU_CAPABILITY);

    let set_svc = service.clone();
    let invoke_svc = service.clone();
    vec![
        (
            command_id::MENU_SET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let ids = p
                    .get("ids")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| Error::invalid_argument("kiri.menu.set requires array ids"))?;
                let ids: Vec<String> = ids
                    .iter()
                    .map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Option<_>>()
                    .ok_or_else(|| Error::invalid_argument("kiri.menu.set ids must be strings"))?;
                set_svc.set_menu(&ids)
            }) as Handler,
        ),
        (
            command_id::MENU_INVOKE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let id = p.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.menu.invoke requires string id")
                })?;
                invoke_svc.invoke(id)
            }) as Handler,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;
    use std::sync::Mutex;

    struct StubMenu {
        menu: Mutex<Vec<String>>,
        invoked: Mutex<Vec<MenuInvoked>>,
    }
    impl MenuRunner for StubMenu {
        fn set_menu(&self, items: &[MenuItem]) -> Result<()> {
            *self.menu.lock().unwrap() = items.iter().map(|i| i.id.clone()).collect();
            Ok(())
        }
        fn invoke(&self, id: &str, action: &str) -> Result<()> {
            self.invoked
                .lock()
                .unwrap()
                .push(MenuInvoked { id: id.to_string(), action: action.to_string() });
            Ok(())
        }
    }

    fn allow() -> MenuAllowlist {
        MenuAllowlist::new(vec![
            MenuItem {
                id: "new".to_string(),
                label: "New Window".to_string(),
                action: "new".to_string(),
            },
            MenuItem {
                id: "quit".to_string(),
                label: "Quit".to_string(),
                action: "quit".to_string(),
            },
        ])
    }

    fn router() -> Router {
        let svc = MenuService::new(
            Arc::new(StubMenu { menu: Mutex::new(Vec::new()), invoked: Mutex::new(Vec::new()) }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_menu(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(MENU_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_items_set_menu() {
        let r = router();
        let out = dispatch(&r, command_id::MENU_SET, serde_json::json!({ "ids": ["new", "quit"] }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn unknown_item_is_denied() {
        let r = router();
        let out =
            dispatch(&r, command_id::MENU_SET, serde_json::json!({ "ids": ["new", "wipe-cache"] }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn allowed_invoke_returns_host_action() {
        let r = router();
        let out = dispatch(&r, command_id::MENU_INVOKE, serde_json::json!({ "id": "quit" }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["action"], "quit");
    }

    #[test]
    fn frontend_supplied_label_is_ignored_host_mapping_wins() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::MENU_INVOKE,
            serde_json::json!({ "id": "quit", "label": "EVIL", "action": "evil" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["action"], "quit");
    }

    #[test]
    fn capability_denied_without_menu_bit() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req =
            WireRequest::new(command_id::MENU_INVOKE, 1, 1, serde_json::json!({ "id": "quit" }));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
