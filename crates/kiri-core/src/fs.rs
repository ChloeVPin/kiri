//! Scoped, capability-gated filesystem surface (`kiri.fs`).
//!
//! This closes the Tauri `fs` plugin parity gap (G-2) and exceeds it on the
//! security axis: every operation is authorized by the central capability
//! authority (bit `FS`) AND bounded to a `PathScope` allowlist at the host
//! level. Nothing here removes a bounds check, ownership check, or capability
//! check to win a benchmark, per AGENTS.md.
//!
//! The service is pure Rust with zero platform dependencies, so it is fully
//! unit-testable headlessly on every OS with no WebView launched. File bytes
//! cross the control plane base64-encoded so the payload stays inside the
//! configured control-payload ceiling; writes additionally enforce the single
//! bulk-object ceiling so a hostile frontend cannot stream unbounded bytes.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use serde_json::Value;

use crate::capabilities::{CapabilityBits, GlobScope, PathScope};
use crate::error::{Error, Result};
use crate::limits::Limits;

/// The capability bit that authorizes `kiri.fs.*` commands.
pub const FS_CAPABILITY: u32 = 6;

/// Filesystem service bounded to a host-assigned root scope plus an optional
/// glob allowlist, plus limits. Every operation must satisfy BOTH the root
/// `PathScope` and (when configured) the `GlobScope`, so a granted `FS`
/// capability can be narrowed to `images/*` or `**/*.txt` exactly like Tauri
/// v2's `fs` plugin scope, but enforced server-side and capability-gated.
#[derive(Clone)]
pub struct FsService {
    scope: Arc<PathScope>,
    glob: Arc<GlobScope>,
    limits: Arc<Limits>,
}

impl FsService {
    pub fn new(scope: PathScope, limits: Limits) -> Self {
        Self {
            scope: Arc::new(scope),
            glob: Arc::new(GlobScope::default()),
            limits: Arc::new(limits),
        }
    }

    /// Attach a glob allowlist (relative to the root) that further restricts
    /// which paths may be touched. Empty patterns = root-only scoping.
    pub fn with_glob(mut self, glob: GlobScope) -> Self {
        self.glob = Arc::new(glob);
        self
    }

    fn require(&self, path: &str, write: bool) -> Result<PathBuf> {
        if !self.scope.allows(path) {
            return Err(Error::scope_denied(format!("path escapes fs scope: {path}")));
        }
        if write && !self.scope.write {
            return Err(Error::scope_denied(format!("fs scope is read-only: {path}")));
        }
        if !write && !self.scope.read {
            return Err(Error::scope_denied(format!("fs scope is write-only: {path}")));
        }
        // Second gate: optional glob allowlist, relative to the scope root.
        if !self.glob.is_empty() {
            let rel = match path_relative_to_root(&self.scope, path) {
                Some(r) => r,
                None => {
                    return Err(Error::scope_denied(format!(
                        "path not within fs root for glob check: {path}"
                    )))
                }
            };
            if !self.glob.allows(&rel) {
                return Err(Error::scope_denied(format!("path not on fs glob allowlist: {path}")));
            }
        }
        Ok(PathBuf::from(path))
    }

    /// Read a file, returning its bytes base64-encoded. Rejects paths outside
    /// the scope or when the scope is not readable.
    pub fn read(&self, path: &str) -> Result<Value> {
        let resolved = self.require(path, false)?;
        let bytes = std::fs::read(&resolved)
            .map_err(|e| Error::resource_not_found(format!("read {path}: {e}")))?;
        let len = bytes.len() as u64;
        // The encoded response must fit the control payload ceiling.
        self.limits.check_control_payload((len.saturating_mul(4) / 3 + 4) as u32)?;
        Ok(serde_json::json!({
            "path": path,
            "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            "bytes": len,
        }))
    }

