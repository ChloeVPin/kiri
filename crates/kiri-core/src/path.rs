//! Capability-gated path/os helper surface (kiri.path.* / kiri.os.*).
//!
//! This closes part of the Tauri path/os plugin parity gap (audit item 2) and
//! exceeds it on the security axis: every operation is authorized by the
//! central capability authority (bit PATH) and routed through this service, so
//! JavaScript cannot reach env vars or filesystem-root queries except via the
//! explicitly granted helpers. Pure string/path math (dirname, basename,
//! extname, stem, join, isAbsolute) plus read-only OS directory discovery
//! (home/temp/app config/data/cache/document/app dir) backed by env vars and
//! std::env::temp_dir, so the entire surface is exercisable headlessly with no
//! WebView and no real filesystem mutation.

use serde_json::Value;
use std::sync::Arc;

use crate::error::Result;

/// Authorizes the kiri.path.* and kiri.os.* commands.
pub const PATH_CAPABILITY: u32 = 9;

/// Host-observable OS directory facts. Resolved from the environment at call
/// time so the values reflect the real running host on every platform.
#[derive(Debug, Clone, Default)]
pub struct PathState {
    pub home_dir: Option<String>,
    pub temp_dir: Option<String>,
}

impl PathState {
    pub fn new() -> Self {
        Self {
            home_dir: std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()),
            temp_dir: std::env::temp_dir().to_str().map(|s| s.to_string()),
        }
    }
}

/// Pure path/os helpers. No capability state is mutated by the pure ops; the
/// OS directory queries read the environment through PathState so tests can
/// inject deterministic values without touching the real host.
#[derive(Clone)]
pub struct PathService {
    state: PathState,
}

impl PathService {
    pub fn new(state: PathState) -> Self {
        Self { state }
    }

    /// Directory portion of a path (everything before the final separator), or
    /// "." when there is no separator.
    pub fn dirname(&self, path: &str) -> Result<Value> {
        let p = std::path::Path::new(path);
        let dir = p.parent().map(|d| d.to_string_lossy().to_string());
        let dir = dir.unwrap_or_else(|| ".".to_string());
        Ok(serde_json::json!({ "path": path, "dirname": dir }))
    }

    /// Final path component, or "" when the path ends in a separator.
    pub fn basename(&self, path: &str) -> Result<Value> {
        let p = std::path::Path::new(path);
        let base = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        Ok(serde_json::json!({ "path": path, "basename": base }))
    }

    /// Extension of the final component without the dot, or "" when none.
    pub fn extname(&self, path: &str) -> Result<Value> {
        let p = std::path::Path::new(path);
        let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        Ok(serde_json::json!({ "path": path, "extname": ext }))
    }

    /// Final component without its extension (or the component when none).
    pub fn stem(&self, path: &str) -> Result<Value> {
        let p = std::path::Path::new(path);
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| {
            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        });
        Ok(serde_json::json!({ "path": path, "stem": stem }))
    }

    /// Join a base path with the provided segments into a single OS-normalized
    /// path. Segments is a JSON array of strings.
    pub fn join(&self, base: &str, segments: &[String]) -> Result<Value> {
        let mut p = std::path::PathBuf::from(base);
        for seg in segments {
            p.push(seg);
        }
        Ok(serde_json::json!({ "path": p.to_string_lossy().to_string() }))
    }

    /// Whether the path is absolute on the host OS.
    pub fn is_absolute(&self, path: &str) -> Result<Value> {
        let p = std::path::Path::new(path);
        Ok(serde_json::json!({ "path": path, "isAbsolute": p.is_absolute() }))
    }

    fn resolve(&self, kind: &str) -> Value {
        match kind {
            "home" => self
                .state
                .home_dir
                .clone()
                .map(|d| serde_json::json!({ "dir": d }))
                .unwrap_or_else(|| serde_json::json!({ "dir": Value::Null })),
            "temp" => self
                .state
                .temp_dir
                .clone()
                .map(|d| serde_json::json!({ "dir": d }))
                .unwrap_or_else(|| serde_json::json!({ "dir": Value::Null })),
            "appConfig" | "appData" | "appCache" | "document" | "app" => {
                let home = self.state.home_dir.clone();
                let resolved = home.map(|h| match kind {
                    "appConfig" => {
                        std::path::Path::new(&h).join("Library").join("Application Support")
                    }
                    "appData" => {
                        std::path::Path::new(&h).join("Library").join("Application Support")
                    }
                    "appCache" => std::path::Path::new(&h).join("Library").join("Caches"),
                    "document" => std::path::Path::new(&h).join("Documents"),
                    _ => std::path::Path::new(&h).join(".kiri"),
                });
                match resolved {
                    Some(p) => serde_json::json!({ "dir": p.to_string_lossy().to_string() }),
                    None => serde_json::json!({ "dir": Value::Null }),
                }
            }
            _ => serde_json::json!({ "dir": Value::Null }),
        }
    }

    pub fn home_dir(&self) -> Result<Value> {
        Ok(self.resolve("home"))
    }
    pub fn temp_dir(&self) -> Result<Value> {
        Ok(self.resolve("temp"))
    }
    pub fn app_config_dir(&self) -> Result<Value> {
        Ok(self.resolve("appConfig"))
    }
    pub fn app_data_dir(&self) -> Result<Value> {
        Ok(self.resolve("appData"))
    }
    pub fn app_cache_dir(&self) -> Result<Value> {
        Ok(self.resolve("appCache"))
    }
    pub fn document_dir(&self) -> Result<Value> {
        Ok(self.resolve("document"))
    }
    pub fn app_dir(&self) -> Result<Value> {
        Ok(self.resolve("app"))
    }
}

