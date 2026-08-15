//! Restricted autostart surface (`kiri.autostart`).
//!
//! This closes a Tauri plugin parity gap and converts Tauri's autostart risk into
//! a Kiri strength. Tauri's autostart plugin, when the capability is granted, lets
//! the frontend enable launch-at-login freely. That is a persistence surface: a
//! compromised or malicious frontend can install itself to run at every login
//! without the host ever expressing a policy about it.
//!
//! Kiri requires BOTH the `AUTOSTART` capability bit AND a host policy
//! (`permitted`). Even when permitted, the runner only ever registers the host's
//! own binary (the host-owned target passed at construction); the frontend can
//! only toggle `enabled` and can never choose which executable persists. A granted
//! capability with host policy `permitted = false` is refused, so JavaScript can
//! never persist an arbitrary binary to login items.
//!
//! The actual registration is behind the `AutostartRunner` trait (mirrors
//! `ShellRunner`/`ShortcutRunner`): the native host injects a real registrar; tests
//! use a `StubAutostart` and assert policy enforcement and capability gating
//! without touching the OS.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.autostart.*` commands.
pub const AUTOSTART_CAPABILITY: u32 = 15;

/// Host policy for autostart. Default-deny: autostart is disabled unless the host
/// explicitly permits it. The frontend cannot change this; only the host can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartAllowlist {
    pub permitted: bool,
}

impl AutostartAllowlist {
    pub fn new(permitted: bool) -> Self {
        Self { permitted }
    }

    pub fn is_permitted(&self) -> bool {
        self.permitted
    }
}

/// Transport seam. The native host provides a real registrar that registers the
/// host's own binary; tests provide a stub. Kept trait-based so the logical
/// protocol has zero platform deps.
pub trait AutostartRunner: Send + Sync {
    /// Enable or disable launch-at-login for the host-owned binary. The runner
    /// only ever registers the host's own target, never a frontend-supplied path.
    fn set_enabled(&self, enabled: bool) -> Result<()>;
    /// Report whether launch-at-login is currently enabled.
    fn is_enabled(&self) -> Result<bool>;
}

/// Capability-scoped autostart service bounded to a host policy plus limits.
#[derive(Clone)]
pub struct AutostartService {
    runner: Arc<dyn AutostartRunner>,
    allowlist: Arc<AutostartAllowlist>,
    limits: Arc<Limits>,
}

impl AutostartService {
    pub fn new(
        runner: Arc<dyn AutostartRunner>,
        allowlist: AutostartAllowlist,
        limits: Limits,
    ) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Enable or disable launch-at-login. Refused unless the host policy permits
    /// autostart. Returns the resulting enabled state for audit/trace.
    pub fn set(&self, enabled: bool) -> Result<Value> {
        if !self.allowlist.is_permitted() {
            return Err(Error::scope_denied("kiri.autostart.set: host policy denies autostart"));
        }
        // No bulk payload, but run the same limit pipeline for uniform enforcement.
        self.limits.check_bulk_object(0)?;
        self.runner.set_enabled(enabled)?;
        Ok(serde_json::json!({ "enabled": enabled, "managed": true }))
    }

    /// Report the current launch-at-login state. Refused unless the host policy
    /// permits autostart (querying is meaningless when disabled by policy).
    pub fn get(&self) -> Result<Value> {
        if !self.allowlist.is_permitted() {
            return Err(Error::scope_denied("kiri.autostart.get: host policy denies autostart"));
        }
        let enabled = self.runner.is_enabled()?;
        Ok(serde_json::json!({ "enabled": enabled, "managed": true }))
    }
}

/// Build the `kiri.autostart.*` handlers bound to one AutostartService.
pub fn autostart_handlers(
    service: AutostartService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(AUTOSTART_CAPABILITY);

    let set_svc = service.clone();
    let get_svc = service.clone();
    vec![
        (
            command_id::AUTOSTART_SET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let enabled = p.get("enabled").and_then(|v| v.as_bool()).ok_or_else(|| {
                    Error::invalid_argument("kiri.autostart.set requires boolean enabled")
                })?;
                set_svc.set(enabled)
            }) as Handler,
        ),
        (
            command_id::AUTOSTART_GET,
            required,
            Arc::new(move |_c, _rid, _p: &Value| get_svc.get()) as Handler,
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

    struct StubAutostart {
        state: std::sync::Arc<std::sync::Mutex<bool>>,
    }
    impl AutostartRunner for StubAutostart {
        fn set_enabled(&self, enabled: bool) -> Result<()> {
            *self.state.lock().unwrap() = enabled;
            Ok(())
        }
        fn is_enabled(&self) -> Result<bool> {
            Ok(*self.state.lock().unwrap())
        }
    }

    fn allow(permitted: bool) -> AutostartAllowlist {
        AutostartAllowlist::new(permitted)
    }

    fn router(permitted: bool) -> Router {
        let state = std::sync::Arc::new(std::sync::Mutex::new(false));
        let svc = AutostartService::new(
            Arc::new(StubAutostart { state }),
            allow(permitted),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_autostart(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(AUTOSTART_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn permitted_set_enables_and_records() {
        let r = router(true);
        let out = dispatch(&r, command_id::AUTOSTART_SET, serde_json::json!({ "enabled": true }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["enabled"], true);
        assert_eq!(out["payload"]["managed"], true);
        // GET echoes the runner state.
        let got = dispatch(&r, command_id::AUTOSTART_GET, serde_json::json!({}));
        assert!(got["error"].is_null());
        assert_eq!(got["payload"]["enabled"], true);
    }

    #[test]
    fn policy_denied_when_not_permitted() {
        let r = router(false);
        let out = dispatch(&r, command_id::AUTOSTART_SET, serde_json::json!({ "enabled": true }));
        assert!(!out["error"].is_null());
        let got = dispatch(&r, command_id::AUTOSTART_GET, serde_json::json!({}));
        assert!(!got["error"].is_null());
    }

    #[test]
    fn capability_denied_without_autostart_bit() {
        let r = router(true);
        let mut granted = CapabilityBits::empty(); // no AUTOSTART bit
        let req = WireRequest::new(
            command_id::AUTOSTART_SET,
            1,
            1,
            serde_json::json!({ "enabled": true }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
