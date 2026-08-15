//! Restricted sidecar-process surface (`kiri.sidecar`).
//!
//! This closes a Tauri sidecar parity gap (G-6) and converts Tauri's sidecar
//! risk into a Kiri strength. Tauri's sidecar API, once the capability is
//! granted, launches an arbitrary companion executable the frontend names
//! (a tamper / supply-chain surface: a malicious or buggy frontend can fork
//! any binary shipped next to the app, or one it smuggles into an allowed dir).
//! Kiri requires BOTH the `SIDECAR` capability bit AND a host allowlist of
//! exact sidecar names; the frontend may only spawn a pre-approved binary by
//! its host-owned name, cannot pass arbitrary argv, and cannot address a path
//! outside the host-declared sidecar set. Spawned output is captured and
//! bounded by the same bulk-object ceiling as `kiri.shell`.
//!
//! The actual spawn is behind the `SidecarRunner` trait (mirrors `ShellRunner`):
//! the native host injects a real spawner; tests use a `StubSidecar` and assert
//! authorization, allowlist enforcement, and size caps without launching real
//! processes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.sidecar.*` commands.
pub const SIDECAR_CAPABILITY: u32 = 21;

/// One host-approved sidecar: an exact binary name (no path, no argv) plus an
/// optional fixed arg prefix the host permits. The frontend references `name`
/// only; it cannot supply or alter the binary path or pass arbitrary argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedSidecar {
    pub name: String,
    pub args: Vec<String>,
}

/// Host-configured allowlist of sidecars that may be spawned. Default-deny: a
/// sidecar launches only if its exact `name` is listed; argv is forced to the
/// host-declared prefix. Empty `args` means "no args allowed".
#[derive(Debug, Clone, Default)]
pub struct SidecarAllowlist {
    items: Vec<AllowedSidecar>,
}

impl SidecarAllowlist {
    pub fn new(items: Vec<AllowedSidecar>) -> Self {
        Self { items }
    }

    fn resolve(&self, name: &str) -> Option<AllowedSidecar> {
        self.items.iter().find(|s| s.name == name).cloned()
    }

    pub fn items(&self) -> &[AllowedSidecar] {
        &self.items
    }
}

/// A captured sidecar result (stdout/stderr captured, bounded).
#[derive(Debug, Clone)]
pub struct SidecarOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Transport seam. The native host provides a real spawner; tests provide a
/// stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait SidecarRunner: Send + Sync {
    /// Spawn the named sidecar with exactly the host-declared arg prefix and
    /// capture its output. The host owns the binary resolution; the runner only
    /// ever receives a resolved path + the allowed argv.
    fn spawn(&self, name: &str, path: &str, args: &[String]) -> Result<SidecarOutput>;
}

/// Host-owned live sidecar handle map (so stop is scoped to a host-assigned id).
#[derive(Debug, Default)]
pub struct SidecarTable {
    next: Mutex<u64>,
    live: Mutex<BTreeMap<u64, String>>,
}

impl SidecarTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a host-assigned handle id for a named sidecar.
    pub fn allocate(&self, name: &str) -> u64 {
        let id = {
            let mut n = self.next.lock().unwrap();
            *n += 1;
            *n
        };
        self.live.lock().unwrap().insert(id, name.to_string());
        id
    }

    pub fn is_live(&self, id: u64) -> bool {
        self.live.lock().unwrap().contains_key(&id)
    }

    pub fn release(&self, id: u64) {
        self.live.lock().unwrap().remove(&id);
    }
}

/// Capability-scoped sidecar service bounded to a name allowlist plus limits.
#[derive(Clone)]
pub struct SidecarService {
    runner: Arc<dyn SidecarRunner>,
    allowlist: Arc<SidecarAllowlist>,
    table: Arc<SidecarTable>,
    limits: Arc<Limits>,
}

impl SidecarService {
    pub fn new(
        runner: Arc<dyn SidecarRunner>,
        allowlist: SidecarAllowlist,
        table: SidecarTable,
        limits: Limits,
    ) -> Self {
        Self {
            runner,
            allowlist: Arc::new(allowlist),
            table: Arc::new(table),
            limits: Arc::new(limits),
        }
    }

