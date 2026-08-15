//! Restricted native dialog surface (`kiri.dialog`).
//!
//! This closes another Tauri plugin parity gap and converts Tauri's dialog risk
//! into a Kiri strength. Tauri's dialog plugin, when the capability is granted,
//! lets the frontend open native message boxes / file pickers with arbitrary
//! title, message, and button text. That is a spoofing/social-engineering
//! surface: a malicious or compromised frontend can render a native dialog that
//! looks like a system prompt ("enter your password", "update required").
//!
//! Kiri requires BOTH the `DIALOG` capability bit AND a host allowlist of
//! permitted dialog *kinds* (and, for message dialogs, a host-owned title
//! template). The frontend may only request a pre-approved dialog kind; it
//! cannot fabricate free-form native UI. The actual native call is behind the
//! `DialogRunner` trait (mirrors `NotificationRunner`): the native host injects a
//! real displayer; tests use a `StubDialog` and assert kind enforcement,
//! capability gating, and the host-owned title contract without opening any UI.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.dialog.*` commands.
pub const DIALOG_CAPABILITY: u32 = 13;

/// A permitted dialog kind the host allows the frontend to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    /// One-way informational message (OK only).
    Message,
    /// Yes/No confirmation.
    Confirm,
    /// Native open-file picker.
    OpenFile,
    /// Native save-file picker.
    SaveFile,
}

impl DialogKind {
    fn parse(s: &str) -> Option<DialogKind> {
        match s {
            "message" => Some(DialogKind::Message),
            "confirm" => Some(DialogKind::Confirm),
            "open-file" => Some(DialogKind::OpenFile),
            "save-file" => Some(DialogKind::SaveFile),
            _ => None,
        }
    }
}

/// Host-approved dialog configuration. `title_template` (with `{0}`.. args) is
/// the host-owned title; the frontend may only supply bounded `args` that fill
/// it. For file pickers, `filters` lists permitted extensions (default-deny:
/// empty list rejects all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogTemplate {
    pub kind: DialogKind,
    pub title_template: String,
    pub args: usize,
    /// Permitted file extensions for file pickers (without leading dot).
    /// Empty = no extension allowed (effectively denies the picker).
    pub filters: Vec<String>,
}

/// Host-configured allowlist of dialogs that may be opened. Default-deny: a
/// dialog opens only if its kind + (for file pickers) extension filter match an
/// entry. The host owns the title text, so the frontend cannot render a
/// free-form native prompt.
#[derive(Debug, Clone, Default)]
pub struct DialogAllowlist {
    templates: Vec<DialogTemplate>,
}

impl DialogAllowlist {
    pub fn new(templates: Vec<DialogTemplate>) -> Self {
        Self { templates }
    }

    fn resolve(&self, kind: &str, ext: Option<&str>) -> Option<(DialogKind, String, usize)> {
        let k = DialogKind::parse(kind)?;
        let t = self.templates.iter().find(|t| t.kind == k)?;
        // For file pickers, the extension must be on the allowlist.
        if matches!(k, DialogKind::OpenFile | DialogKind::SaveFile) {
            match ext {
                Some(e) if t.filters.iter().any(|f| f.eq_ignore_ascii_case(e)) => {}
                _ => return None,
            }
        }
        Some((k, t.title_template.clone(), t.args))
    }

    pub fn templates(&self) -> &[DialogTemplate] {
        &self.templates
    }
}

/// A dialog result returned to the caller.
#[derive(Debug, Clone)]
pub struct DialogResult {
    pub kind: String,
    /// For confirm: true if confirmed. For file pickers: chosen path(s).
    pub confirmed: bool,
    pub paths: Vec<String>,
}

/// Transport seam. The native host provides a real dialog displayer; tests
/// provide a stub. Kept trait-based so the logical protocol has zero platform
/// deps.
pub trait DialogRunner: Send + Sync {
    fn show(&self, kind: DialogKind, title: &str) -> Result<DialogResult>;
}

