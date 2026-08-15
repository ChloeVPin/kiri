//! Restricted deep-link surface (`kiri.deeplink`).
//!
//! This closes a Tauri plugin parity gap and converts Tauri's deep-link risk into a
//! Kiri strength. Tauri's deep-link plugin, when the capability is granted, lets the
//! frontend register arbitrary URI schemes. That is a scheme-squatting / handler-hijack
//! surface: a malicious app can bind a scheme owned by another app (e.g. `zoom://`,
//! `ssh://`) and intercept launches meant for it.
//!
//! Kiri requires BOTH the `DEEPLINK` capability bit AND a host allowlist of exact
//! schemes. The frontend may only register a scheme the host has explicitly approved;
//! it cannot bind an arbitrary URI scheme. The runner only ever receives a host-owned,
//! allowlisted scheme, so JavaScript can never squat on another app's scheme. A granted
//! capability with no matching allowlist entry is refused.
//!
//! The actual registration is behind the `DeeplinkRunner` trait (mirrors
//! `ShortcutRunner`/`AutostartRunner`): the native host injects a real registrar; tests
//! use a `StubDeeplink` and assert scheme enforcement and capability gating without
//! touching the OS.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.deeplink.*` commands.
pub const DEEPLINK_CAPABILITY: u32 = 17;

/// One host-approved deep-link scheme (without the trailing `://`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeeplinkScheme {
    pub scheme: String,
}

/// Host-configured allowlist of schemes. Default-deny: a scheme registers only if
/// it is listed exactly. The host owns the scheme set; the frontend only picks an
/// approved scheme.
#[derive(Debug, Clone, Default)]
pub struct DeeplinkAllowlist {
    schemes: Vec<DeeplinkScheme>,
}

impl DeeplinkAllowlist {
    pub fn new(schemes: Vec<DeeplinkScheme>) -> Self {
        Self { schemes }
    }

    fn resolve(&self, scheme: &str) -> Option<DeeplinkScheme> {
        self.schemes.iter().find(|s| s.scheme == scheme).cloned()
    }

    pub fn schemes(&self) -> &[DeeplinkScheme] {
        &self.schemes
    }
}

/// A registered deep-link result, returned for audit/trace.
#[derive(Debug, Clone)]
pub struct DeeplinkRegistered {
    pub scheme: String,
}

/// Transport seam. The native host provides a real registrar; tests provide a stub.
/// Kept trait-based so the logical protocol has zero platform deps.
pub trait DeeplinkRunner: Send + Sync {
    /// Register the exact `scheme` for this app. The runner only ever receives a
    /// host-owned, allowlisted scheme.
    fn register(&self, scheme: &str) -> Result<()>;
}

/// Capability-scoped deep-link service bounded to a scheme allowlist plus limits.
#[derive(Clone)]
pub struct DeeplinkService {
    runner: Arc<dyn DeeplinkRunner>,
    allowlist: Arc<DeeplinkAllowlist>,
    limits: Arc<Limits>,
}

impl DeeplinkService {
    pub fn new(
        runner: Arc<dyn DeeplinkRunner>,
        allowlist: DeeplinkAllowlist,
        limits: Limits,
    ) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Register a deep-link scheme if it is on the allowlist. Returns the resolved
    /// (host-owned) scheme for audit/trace.
    pub fn register(&self, scheme: &str) -> Result<Value> {
        let binding = self.allowlist.resolve(scheme).ok_or_else(|| {
            Error::scope_denied(format!(
                "kiri.deeplink.register: scheme not on allowlist: {scheme}"
            ))
        })?;
        self.limits.check_bulk_object(binding.scheme.len() as u64)?;
        self.runner.register(&binding.scheme)?;
        Ok(serde_json::json!({ "scheme": binding.scheme }))
    }
}

/// Build the `kiri.deeplink.*` handlers bound to one DeeplinkService.
pub fn deeplink_handlers(
    service: DeeplinkService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(DEEPLINK_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::DEEPLINK_REGISTER,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let scheme = p.get("scheme").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::invalid_argument("kiri.deeplink.register requires string scheme")
            })?;
            svc.register(scheme)
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

    struct StubDeeplink {
        registered: std::sync::Arc<std::sync::Mutex<Vec<DeeplinkRegistered>>>,
    }
    impl DeeplinkRunner for StubDeeplink {
        fn register(&self, scheme: &str) -> Result<()> {
            self.registered.lock().unwrap().push(DeeplinkRegistered { scheme: scheme.to_string() });
            Ok(())
        }
    }

    fn allow() -> DeeplinkAllowlist {
        DeeplinkAllowlist::new(vec![DeeplinkScheme { scheme: "kiri-app".to_string() }])
    }

    fn router() -> Router {
        let registered = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let svc =
            DeeplinkService::new(Arc::new(StubDeeplink { registered }), allow(), Limits::default());
        Router::new_with_limits(Limits::default()).with_deeplink(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(DEEPLINK_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_scheme_registers() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DEEPLINK_REGISTER,
            serde_json::json!({ "scheme": "kiri-app" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["scheme"], "kiri-app");
    }

    #[test]
    fn unknown_scheme_is_denied() {
        let r = router();
        let out =
            dispatch(&r, command_id::DEEPLINK_REGISTER, serde_json::json!({ "scheme": "zoom" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn capability_denied_without_deeplink_bit() {
        let r = router();
        let granted = CapabilityBits::empty(); // no DEEPLINK bit
        let req = WireRequest::new(
            command_id::DEEPLINK_REGISTER,
            1,
            1,
            serde_json::json!({ "scheme": "kiri-app" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