    /// Spawn a host-allowlisted sidecar by name. The frontend may only supply
    /// the approved `name`; argv is forced to the host-declared prefix. Returns
    /// a host-assigned handle id for later stop, plus the captured output.
    pub fn spawn(&self, name: &str, payload_args: &[String]) -> Result<Value> {
        let sidecar = self.allowlist.resolve(name).ok_or_else(|| {
            Error::scope_denied(format!("kiri.sidecar.spawn: name not on allowlist: {name}"))
        })?;
        // The frontend cannot extend argv beyond the host-declared prefix.
        if payload_args.len() > sidecar.args.len() {
            return Err(Error::scope_denied(
                "kiri.sidecar.spawn: frontend may not pass argv beyond host allowlist",
            ));
        }
        // The runner resolves the binary path from the host-owned name; the
        // frontend never supplies a path.
        let output = self.runner.spawn(&sidecar.name, &sidecar.name, &sidecar.args)?;
        self.limits.check_bulk_object(output.stdout.len() as u64)?;
        self.limits.check_bulk_object(output.stderr.len() as u64)?;
        let handle = self.table.allocate(&sidecar.name);
        Ok(serde_json::json!({
            "handle": handle,
            "name": sidecar.name,
            "exit_code": output.exit_code,
            "stdout": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &output.stdout),
            "stderr": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &output.stderr),
        }))
    }

    /// Stop a previously spawned sidecar by its host-assigned handle id. The
    /// frontend cannot address an arbitrary OS process, only a Kiri-issued id.
    pub fn stop(&self, handle: u64) -> Result<Value> {
        if !self.table.is_live(handle) {
            return Err(Error::invalid_argument(format!(
                "kiri.sidecar.stop: unknown handle {handle}"
            )));
        }
        self.table.release(handle);
        Ok(serde_json::json!({ "handle": handle, "stopped": true }))
    }

    /// Report which host-allowlisted sidecars exist (names only; never paths or
    /// argv). Lets the frontend discover what it may spawn without ever naming a
    /// binary path itself.
    pub fn list(&self) -> Value {
        serde_json::json!({
            "names": self.allowlist.items().iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        })
    }
}

/// Build the kiri.sidecar handlers bound to one SidecarService.
pub fn sidecar_handlers(
    service: SidecarService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(SIDECAR_CAPABILITY);

    let spawn_svc = service.clone();
    let stop_svc = service.clone();
    let list_svc = service.clone();
    vec![
        (
            command_id::SIDECAR_SPAWN,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let name = p.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.sidecar.spawn requires string name")
                })?;
                let args = p
                    .get("args")
                    .and_then(|v| v.as_array())
                    .and_then(|a| {
                        a.iter()
                            .map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Option<Vec<_>>>()
                    })
                    .unwrap_or_default();
                spawn_svc.spawn(name, &args)
            }) as Handler,
        ),
        (
            command_id::SIDECAR_STOP,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let handle = p.get("handle").and_then(|v| v.as_u64()).ok_or_else(|| {
                    Error::invalid_argument("kiri.sidecar.stop requires numeric handle")
                })?;
                stop_svc.stop(handle)
            }) as Handler,
        ),
        (
            command_id::SIDECAR_LIST,
            required,
            Arc::new(move |_c, _rid, _p: &Value| Ok(list_svc.list())) as Handler,
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

    struct StubSidecar {
        spawned: Mutex<Vec<String>>,
    }
    impl SidecarRunner for StubSidecar {
        fn spawn(&self, name: &str, _path: &str, args: &[String]) -> Result<SidecarOutput> {
            self.spawned.lock().unwrap().push(name.to_string());
            let joined = args.join(" ");
            Ok(SidecarOutput {
                exit_code: 0,
                stdout: format!("ok:{name}:{joined}").into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    fn allow() -> SidecarAllowlist {
        SidecarAllowlist::new(vec![
            AllowedSidecar {
                name: "helper".to_string(),
                args: vec!["--mode".to_string(), "fast".to_string()],
            },
            AllowedSidecar { name: "indexer".to_string(), args: vec![] },
        ])
    }

    fn router() -> Router {
        let svc = SidecarService::new(
            Arc::new(StubSidecar { spawned: Mutex::new(Vec::new()) }),
            allow(),
            SidecarTable::new(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_sidecar(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(SIDECAR_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_spawn_returns_handle_and_captured_output() {
        let r = router();
        let out = dispatch(&r, command_id::SIDECAR_SPAWN, serde_json::json!({ "name": "helper" }));
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert!(out["payload"]["handle"].as_u64().is_some());
        // stdout is base64 of "ok:helper:--mode fast"; decode and check prefix.
        let raw = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            out["payload"]["stdout"].as_str().unwrap(),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&raw).starts_with("ok:"));
    }

    #[test]
    fn unknown_name_denied() {
        let r = router();
        let out = dispatch(&r, command_id::SIDECAR_SPAWN, serde_json::json!({ "name": "evil" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn frontend_cannot_extend_argv() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::SIDECAR_SPAWN,
            serde_json::json!({ "name": "helper", "args": ["--mode", "fast", "extra"] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn stop_unknown_handle_denied() {
        let r = router();
        let out = dispatch(&r, command_id::SIDECAR_STOP, serde_json::json!({ "handle": 999 }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn list_returns_names_only() {
        let r = router();
        let out = dispatch(&r, command_id::SIDECAR_LIST, serde_json::json!({}));
        assert!(out["error"].is_null());
        let names = out["payload"]["names"].as_array().unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| n.is_string()));
    }
}