/// Capability-scoped dialog service bounded to a host allowlist plus limits.
#[derive(Clone)]
pub struct DialogService {
    runner: Arc<dyn DialogRunner>,
    allowlist: Arc<DialogAllowlist>,
    limits: Arc<Limits>,
}

impl DialogService {
    pub fn new(runner: Arc<dyn DialogRunner>, allowlist: DialogAllowlist, limits: Limits) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Open a dialog if its kind (and file extension, if a picker) is on the
    /// allowlist. Returns the host-resolved title so the audit trail is honest.
    pub fn open(&self, kind: &str, args: &[String], ext: Option<&str>) -> Result<Value> {
        let (k, title_template, argc) = self.allowlist.resolve(kind, ext).ok_or_else(|| {
            Error::scope_denied(format!("kiri.dialog.open: kind not allowed: {kind}"))
        })?;
        if args.len() > argc {
            return Err(Error::scope_denied(format!(
                "kiri.dialog.open: too many args for {kind} (max {argc})"
            )));
        }
        self.limits.check_bulk_object((title_template.len()) as u64)?;
        let title = substitute(&title_template, args);
        let res = self.runner.show(k, &title)?;
        Ok(serde_json::json!({
            "kind": kind,
            "title": title,
            "confirmed": res.confirmed,
            "paths": res.paths,
        }))
    }
}

fn substitute(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = template[i + 1..].find('}') {
                let inner = &template[i + 1..i + 1 + end];
                if let Ok(idx) = inner.parse::<usize>() {
                    if let Some(arg) = args.get(idx) {
                        out.push_str(arg);
                        i = i + 1 + end + 1;
                        continue;
                    }
                }
                out.push('{');
                i += 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Build the `kiri.dialog.*` handlers bound to one DialogService.
pub fn dialog_handlers(
    service: DialogService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(DIALOG_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::DIALOG_OPEN,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let kind = p
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::invalid_argument("kiri.dialog.open requires string kind"))?;
            let args = p
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let ext = p.get("ext").and_then(|v| v.as_str());
            svc.open(kind, &args, ext)
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

    struct StubDialog {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl DialogRunner for StubDialog {
        fn show(&self, _k: DialogKind, title: &str) -> Result<DialogResult> {
            self.seen.lock().unwrap().push(title.to_string());
            Ok(DialogResult { kind: "stub".to_string(), confirmed: true, paths: vec![] })
        }
    }

    fn allow() -> DialogAllowlist {
        DialogAllowlist::new(vec![
            DialogTemplate {
                kind: DialogKind::Message,
                title_template: "Update available: {0}".to_string(),
                args: 1,
                filters: vec![],
            },
            DialogTemplate {
                kind: DialogKind::OpenFile,
                title_template: "Open project".to_string(),
                args: 0,
                filters: vec!["kiri".to_string(), "json".to_string()],
            },
        ])
    }

    fn router() -> Router {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let svc = DialogService::new(Arc::new(StubDialog { seen }), allow(), Limits::default());
        Router::new_with_limits(Limits::default()).with_dialog(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(DIALOG_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_message_dialog_shows_resolved_title() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DIALOG_OPEN,
            serde_json::json!({ "kind": "message", "args": ["v2.0"] }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["title"], "Update available: v2.0");
        assert_eq!(out["payload"]["confirmed"], true);
    }

    #[test]
    fn unknown_kind_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DIALOG_OPEN,
            serde_json::json!({ "kind": "confirm", "args": [] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn file_picker_rejects_disallowed_extension() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DIALOG_OPEN,
            serde_json::json!({ "kind": "open-file", "ext": "exe" }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn file_picker_allows_approved_extension() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DIALOG_OPEN,
            serde_json::json!({ "kind": "open-file", "ext": "kiri" }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
    }

    #[test]
    fn too_many_args_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::DIALOG_OPEN,
            serde_json::json!({ "kind": "message", "args": ["a", "b"] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn missing_dialog_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::DIALOG_OPEN,
            1,
            1,
            serde_json::json!({ "kind": "message", "args": ["x"] }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }
}