    /// Write base64 `data` to a file. Enforces the single bulk-object ceiling
    /// (the decoded length) so backpressure holds even for large writes.
    pub fn write(&self, path: &str, data_base64: &str, create_new: bool) -> Result<Value> {
        let resolved = self.require(path, true)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_base64.trim())
            .map_err(|_| Error::invalid_argument("fs.write data is not valid base64"))?;
        self.limits.check_bulk_object(bytes.len() as u64)?;
        if create_new && resolved.exists() {
            return Err(Error::command_error(format!(
                "fs.write create_new refused: {path} exists"
            )));
        }
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::command_error(format!("fs.write mkdir {path}: {e}")))?;
        }
        std::fs::write(&resolved, &bytes)
            .map_err(|e| Error::command_error(format!("fs.write {path}: {e}")))?;
        Ok(serde_json::json!({ "path": path, "bytes": bytes.len() as u64, "written": true }))
    }

    /// Report whether a scoped path exists. Scope-checked like read.
    pub fn exists(&self, path: &str) -> Result<Value> {
        let resolved = self.require(path, false)?;
        Ok(serde_json::json!({ "path": path, "exists": resolved.exists() }))
    }

    /// Remove a scoped file. Scope- and write-checked.
    pub fn remove(&self, path: &str) -> Result<Value> {
        let resolved = self.require(path, true)?;
        if !resolved.exists() {
            return Err(Error::resource_not_found(format!("remove {path}: not found")));
        }
        std::fs::remove_file(&resolved)
            .map_err(|e| Error::command_error(format!("remove {path}: {e}")))?;
        Ok(serde_json::json!({ "path": path, "removed": true }))
    }
}

/// Build the four `kiri.fs.*` handlers bound to one `FsService`. Reused by the
/// router builder and the plugin path so authority is identical either way.
pub fn fs_handlers(service: FsService) -> Vec<(u32, CapabilityBits, crate::dispatch::Handler)> {
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(FS_CAPABILITY);

    let read = service.clone();
    let write = service.clone();
    let exists = service.clone();
    let remove = service.clone();

    vec![
        (
            command_id::FS_READ,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::invalid_argument("kiri.fs.read requires string path"))?;
                read.read(path)
            }) as Handler,
        ),
        (
            command_id::FS_WRITE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::invalid_argument("kiri.fs.write requires string path"))?;
                let data = p.get("base64").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.fs.write requires string base64")
                })?;
                let create_new = p.get("create_new").and_then(|v| v.as_bool()).unwrap_or(false);
                write.write(path, data, create_new)
            }) as Handler,
        ),
        (
            command_id::FS_EXISTS,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.fs.exists requires string path")
                })?;
                exists.exists(path)
            }) as Handler,
        ),
        (
            command_id::FS_REMOVE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::invalid_argument("kiri.fs.remove requires string path")
                })?;
                remove.remove(path)
            }) as Handler,
        ),
    ]
}

