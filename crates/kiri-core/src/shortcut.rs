//! Restricted global-shortcut surface (`kiri.shortcut`).
//!
//! This closes a Tauri plugin parity gap and converts Tauri's global-shortcut
//! risk into a Kiri strength. Tauri's global-shortcut plugin, when the
//! capability is granted, lets the frontend register an arbitrary global key
//! combo. That is a focus/UX-hijack surface: a compromised or malicious
//! frontend could bind a sensitive combo (Cmd+Q, a password-manager autofill
//! chord, etc.) and intercept it globally across the whole desktop.
//!
//! Kiri requires BOTH the `SHORTCUT` capability bit AND a host allowlist. The
//! frontend may only *enable* a pre-approved binding (an exact accelerator
//! mapped to a bounded action id); it cannot invent an accelerator or rebind a
//! combo to a different action. The host owns the accelerator->action mapping.
//! A granted capability with no matching allowlist entry is refused, so
//! JavaScript can never register an arbitrary global hotkey.
//!
//! The actual registration is behind the `ShortcutRunner` trait (mirrors
//! `ShellRunner`/`NotificationRunner`): the native host injects a real
//! registrar; tests use a `StubShortcut` and assert allowlist enforcement and
//! capability gating without touching the OS.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.shortcut.*` commands.
pub const SHORTCUT_CAPABILITY: u32 = 14;

/// One host-approved global shortcut. The frontend may only enable the exact
/// `accelerator`; it cannot supply or alter the accelerator string. `action` is
/// a bounded, host-defined identifier echoed back to the frontend when the combo
/// fires, so the frontend logic is data, not a free OS keybinding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutBinding {
    pub accelerator: String,
    pub action: String,
}

/// Host-configured allowlist of global shortcuts. Default-deny: a combo
/// registers only if its exact accelerator is listed. The host owns the mapping
/// from accelerator to action; the frontend only picks a known combo.
#[derive(Debug, Clone, Default)]
pub struct ShortcutAllowlist {
    bindings: Vec<ShortcutBinding>,
}

impl ShortcutAllowlist {
    pub fn new(bindings: Vec<ShortcutBinding>) -> Self {
        Self { bindings }
    }

    fn resolve(&self, accelerator: &str) -> Option<ShortcutBinding> {
        self.bindings.iter().find(|b| b.accelerator == accelerator).cloned()
    }

    pub fn bindings(&self) -> &[ShortcutBinding] {
        &self.bindings
    }
}

/// A registered shortcut result, returned to the caller for audit/trace.
#[derive(Debug, Clone)]
pub struct ShortcutRegistered {
    pub accelerator: String,
    pub action: String,
}

/// Transport seam. The native host provides a real registrar; tests provide a
/// stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait ShortcutRunner: Send + Sync {
    /// Register/enable the exact accelerator under `action`. The runner only
    /// ever receives a host-owned, allowlisted accelerator.
    fn register(&self, accelerator: &str, action: &str) -> Result<()>;
}

/// Capability-scoped shortcut service bounded to an allowlist plus limits.
#[derive(Clone)]
pub struct ShortcutService {
    runner: Arc<dyn ShortcutRunner>,
    allowlist: Arc<ShortcutAllowlist>,
    limits: Arc<Limits>,
}

impl ShortcutService {
    pub fn new(
        runner: Arc<dyn ShortcutRunner>,
        allowlist: ShortcutAllowlist,
        limits: Limits,
    ) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Enable a global shortcut if its exact accelerator is on the allowlist.
    /// Returns the resolved (host-owned) accelerator/action for audit/trace.
    pub fn register(&self, accelerator: &str) -> Result<Value> {
        let binding = self.allowlist.resolve(accelerator).ok_or_else(|| {
            Error::scope_denied(format!(
                "kiri.shortcut.register: accelerator not on allowlist: {accelerator}"
            ))
        })?;
        // Bounded accelerator length so a template misconfiguration cannot flood.
        self.limits.check_bulk_object(binding.accelerator.len() as u64)?;
        self.runner.register(&binding.accelerator, &binding.action)?;
        Ok(serde_json::json!({
            "accelerator": binding.accelerator,
            "action": binding.action,
        }))
    }
}

/// Build the `kiri.shortcut.*` handlers bound to one ShortcutService.
pub fn shortcut_handlers(
    service: ShortcutService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(SHORTCUT_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::SHORTCUT_REGISTER,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let accelerator = p.get("accelerator").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::invalid_argument("kiri.shortcut.register requires string accelerator")
            })?;
            svc.register(accelerator)
        }) as Handler,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;

    struct StubShortcut {
        registered: std::sync::Arc<std::sync::Mutex<Vec<ShortcutRegistered>>>,
    }
    impl ShortcutRunner for StubShortcut {
        fn register(&self, accelerator: &str, action: &str) -> Result<()> {
            self.registered.lock().unwrap().push(ShortcutRegistered {
                accelerator: accelerator.to_string(),
                action: action.to_string(),
            });
            Ok(())
        }
    }

    fn allow() -> ShortcutAllowlist {
        ShortcutAllowlist::new(vec![
            ShortcutBinding { accelerator: "CmdOrCtrl+S".to_string(), action: "save".to_string() },
            ShortcutBinding {
                accelerator: "CmdOrCtrl+K".to_string(),
                action: "command-palette".to_string(),
            },
        ])
    }

    fn router() -> Router {
        let registered = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let svc =
            ShortcutService::new(Arc::new(StubShortcut { registered }), allow(), Limits::default());
        Router::new_with_limits(Limits::default()).with_shortcut(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(SHORTCUT_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_accelerator_registers() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::SHORTCUT_REGISTER,
            serde_json::json!({ "accelerator": "CmdOrCtrl+S" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["accelerator"], "CmdOrCtrl+S");
        assert_eq!(out["payload"]["action"], "save");
    }

    #[test]
    fn unknown_accelerator_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::SHORTCUT_REGISTER,
            serde_json::json!({ "accelerator": "CmdOrCtrl+Q" }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn frontend_supplied_action_is_ignored_host_mapping_wins() {
        // The frontend may only send the accelerator; the host owns the action.
        // A frontend-supplied "action" field is ignored and the host allowlist
        // mapping is authoritative, so it cannot redirect a known combo elsewhere.
        let r = router();
        let out = dispatch(
            &r,
            command_id::SHORTCUT_REGISTER,
            serde_json::json!({ "accelerator": "CmdOrCtrl+K", "action": "evil" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["accelerator"], "CmdOrCtrl+K");
        assert_eq!(out["payload"]["action"], "command-palette");
    }

    #[test]
    fn capability_denied_without_shortcut_bit() {
        let r = router();
        let granted = CapabilityBits::empty(); // no SHORTCUT bit
        let req = WireRequest::new(
            command_id::SHORTCUT_REGISTER,
            1,
            1,
            serde_json::json!({ "accelerator": "CmdOrCtrl+S" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
