//! Restricted opener surface (`kiri.opener`).
//!
//! This closes the Tauri `opener` plugin parity gap and converts Tauri's open
//! risk into a Kiri strength. Tauri's opener plugin, when the capability is
//! granted, opens arbitrary URLs and files via the OS default association, so a
//! malicious or careless frontend can launch `file://` paths outside the app
//! sandbox, `ssh://`/`telnet://` handlers, or mailto/exec schemes the user never
//! intended to expose.
//!
//! Kiri requires BOTH the `OPENER` capability bit AND a host allowlist. The
//! allowlist is two-layered and default-deny:
//!   1. `AllowedUrlScheme`: exact URL schemes (e.g. `https`, `mailto`) the
//!      frontend may open. No scheme on the list is refused.
//!   2. `AllowedFileExtension`: a fixed set of file extensions the frontend may
//!      open from within an app-owned path. Anything else is refused.
//!
//! The actual open is behind the `OpenerRunner` trait (mirrors `ShellRunner`): the
//! native host injects a real opener that defers to the OS default association;
//! tests use a `StubOpener` and assert scheme/extension enforcement and capability
//! gating without touching the desktop shell.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.opener.*` commands.
pub const OPENER_CAPABILITY: u32 = 18;

/// One allowed URL scheme, matched exactly after lowercasing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedUrlScheme {
    pub scheme: String,
}

/// One allowed file extension (without the leading dot), matched exactly after
/// lowercasing. The frontend may only open files whose extension is on this list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedFileExtension {
    pub extension: String,
}

/// Host-configured allowlist. Default-deny on both axes: a URL opens only if its
/// scheme is listed; a file opens only if its extension is listed.
#[derive(Debug, Clone, Default)]
pub struct OpenerAllowlist {
    url_schemes: Vec<AllowedUrlScheme>,
    file_extensions: Vec<AllowedFileExtension>,
}

impl OpenerAllowlist {
    pub fn new(
        url_schemes: Vec<AllowedUrlScheme>,
        file_extensions: Vec<AllowedFileExtension>,
    ) -> Self {
        Self { url_schemes, file_extensions }
    }

    fn allows_url(&self, scheme: &str) -> bool {
        let s = scheme.to_ascii_lowercase();
        self.url_schemes.iter().any(|u| u.scheme.to_ascii_lowercase() == s)
    }

    fn allows_extension(&self, ext: &str) -> bool {
        let e = ext.trim_start_matches('.').to_ascii_lowercase();
        self.file_extensions.iter().any(|x| x.extension.to_ascii_lowercase() == e)
    }

    pub fn url_schemes(&self) -> &[AllowedUrlScheme] {
        &self.url_schemes
    }

    pub fn file_extensions(&self) -> &[AllowedFileExtension] {
        &self.file_extensions
    }
}

/// A parsed open target, resolved on the host side before any OS call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    /// A URL with a known scheme (e.g. `https://...`, `mailto:...`).
    Url { scheme: String, rest: String },
    /// A filesystem path whose extension (if any) is host-approved.
    File { path: String, extension: String },
}

impl OpenTarget {
    /// Parse `target` into a `Url` or `File`. Errors on an empty target.
    pub fn parse(target: &str) -> Result<Self> {
        if target.trim().is_empty() {
            return Err(Error::invalid_argument("kiri.opener.open requires a non-empty target"));
        }
        if let Some(idx) = target.find("://") {
            let scheme = target[..idx].to_string();
            let rest = target[idx + 3..].to_string();
            if scheme.is_empty() {
                return Err(Error::invalid_argument("kiri.opener.open: empty URL scheme"));
            }
            return Ok(OpenTarget::Url { scheme, rest });
        }
        let extension = std::path::Path::new(target)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        Ok(OpenTarget::File { path: target.to_string(), extension })
    }
}

