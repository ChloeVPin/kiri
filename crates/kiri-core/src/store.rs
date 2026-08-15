//! Restricted scoped-store surface (`kiri.store`).
//!
//! This closes a Tauri plugin parity gap and converts Tauri's store risk into a
//! Kiri strength. Tauri's store plugin, when the capability is granted, lets the
//! frontend read/write the whole store with no namespace scoping. That is a
//! cross-feature data-leak surface: one frontend module can tamper with another
//! module's persisted state (e.g. a malicious widget rewriting `auth.session`).
//!
//! Kiri requires BOTH the `STORE` capability bit AND a host allowlist of scoped
//! namespaces. The frontend may only get/set keys inside an approved namespace
//! (the namespace owns the prefix boundary), and values are bounded by the shared
//! bulk-object limit. A granted capability with no matching namespace is refused,
//! so JavaScript can never reach a namespace the host has not explicitly approved.
//!
//! The actual persistence is behind the `StoreBackend` trait (mirrors
//! `HttpClient`/`ShortcutRunner`): the native host injects a real backend; tests
//! use an in-memory `StubStore` and assert namespace enforcement and capability
//! gating without touching disk.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.store.*` commands.
pub const STORE_CAPABILITY: u32 = 16;

/// One host-approved store namespace. `prefix` is the exact namespace root the
/// frontend may address (e.g. `app.prefs`). The backend confines all keys under
/// this namespace; the frontend cannot escape to another namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreNamespace {
    pub prefix: String,
}

/// Host-configured allowlist of store namespaces. Default-deny: a key resolves
/// only if its namespace is on the allowlist. The host owns the namespace set; the
/// frontend only picks an approved namespace and a key within it.
#[derive(Debug, Clone, Default)]
pub struct StoreAllowlist {
    namespaces: Vec<StoreNamespace>,
}

impl StoreAllowlist {
    pub fn new(namespaces: Vec<StoreNamespace>) -> Self {
        Self { namespaces }
    }

    fn resolve(&self, namespace: &str) -> Option<StoreNamespace> {
        self.namespaces.iter().find(|n| n.prefix == namespace).cloned()
    }

    pub fn namespaces(&self) -> &[StoreNamespace] {
        &self.namespaces
    }
}

/// Transport seam. The native host provides a real backend (file/sqlite/KV);
/// tests provide a stub. Kept trait-based so the logical protocol has zero
/// platform deps.
pub trait StoreBackend: Send + Sync {
    /// Read `key` within `namespace`. Returns the stored value, or `None` if absent.
    fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>>;
    /// Write `key` within `namespace` to `value`.
    fn set(&self, namespace: &str, key: &str, value: Value) -> Result<()>;
}

/// Capability-scoped store service bounded to a namespace allowlist plus limits
/// (value byte cap).
#[derive(Clone)]
pub struct StoreService {
    backend: Arc<dyn StoreBackend>,
    allowlist: Arc<StoreAllowlist>,
    limits: Arc<Limits>,
}

impl StoreService {
    pub fn new(backend: Arc<dyn StoreBackend>, allowlist: StoreAllowlist, limits: Limits) -> Self {
        Self { backend, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Read a key if its namespace is on the allowlist. Returns the stored value
    /// (or null) for audit/trace.
    pub fn get(&self, namespace: &str, key: &str) -> Result<Value> {
        self.allowlist.resolve(namespace).ok_or_else(|| {
            Error::scope_denied(format!("kiri.store.get: namespace not on allowlist: {namespace}"))
        })?;
        let v = self.backend.get(namespace, key)?;
        Ok(serde_json::json!({ "value": v }))
    }

    /// Write a key if its namespace is on the allowlist and the serialized value
    /// respects the bulk-object limit. Returns the stored value for trace.
    pub fn set(&self, namespace: &str, key: &str, value: Value) -> Result<Value> {
        self.allowlist.resolve(namespace).ok_or_else(|| {
            Error::scope_denied(format!("kiri.store.set: namespace not on allowlist: {namespace}"))
        })?;
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| Error::invalid_argument(format!("value not serializable: {e}")))?;
        self.limits.check_bulk_object(bytes.len() as u64)?;
        self.backend.set(namespace, key, value.clone())?;
        Ok(serde_json::json!({ "value": value }))
    }
}

/// Build the `kiri.store.*` handlers bound to one StoreService.
pub fn store_handlers(
    service: StoreService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(STORE_CAPABILITY);

    let get_svc = service.clone();
    let set_svc = service.clone();
    vec![
        (
            command_id::STORE_GET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let namespace = p.get("namespace").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.store.get requires string namespace")
                })?;
                let key = p
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::invalid_argument("kiri.store.get requires string key"))?;
                get_svc.get(namespace, key)
            }) as Handler,
        ),
        (
            command_id::STORE_SET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let namespace = p.get("namespace").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.store.set requires string namespace")
                })?;
                let key = p
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::invalid_argument("kiri.store.set requires string key"))?;
                let value = p
                    .get("value")
                    .cloned()
                    .ok_or_else(|| Error::invalid_argument("kiri.store.set requires value"))?;
                set_svc.set(namespace, key, value)
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
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct StubStore {
        data: Mutex<HashMap<(String, String), Value>>,
    }
    impl StoreBackend for StubStore {
        fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
            Ok(self.data.lock().unwrap().get(&(namespace.to_string(), key.to_string())).cloned())
        }
        fn set(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
            self.data.lock().unwrap().insert((namespace.to_string(), key.to_string()), value);
            Ok(())
        }
    }

    fn allow() -> StoreAllowlist {
        StoreAllowlist::new(vec![StoreNamespace { prefix: "app.prefs".to_string() }])
    }

    fn router() -> Router {
        let svc = StoreService::new(
            Arc::new(StubStore { data: Mutex::new(HashMap::new()) }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_store(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(STORE_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_namespace_set_then_get() {
        let r = router();
        let set = dispatch(
            &r,
            command_id::STORE_SET,
            serde_json::json!({ "namespace": "app.prefs", "key": "theme", "value": "dark" }),
        );
        assert!(set["error"].is_null(), "unexpected error: {set}");
        let got = dispatch(
            &r,
            command_id::STORE_GET,
            serde_json::json!({ "namespace": "app.prefs", "key": "theme" }),
        );
        assert!(got["error"].is_null());
        assert_eq!(got["payload"]["value"], "dark");
    }

    #[test]
    fn unknown_namespace_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::STORE_GET,
            serde_json::json!({ "namespace": "auth.session", "key": "token" }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn capability_denied_without_store_bit() {
        let r = router();
        let mut granted = CapabilityBits::empty(); // no STORE bit
        let req = WireRequest::new(
            command_id::STORE_GET,
            1,
            1,
            serde_json::json!({ "namespace": "app.prefs", "key": "theme" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
