//! Restricted, host-owned configuration surface (`kiri.config`).
//!
//! This converts a Tauri weakness into a Kiri strength (audit item 17). Tauri
//! exposes its entire `tauri.conf.json` object to the frontend by default
//! (`getConfig()` returns the full config, including fields like
//! `bundle`, `updater.endpoints`, `app.windows`, and plugin settings) — an
//! information-leak: any granted frontend can read host-intended build/runtime
//! metadata it was never meant to see. Kiri requires BOTH the `CONFIG`
//! capability bit AND a host allowlist of exact config key paths; the frontend
//! may only read pre-approved keys whose names are host-owned. A granted
//! capability addressing an unknown key is refused, so JavaScript can never
//! read arbitrary host config. The values are behind a `ConfigBackend` trait
//! (mirrors `EventBusBackend`): the native host injects the real config; tests
//! use a `StubConfig` and assert authorization and key-allowlist enforcement
//! headlessly.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.config.*` commands. Reuses the shared `CONFIG` bit
/// (22) so it stays in lockstep with `capability_bit::CONFIG` and `for_command`.
pub const CONFIG_CAPABILITY: u32 = crate::dispatch::capability_bit::CONFIG;

/// One host-approved config key path. The frontend references `key` only; it
/// cannot invent a key path. The host owns the set of readable keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedConfigKey {
    pub key: String,
}

/// Host-configured allowlist of config keys. Default-deny: a read succeeds only
/// if the exact key path is listed. The host owns the key namespace; the
/// frontend only picks a known key.
#[derive(Debug, Clone, Default)]
pub struct ConfigAllowlist {
    keys: Vec<AllowedConfigKey>,
}

impl ConfigAllowlist {
    pub fn new(keys: Vec<AllowedConfigKey>) -> Self {
        Self { keys }
    }

    fn allows(&self, key: &str) -> bool {
        self.keys.iter().any(|k| k.key == key)
    }

    pub fn keys(&self) -> &[AllowedConfigKey] {
        &self.keys
    }
}

/// Transport seam. The native host provides the real config; tests provide a
/// stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait ConfigBackend: Send + Sync {
    /// Return the host-owned value for `key`, or `None` if the backend does not
    /// define it (distinct from allowlist denial, which is enforced earlier).
    fn get(&self, key: &str) -> Option<Value>;
}

/// Capability-scoped config service bounded to a key allowlist plus limits.
#[derive(Clone)]
pub struct ConfigService {
    backend: Arc<dyn ConfigBackend>,
    allowlist: Arc<ConfigAllowlist>,
    limits: Arc<Limits>,
}

impl ConfigService {
    pub fn new(
        backend: Arc<dyn ConfigBackend>,
        allowlist: ConfigAllowlist,
        limits: Limits,
    ) -> Self {
        Self { backend, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Read a host-allowlisted config key. The frontend may only name a
    /// pre-approved key; an unknown key is refused. The value is bounded by the
    /// shared bulk-object ceiling so a large config blob cannot exhaust memory.
    pub fn read(&self, key: &str) -> Result<Value> {
        if !self.allowlist.allows(key) {
            return Err(Error::scope_denied(format!(
                "kiri.config.get: key not on allowlist: {key}"
            )));
        }
        let value = self.backend.get(key).unwrap_or(Value::Null);
        let serialized = serde_json::to_vec(&value).map_err(|e| {
            Error::invalid_argument(format!("kiri.config.get: value not serializable: {e}"))
        })?;
        self.limits.check_bulk_object(serialized.len() as u64)?;
        Ok(serde_json::json!({ "key": key, "value": value }))
    }

    /// Report which host-allowlisted keys exist (paths only). Lets the frontend
    /// discover what it may read without ever naming an arbitrary key itself.
    pub fn list(&self) -> Value {
        serde_json::json!({
            "keys": self.allowlist.keys().iter().map(|k| k.key.clone()).collect::<Vec<_>>(),
        })
    }
}

/// Build the kiri.config handlers bound to one ConfigService.
pub fn config_handlers(
    service: ConfigService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(CONFIG_CAPABILITY);

    let read_svc = service.clone();
    let list_svc = service.clone();
    vec![
        (
            command_id::CONFIG_GET,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let key = p.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.config.get requires string key")
                })?;
                read_svc.read(key)
            }) as Handler,
        ),
        (
            command_id::CONFIG_KEYS,
            required,
            Arc::new(move |_c, _rid, _p: &Value| Ok(list_svc.list())) as Handler,
        ),
    ]
}

/// Bridge a simple in-process config map to the restricted `ConfigBackend`
/// trait so the runtime can reuse one owned map for both the host and the
/// allowlisted audit-17 surface.
pub struct MapConfigBackend(std::collections::HashMap<String, Value>);

impl MapConfigBackend {
    pub fn new(map: std::collections::HashMap<String, Value>) -> Self {
        Self(map)
    }
}

impl ConfigBackend for MapConfigBackend {
    fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).cloned()
    }
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

    struct StubConfig {
        map: Mutex<std::collections::HashMap<String, Value>>,
    }
    impl ConfigBackend for StubConfig {
        fn get(&self, key: &str) -> Option<Value> {
            self.map.lock().unwrap().get(key).cloned()
        }
    }

    fn allow() -> ConfigAllowlist {
        ConfigAllowlist::new(vec![
            AllowedConfigKey { key: "app.name".to_string() },
            AllowedConfigKey { key: "window.theme".to_string() },
        ])
    }

    fn router() -> Router {
        let mut map = std::collections::HashMap::new();
        map.insert("app.name".to_string(), Value::String("Kiri".to_string()));
        map.insert("window.theme".to_string(), Value::String("system".to_string()));
        map.insert("bundle.private".to_string(), Value::String("secret".to_string()));
        let svc = ConfigService::new(
            Arc::new(StubConfig { map: Mutex::new(map) }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_config(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(CONFIG_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_get_returns_value() {
        let r = router();
        let out = dispatch(&r, command_id::CONFIG_GET, serde_json::json!({ "key": "app.name" }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["value"], "Kiri");
    }

    #[test]
    fn unknown_key_get_denied() {
        // bundle.private exists in the backend but is NOT on the allowlist.
        let r = router();
        let out =
            dispatch(&r, command_id::CONFIG_GET, serde_json::json!({ "key": "bundle.private" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn unknown_key_path_denied() {
        let r = router();
        let out = dispatch(&r, command_id::CONFIG_GET, serde_json::json!({ "key": "evil" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn list_returns_key_paths_only() {
        let r = router();
        let out = dispatch(&r, command_id::CONFIG_KEYS, serde_json::json!({}));
        assert!(out["error"].is_null());
        let keys = out["payload"]["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(!keys.iter().any(|k| k.as_str() == Some("bundle.private")));
    }
}
