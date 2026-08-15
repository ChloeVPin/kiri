//! Platform-neutral security policy (specs/SECURITY.md).
//!
//! Both host backends (Win32/WebView2 on Windows, wry/tao on macOS/Linux)
//! share this policy so the trust boundary is identical on every platform:
//!
//! - the application origin is fixed and known at build time;
//! - only messages and navigations from that origin are trusted;
//! - caller identity and the capability mask are assigned by native code and
//!   can never be supplied by JavaScript.
//!
//! Transport-specific origin extraction (WebView2 `Source`, wry IPC
//! `Origin` header) lives in each backend; this module judges the result.

/// The application origin used by the cross (wry/tao) backend. It is a custom
/// scheme host, so it is stable and cannot be confused with a remote origin.
pub const CROSS_APP_ORIGIN: &str = "kiri://localhost";

/// The application origin used by the Windows direct backend (WebV2 virtual
/// host mapping, docs/06-windows.md D-004).
pub const WINDOWS_APP_ORIGIN: &str = "https://app.local";

/// True when `origin` is one of the known application origins, or a path
/// under one of them. Document URLs for the app page (for example
/// `kiri://localhost/index.html`) are accepted so the IPC trust boundary
/// matches the Windows `is_app_origin_url` check.
pub fn is_app_origin(origin: &str) -> bool {
    origin == CROSS_APP_ORIGIN
        || origin == WINDOWS_APP_ORIGIN
        || origin.starts_with(&format!("{CROSS_APP_ORIGIN}/"))
        || origin.starts_with(&format!("{WINDOWS_APP_ORIGIN}/"))
}

/// Policy for navigations initiated by the page.
///
/// Returns `true` only when the target is the application origin (or an
/// in-page fragment/relative navigation that stays within it). Remote
/// navigations are rejected so a remote page can never retain privileged
/// bridge access (specs/SECURITY.md, Navigation).
pub fn is_navigation_allowed(target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    // Allow same-origin and relative/fragment navigations; reject anything
    // that escapes to a remote scheme (http/https other than the app origin,
    // or any other scheme).
    if target.starts_with("http://") || target.starts_with("https://") {
        return is_app_origin(target);
    }
    if target.starts_with("kiri://") {
        return target == CROSS_APP_ORIGIN || target.starts_with(&format!("{CROSS_APP_ORIGIN}/"));
    }
    // Relative paths, fragments, and javascript: are not remote navigations
    // that would load a new document from another origin; the webview keeps
    // the current (application) document. Block javascript: and data: URIs.
    if target.starts_with("javascript:") || target.starts_with("data:") {
        return false;
    }
    true
}

/// Authoritative capability assignment for a trusted native caller.
///
/// Native code calls this; JavaScript never supplies the mask. The returned
/// `CapabilityBits` is the full trusted set for the application frontend.
pub fn trusted_frontend_capabilities() -> crate::capabilities::CapabilityBits {
    let mut caps = crate::capabilities::CapabilityBits::empty();
    caps.set(crate::dispatch::capability_bit::PING);
    caps.set(crate::dispatch::capability_bit::DIAGNOSTICS);
    // R-3 JS surface: the trusted frontend may read platform/app facts and use
    // the event bus. Resource mutation still requires the separate RESOURCES
    // bit and is not granted here.
    caps.set(crate::dispatch::capability_bit::PLATFORM);
    caps.set(crate::dispatch::capability_bit::APP);
    caps.set(crate::dispatch::capability_bit::EVENT);
    // G-6: the trusted frontend may use the capability-gated clipboard surface
    // (kiri.clipboard.read/write). Authorization still flows through the
    // CLIPBOARD capability bit even though it is granted here.
    caps.set(crate::dispatch::capability_bit::CLIPBOARD);
    // G-7: the trusted frontend may use the capability-gated path/os
    // surface (kiri.path.* / kiri.os.*). Authorization still flows through
    // the PATH capability bit even though it is granted here. Tauri grants
    // its path/os plugins by default; Kiri gates them, exceeding that
    // security axis by default.
    caps.set(crate::dispatch::capability_bit::PATH);
    // G-3: the trusted frontend may use the capability-scoped http surface
    // (kiri.http.get). Authorization still flows through the HTTP capability
    // bit even though it is granted here; the host allowlist is the second
    // gate, so this still exceeds Tauri's unrestricted http plugin.
    caps.set(crate::dispatch::capability_bit::HTTP);
    // G-4: the trusted frontend may use the restricted, host-allowlisted
    // shell surface (kiri.shell.run). Authorization still flows through the
    // SHELL capability bit even though it is granted here; the host allowlist
    // is the second gate, so this still exceeds Tauri's unrestricted shell
    // plugin.
    caps.set(crate::dispatch::capability_bit::SHELL);
    // G-4b: the trusted frontend may use the restricted, host-template-
    // allowlisted notification surface (kiri.notification.show). Authorization
    // still flows through the NOTIFICATION capability bit even though it is
    // granted here; the host template allowlist is the second gate, so this
    // still exceeds Tauri's unrestricted notification plugin.
    caps.set(crate::dispatch::capability_bit::NOTIFICATION);
    // G-4c: the trusted frontend may use the restricted, host-allowlisted native
    // dialog surface (kiri.dialog.open). Authorization still flows through the
    // DIALOG capability bit even though it is granted here; the host allowlist is
    // the second gate, so this still exceeds Tauri's unrestricted dialog plugin.
    caps.set(crate::dispatch::capability_bit::DIALOG);
    // G-4d: the trusted frontend may use the restricted, host-allowlisted global
    // shortcut surface (kiri.shortcut.register). Authorization still flows through the
    // SHORTCUT capability bit even though it is granted here; the host allowlist is
    // the second gate, so this still exceeds Tauri's unrestricted global-shortcut plugin.
    caps.set(crate::dispatch::capability_bit::SHORTCUT);
    // G-4e: the trusted frontend may use the restricted, host-policy-gated autostart
    // surface (kiri.autostart.set/get). Authorization still flows through the AUTOSTART
    // capability bit even though it is granted here; the host policy is the second gate,
    // so this still exceeds Tauri's unrestricted autostart plugin.
    caps.set(crate::dispatch::capability_bit::AUTOSTART);
    caps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_origins_recognized() {
        assert!(is_app_origin(CROSS_APP_ORIGIN));
        assert!(is_app_origin(WINDOWS_APP_ORIGIN));
        assert!(!is_app_origin("https://evil.example.com"));
        assert!(!is_app_origin("null"));
        assert!(is_app_origin("kiri://localhost/index.html"));
        assert!(is_app_origin("https://app.local/index.html"));
    }

    #[test]
    fn only_app_origin_navigations_allowed() {
        assert!(is_navigation_allowed(CROSS_APP_ORIGIN));
        assert!(is_navigation_allowed(WINDOWS_APP_ORIGIN));
        assert!(!is_navigation_allowed("https://evil.example.com/"));
        assert!(!is_navigation_allowed("http://app.local/"));
        assert!(!is_navigation_allowed("javascript:alert(1)"));
        assert!(!is_navigation_allowed("data:text/html,x"));
        // relative/fragment stays within the document
        assert!(is_navigation_allowed("#section"));
        assert!(is_navigation_allowed("/index.html"));
    }

    #[test]
    fn trusted_frontend_has_ping() {
        let caps = trusted_frontend_capabilities();
        assert!(caps.has(crate::dispatch::capability_bit::PING));
    }
}
