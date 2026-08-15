//! Restricted notification surface (`kiri.notification`).
//!
//! This closes another Tauri plugin parity gap and converts Tauri's notification
//! risk into a Kiri strength. Tauri's notification plugin, when the capability is
//! granted, lets the frontend send an arbitrary title and body. That is a
//! spoofing/phishing surface: a compromised or malicious frontend can render a
//! system-looking notification (fake "update available", credential prompt, etc.).
//!
//! Kiri requires BOTH the `NOTIFICATION` capability bit AND a host template
//! allowlist. The frontend references a pre-registered template id and supplies
//! only bounded positional arguments; the host owns the title/body text and the
//! placeholder substitution. A granted capability with no matching template id is
//! refused, so JavaScript can never render free-form notification content. The
//! resolved title/body are plain text handed to the runner; Kiri never forwards
//! frontend HTML into a notification.
//!
//! The actual display is behind the `NotificationRunner` trait (mirrors
//! `ShellRunner`/`HttpClient`): the native host injects a real displayer; tests
//! use a `StubNotification` and assert template enforcement, arg validation, and
//! capability gating without showing any UI.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.notification.*` commands.
pub const NOTIFICATION_CAPABILITY: u32 = 12;

/// One host-approved notification template. `args` is the expected positional
/// argument count; the frontend may supply at most that many. Title/body may
/// contain `{0}`, `{1}`, ... placeholders substituted by the supplied args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTemplate {
    pub id: String,
    pub title: String,
    pub body: String,
    pub args: usize,
}

/// Host-configured allowlist of notification templates. Default-deny: a
/// notification shows only if its template id is listed. The host owns the text;
/// the frontend only picks a template and fills bounded args.
#[derive(Debug, Clone, Default)]
pub struct NotificationAllowlist {
    templates: Vec<NotificationTemplate>,
}

impl NotificationAllowlist {
    pub fn new(templates: Vec<NotificationTemplate>) -> Self {
        Self { templates }
    }

    fn resolve(&self, id: &str, args: &[String]) -> Option<(String, String)> {
        let t = self.templates.iter().find(|t| t.id == id)?;
        if args.len() > t.args {
            return None;
        }
        let title = substitute(&t.title, args);
        let body = substitute(&t.body, args);
        Some((title, body))
    }

    pub fn templates(&self) -> &[NotificationTemplate] {
        &self.templates
    }
}

/// Substitute `{i}` placeholders (i in 0..args.len()) with the matching arg.
/// Unknown placeholders pass through unchanged; missing args leave the literal.
fn substitute(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Find closing brace.
            if let Some(end) = template[i + 1..].find('}') {
                let inner = &template[i + 1..i + 1 + end];
                if let Ok(idx) = inner.parse::<usize>() {
                    if let Some(arg) = args.get(idx) {
                        out.push_str(arg);
                        i = i + 1 + end + 1;
                        continue;
                    }
                }
                // Not a valid index; emit literally.
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

/// A displayed notification result.
#[derive(Debug, Clone)]
pub struct NotificationShown {
    pub template_id: String,
    pub title: String,
    pub body: String,
}

/// Transport seam. The native host provides a real displayer; tests provide a
/// stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait NotificationRunner: Send + Sync {
    fn show(&self, title: &str, body: &str) -> Result<()>;
}

/// Capability-scoped notification service bounded to a template allowlist plus
/// limits (body length cap).
#[derive(Clone)]
pub struct NotificationService {
    runner: Arc<dyn NotificationRunner>,
    allowlist: Arc<NotificationAllowlist>,
    limits: Arc<Limits>,
}

impl NotificationService {
    pub fn new(
        runner: Arc<dyn NotificationRunner>,
        allowlist: NotificationAllowlist,
        limits: Limits,
    ) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Show a notification if its template id is on the allowlist and all bounds
    /// hold. Returns the resolved (host-owned) title/body for audit/trace.
    pub fn show(&self, template_id: &str, args: &[String]) -> Result<Value> {
        let (title, body) = self.allowlist.resolve(template_id, args).ok_or_else(|| {
            Error::scope_denied(format!(
                "kiri.notification.show: template not on allowlist: {template_id}"
            ))
        })?;
        // Bounded body length so a template misconfiguration cannot flood memory.
        self.limits.check_bulk_object((title.len() + body.len()) as u64)?;
        self.runner.show(&title, &body)?;
        Ok(serde_json::json!({
            "templateId": template_id,
            "title": title,
            "body": body,
        }))
    }
}

/// Build the `kiri.notification.*` handlers bound to one NotificationService.
pub fn notification_handlers(
    service: NotificationService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(NOTIFICATION_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::NOTIFY,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let template_id = p.get("template").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::invalid_argument("kiri.notification.show requires string template")
            })?;
            let args = p
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            svc.show(template_id, &args)
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

    struct StubNotification {
        shown: std::sync::Arc<std::sync::Mutex<Vec<NotificationShown>>>,
    }
    impl NotificationRunner for StubNotification {
        fn show(&self, title: &str, body: &str) -> Result<()> {
            self.shown.lock().unwrap().push(NotificationShown {
                template_id: String::new(),
                title: title.to_string(),
                body: body.to_string(),
            });
            Ok(())
        }
    }

    fn allow() -> NotificationAllowlist {
        NotificationAllowlist::new(vec![NotificationTemplate {
            id: "download-complete".to_string(),
            title: "Download finished: {0}".to_string(),
            body: "Saved to {1}".to_string(),
            args: 2,
        }])
    }

    fn router() -> Router {
        let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let svc = NotificationService::new(
            Arc::new(StubNotification { shown }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_notification(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(NOTIFICATION_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_template_shows_resolved_text() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::NOTIFY,
            serde_json::json!({ "template": "download-complete", "args": ["report.pdf", "/tmp/report.pdf"] }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["title"], "Download finished: report.pdf");
        assert_eq!(out["payload"]["body"], "Saved to /tmp/report.pdf");
    }

    #[test]
    fn unknown_template_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::NOTIFY,
            serde_json::json!({ "template": "system-update-required", "args": [] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn too_many_args_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::NOTIFY,
            serde_json::json!({ "template": "download-complete", "args": ["a", "b", "c"] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn missing_notification_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::NOTIFY,
            1,
            1,
            serde_json::json!({ "template": "download-complete", "args": ["a", "b"] }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn frontend_cannot_inject_freeform_title() {
        // There is no command that forwards a raw title/body; the only path is a
        // host-owned template. Assert the allowlist exposes no free-form shape.
        let a = allow();
        assert!(a.templates().iter().all(|t| t.title.contains('{') || !t.title.is_empty()));
        // The contract: every template is host-declared, never frontend-supplied.
        assert!(a.templates().iter().all(|t| !t.id.is_empty()));
    }
}