/// Build the kiri.path.* and kiri.os.* handlers bound to one PathService.
/// Reused by the router builder and any plugin path so authority is identical.
pub fn path_handlers(
    service: PathService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(PATH_CAPABILITY);

    let d = service.clone();
    let b = service.clone();
    let e = service.clone();
    let s = service.clone();
    let j = service.clone();
    let ia = service.clone();
    let home = service.clone();
    let temp = service.clone();
    let cfg = service.clone();
    let data = service.clone();
    let cache = service.clone();
    let doc = service.clone();
    let app = service.clone();

    vec![
        (
            command_id::PATH_DIRNAME,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument("kiri.path.dirname requires string path")
                })?;
                d.dirname(path)
            }) as Handler,
        ),
        (
            command_id::PATH_BASENAME,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument("kiri.path.basename requires string path")
                })?;
                b.basename(path)
            }) as Handler,
        ),
        (
            command_id::PATH_EXTNAME,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument("kiri.path.extname requires string path")
                })?;
                e.extname(path)
            }) as Handler,
        ),
        (
            command_id::PATH_STEM,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument("kiri.path.stem requires string path")
                })?;
                s.stem(path)
            }) as Handler,
        ),
        (
            command_id::PATH_JOIN,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let base = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument("kiri.path.join requires string path")
                })?;
                let segs = p
                    .get("segments")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                j.join(base, &segs)
            }) as Handler,
        ),
        (
            command_id::PATH_IS_ABSOLUTE,
            required,
            Arc::new(move |_c, _rid, p: &Value| {
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::Error::invalid_argument(
                        "kiri.path.isAbsolute requires string path",
                    )
                })?;
                ia.is_absolute(path)
            }) as Handler,
        ),
        (
            command_id::OS_HOME_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| home.home_dir()) as Handler,
        ),
        (
            command_id::OS_TEMP_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| temp.temp_dir()) as Handler,
        ),
        (
            command_id::OS_APP_CONFIG_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| cfg.app_config_dir()) as Handler,
        ),
        (
            command_id::OS_APP_DATA_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| data.app_data_dir()) as Handler,
        ),
        (
            command_id::OS_APP_CACHE_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| cache.app_cache_dir()) as Handler,
        ),
        (
            command_id::OS_DOCUMENT_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| doc.document_dir()) as Handler,
        ),
        (
            command_id::OS_APP_DIR,
            required,
            Arc::new(move |_c, _rid, _p: &Value| app.app_dir()) as Handler,
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

    fn router() -> Router {
        let state = PathState {
            home_dir: Some("/Users/kiri".to_string()),
            temp_dir: Some("/tmp".to_string()),
        };
        Router::new_with_limits(crate::limits::Limits::default()).with_path(PathService::new(state))
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(PATH_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn dirname_basename_extname_stem() {
        let r = router();
        let out =
            dispatch(&r, command_id::PATH_DIRNAME, serde_json::json!({ "path": "/a/b/c.txt" }));
        assert_eq!(out["payload"]["dirname"], "/a/b");
        let out =
            dispatch(&r, command_id::PATH_BASENAME, serde_json::json!({ "path": "/a/b/c.txt" }));
        assert_eq!(out["payload"]["basename"], "c.txt");
        let out =
            dispatch(&r, command_id::PATH_EXTNAME, serde_json::json!({ "path": "/a/b/c.txt" }));
        assert_eq!(out["payload"]["extname"], "txt");
        let out = dispatch(&r, command_id::PATH_STEM, serde_json::json!({ "path": "/a/b/c.txt" }));
        assert_eq!(out["payload"]["stem"], "c");
    }

    #[test]
    fn join_is_os_normalized() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::PATH_JOIN,
            serde_json::json!({ "path": "/a", "segments": ["b", "c"] }),
        );
        assert_eq!(out["payload"]["path"], if cfg!(windows) { r"\a\b\c" } else { "/a/b/c" });
    }

    #[test]
    fn is_absolute_reports_correctly() {
        let r = router();
        let abs = dispatch(&r, command_id::PATH_IS_ABSOLUTE, serde_json::json!({ "path": "/a/b" }));
        assert_eq!(abs["payload"]["isAbsolute"], !cfg!(windows) || true);
        let rel = dispatch(&r, command_id::PATH_IS_ABSOLUTE, serde_json::json!({ "path": "a/b" }));
        assert_eq!(rel["payload"]["isAbsolute"], false);
    }

    #[test]
    fn os_dirs_resolve_from_state() {
        let r = router();
        let home = dispatch(&r, command_id::OS_HOME_DIR, serde_json::json!({}));
        assert_eq!(home["payload"]["dir"], "/Users/kiri");
        let cfgd = dispatch(&r, command_id::OS_APP_CONFIG_DIR, serde_json::json!({}));
        assert!(cfgd["payload"]["dir"].as_str().unwrap().contains("Application Support"));
    }

    #[test]
    fn missing_path_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req =
            WireRequest::new(command_id::PATH_DIRNAME, 1, 1, serde_json::json!({ "path": "/x" }));
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }
}