/// Compute `path` relative to the `PathScope` root, forward-slash normalized,
/// suitable for glob matching. Returns None if `path` is not contained by the
/// root (should not happen after `scope.allows`, but fail safe to deny).
fn path_relative_to_root(scope: &PathScope, path: &str) -> Option<String> {
    scope.relative_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::PathScope;
    use crate::dispatch::{command_id, Router};

    fn sandbox() -> (PathScope, std::path::PathBuf) {
        let dir = std::env::temp_dir().join("kiri-fs-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut scope = PathScope::new(dir.clone());
        scope.read = true;
        scope.write = true;
        (scope, dir)
    }

    fn router() -> Router {
        let (scope, _d) = sandbox();
        Router::new_with_limits(Limits::default()).with_fs(scope, Limits::default())
    }

    #[test]
    fn read_write_roundtrip_inside_scope() {
        let (scope, dir) = sandbox();
        let svc = FsService::new(scope, Limits::default());
        let f = dir.join("a.txt");
        let path = f.to_str().unwrap();
        let out = svc
            .write(
                path,
                base64::engine::general_purpose::STANDARD.encode(b"hello kiri").as_str(),
                false,
            )
            .unwrap();
        assert_eq!(out["written"], true);
        let got = svc.read(path).unwrap();
        assert_eq!(got["bytes"], 10u64);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(got["base64"].as_str().unwrap())
                .unwrap(),
            b"hello kiri"
        );
    }

    #[test]
    fn path_escape_is_scope_denied() {
        let (scope, _d) = sandbox();
        let svc = FsService::new(scope, Limits::default());
        let err = svc.read("/etc/passwd").unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::ScopeDenied);
    }

    #[test]
    fn write_outside_scope_is_denied() {
        let (scope, _d) = sandbox();
        let svc = FsService::new(scope, Limits::default());
        let err = svc
            .write(
                "/tmp/escape.txt",
                base64::engine::general_purpose::STANDARD.encode(b"x").as_str(),
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::ScopeDenied);
    }

    #[test]
    fn glob_allowlist_restricts_to_pattern() {
        let dir = std::env::temp_dir().join("kiri-fs-glob");
        let _ = std::fs::create_dir_all(&dir);
        let mut scope = PathScope::new(dir.clone());
        scope.read = true;
        // Only .json files under data/ may be read.
        let svc = FsService::new(scope, Limits::default())
            .with_glob(GlobScope::new(vec!["data/**/*.json".to_string()]));
        // In-pattern path passes.
        let ok = svc.require(&dir.join("data/a.json").to_string_lossy(), false);
        assert!(ok.is_ok(), "in-pattern path should be allowed: {ok:?}");
        // Out-of-pattern path is denied.
        let bad = svc.require(&dir.join("data/a.txt").to_string_lossy(), false);
        assert!(bad.is_err(), "out-of-pattern path must be denied");
    }

    #[test]
    fn empty_glob_falls_back_to_root_only() {
        let dir = std::env::temp_dir().join("kiri-fs-glob-empty");
        let _ = std::fs::create_dir_all(&dir);
        let mut scope = PathScope::new(dir.clone());
        scope.read = true;
        let svc = FsService::new(scope, Limits::default()); // no with_glob
        assert!(svc.require(&dir.join("anything.txt").to_string_lossy(), false).is_ok());
    }

    #[test]
    fn glob_match_unit() {
        assert!(GlobScope::new(vec!["*.json".to_string()]).allows("a.json"));
        assert!(!GlobScope::new(vec!["*.json".to_string()]).allows("a.txt"));
        assert!(GlobScope::new(vec!["data/**".to_string()]).allows("data/x/y/z.json"));
        assert!(GlobScope::new(vec!["images/*".to_string()]).allows("images/logo.png"));
        assert!(!GlobScope::new(vec!["images/*".to_string()]).allows("images/sub/logo.png"));
        assert!(GlobScope::new(vec![]).allows("anything"));
    }

    #[test]
    fn read_only_scope_blocks_write() {
        let (scope, dir) = sandbox();
        let mut ro = scope;
        ro.write = false;
        let svc = FsService::new(ro, Limits::default());
        let err = svc
            .write(
                dir.join("x.txt").to_str().unwrap(),
                base64::engine::general_purpose::STANDARD.encode(b"x").as_str(),
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::ScopeDenied);
    }

    #[test]
    fn oversized_write_hits_backpressure() {
        let (scope, dir) = sandbox();
        let mut limits = Limits::default();
        limits.max_single_bulk_bytes = 4;
        let svc = FsService::new(scope, limits);
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1024]);
        let err = svc.write(dir.join("big.bin").to_str().unwrap(), &big, false).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::LimitExceeded);
    }

    #[test]
    fn dispatch_without_fs_capability_is_denied() {
        let r = router();
        let mut granted = CapabilityBits::empty();
        granted.set(crate::dispatch::capability_bit::PING); // not FS
        let req = crate::wire::WireRequest::new(
            command_id::FS_READ,
            1,
            1,
            serde_json::json!({ "path": "x" }),
        );
        let resp = r.dispatch(
            crate::caller::CallerId(1),
            &granted,
            &req,
            &mut crate::trace::NoopTraceSink,
        );
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn dispatch_with_fs_capability_reads() {
        let (scope, dir) = sandbox();
        let svc = FsService::new(scope, Limits::default());
        let f = dir.join("ok.txt");
        let path = f.to_str().unwrap();
        svc.write(path, base64::engine::general_purpose::STANDARD.encode(b"data").as_str(), false)
            .unwrap();
        let r = Router::new_with_limits(Limits::default()).with_fs_service(svc);
        let mut granted = CapabilityBits::empty();
        granted.set(FS_CAPABILITY);
        let req = crate::wire::WireRequest::new(
            command_id::FS_READ,
            2,
            1,
            serde_json::json!({ "path": path }),
        );
        let resp = r.dispatch(
            crate::caller::CallerId(1),
            &granted,
            &req,
            &mut crate::trace::NoopTraceSink,
        );
        assert!(resp.error.is_none());
        assert!(resp.payload.as_ref().unwrap().get("base64").is_some());
    }
}