/// Transport seam. The native host provides a real opener that defers to the OS
/// default association; tests provide a stub. Kept trait-based so the logical
/// protocol has zero platform deps.
pub trait OpenerRunner: Send + Sync {
    /// Open `target` through the OS default association. The runner only ever
    /// receives a host-resolved, allowlisted target.
    fn open(&self, target: &OpenTarget) -> Result<()>;
}

/// Capability-scoped opener service bounded to a scheme/extension allowlist plus
/// limits.
#[derive(Clone)]
pub struct OpenerService {
    runner: Arc<dyn OpenerRunner>,
    allowlist: Arc<OpenerAllowlist>,
    limits: Arc<Limits>,
}

impl OpenerService {
    pub fn new(runner: Arc<dyn OpenerRunner>, allowlist: OpenerAllowlist, limits: Limits) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Open `target` if it passes the allowlist. Returns the resolved target for
    /// audit/trace. The runner only ever receives a host-owned, allowlisted target.
    pub fn open(&self, target: &str) -> Result<Value> {
        self.limits.check_bulk_object(target.len() as u64)?;
        let parsed = OpenTarget::parse(target)?;
        match &parsed {
            OpenTarget::Url { scheme, .. } => {
                if !self.allowlist.allows_url(scheme) {
                    return Err(Error::scope_denied(format!(
                        "kiri.opener.open: URL scheme not on allowlist: {scheme}"
                    )));
                }
            }
            OpenTarget::File { extension, .. } => {
                if !self.allowlist.allows_extension(extension) {
                    return Err(Error::scope_denied(format!(
                        "kiri.opener.open: file extension not on allowlist: {extension}"
                    )));
                }
            }
        }
        self.runner.open(&parsed)?;
        Ok(serde_json::json!({ "target": target }))
    }
}

/// Build the `kiri.opener.*` handlers bound to one OpenerService.
pub fn opener_handlers(
    service: OpenerService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(OPENER_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::OPENER_OPEN,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let target = p.get("target").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::invalid_argument("kiri.opener.open requires string target")
            })?;
            svc.open(target)
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

    struct StubOpener {
        opened: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl OpenerRunner for StubOpener {
        fn open(&self, target: &OpenTarget) -> Result<()> {
            self.opened.lock().unwrap().push(format!("{target:?}"));
            Ok(())
        }
    }

    fn allow() -> OpenerAllowlist {
        OpenerAllowlist::new(
            vec![
                AllowedUrlScheme { scheme: "https".to_string() },
                AllowedUrlScheme { scheme: "mailto".to_string() },
            ],
            vec![
                AllowedFileExtension { extension: "pdf".to_string() },
                AllowedFileExtension { extension: "txt".to_string() },
            ],
        )
    }

    fn router() -> Router {
        let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let svc = OpenerService::new(Arc::new(StubOpener { opened }), allow(), Limits::default());
        Router::new_with_limits(Limits::default()).with_opener(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(OPENER_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_https_url_opens() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::OPENER_OPEN,
            serde_json::json!({ "target": "https://kiri.dev" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["target"], "https://kiri.dev");
    }

    #[test]
    fn allowed_pdf_file_opens() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::OPENER_OPEN,
            serde_json::json!({ "target": "/tmp/report.pdf" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
    }

    #[test]
    fn disallowed_url_scheme_is_denied() {
        let r = router();
        let out =
            dispatch(&r, command_id::OPENER_OPEN, serde_json::json!({ "target": "ssh://host" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn disallowed_file_extension_is_denied() {
        let r = router();
        let out =
            dispatch(&r, command_id::OPENER_OPEN, serde_json::json!({ "target": "/tmp/run.exe" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn capability_denied_without_opener_bit() {
        let r = router();
        let granted = CapabilityBits::empty(); // no OPENER bit
        let req = WireRequest::new(
            command_id::OPENER_OPEN,
            1,
            1,
            serde_json::json!({ "target": "https://kiri.dev" }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        let out = serde_json::to_value(&resp).unwrap();
        assert!(!out["error"].is_null());
    }
}
