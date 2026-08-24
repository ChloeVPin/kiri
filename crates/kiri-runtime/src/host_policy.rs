//! Shared host policy: the allowlists that gate every double-gated surface.
//!
//! Both backends (`host_cross.rs` on macOS/Linux, `host_windows.rs` on
//! Windows) call these same functions so the security posture is identical
//! on every platform. A divergence here would mean one backend is weaker
//! than the other — this module exists so that cannot happen by accident.
//!
//! Each function returns the seed allowlist for a capability-gated surface.
//! In a real app these would be loaded from a `kiri.toml` config file; for
//! the runtime they are compiled in so the default posture is safe and
//! auditable.

/// Host allowlist for `kiri.http.*`. Default-deny: only these hosts may be
/// fetched even when the HTTP capability is granted.
pub fn http_allow_hosts() -> Vec<String> {
    vec!["api.example.com".to_string(), "127.0.0.1".to_string(), "localhost".to_string()]
}

/// Host glob allowlist for `kiri.fs.*` relative to the fs root. Empty would
/// be root-only; the seed uses a safe read-only data scope.
pub fn fs_glob_patterns() -> Vec<String> {
    vec!["data/**".to_string(), "config/*.json".to_string(), "*.log".to_string()]
}

/// Host-owned filesystem-watch targets. The seed watches only the runtime's
/// bounded temporary application directory; frontend code cannot widen it.
pub fn fs_watch_targets() -> Vec<kiri_core::fs_watch::WatchTarget> {
    let root = std::env::temp_dir().join("kiri-fs").to_string_lossy().into_owned();
    vec![
        kiri_core::fs_watch::WatchTarget {
            path: root.clone(),
            kind: kiri_core::fs_watch::WatchKind::All,
        },
        kiri_core::fs_watch::WatchTarget {
            path: root,
            kind: kiri_core::fs_watch::WatchKind::Modify,
        },
    ]
}

/// Host-owned WebSocket URLs. The seed keeps network use local and explicit;
/// an application release should replace this policy with its own signed
/// configuration rather than widening it from frontend JavaScript.
pub fn ws_allow_urls() -> Vec<String> {
    vec!["ws://127.0.0.1:8765".to_string(), "ws://localhost:8765".to_string()]
}

/// Host allowlist for `kiri.shell.run`. Default-deny: only the exact program
/// + arg prefix below may spawn. The seed entry is a harmless readonly probe.
pub fn shell_allow_commands() -> Vec<kiri_core::shell::AllowedCommand> {
    vec![kiri_core::shell::AllowedCommand {
        program: "echo".to_string(),
        args: vec!["kiri-probe".to_string()],
    }]
}

/// Host allowlist of sidecar binary names. Only these exact names may be
/// spawned by the frontend; argv is forced to the host-declared prefix.
pub fn sidecar_allow() -> Vec<kiri_core::sidecar::AllowedSidecar> {
    vec![kiri_core::sidecar::AllowedSidecar {
        name: "kiri-helper".to_string(),
        args: vec!["--mode".to_string(), "fast".to_string()],
    }]
}

/// Host allowlist of event channel names. Only these exact channel names may
/// be published/subscribed by the frontend.
pub fn event_channels() -> Vec<kiri_core::event::AllowedChannel> {
    vec![
        kiri_core::event::AllowedChannel { name: "ping".to_string() },
        kiri_core::event::AllowedChannel { name: "update".to_string() },
        kiri_core::event::AllowedChannel { name: "diag".to_string() },
    ]
}

/// Host key allowlist for `kiri.config.get`. Default-deny: only the exact key
/// paths below may be read by the frontend.
pub fn config_keys() -> Vec<kiri_core::config::AllowedConfigKey> {
    vec![
        kiri_core::config::AllowedConfigKey { key: "app.name".to_string() },
        kiri_core::config::AllowedConfigKey { key: "app.version".to_string() },
        kiri_core::config::AllowedConfigKey { key: "window.theme".to_string() },
    ]
}

/// Host allowlist for `kiri.store.*`. Default-deny: only the exact namespaces
/// below may be addressed.
pub fn store_namespaces() -> Vec<kiri_core::store::StoreNamespace> {
    vec![kiri_core::store::StoreNamespace { prefix: "app.prefs".to_string() }]
}

/// Host allowlist for `kiri.deeplink.register`. Only a host-approved exact
/// scheme may be registered.
pub fn deeplink_schemes() -> Vec<kiri_core::deeplink::DeeplinkScheme> {
    vec![kiri_core::deeplink::DeeplinkScheme { scheme: "kiri-app".to_string() }]
}

/// Host allowlist for `kiri.opener.open` URL schemes.
pub fn opener_url_schemes() -> Vec<kiri_core::opener::AllowedUrlScheme> {
    vec![
        kiri_core::opener::AllowedUrlScheme { scheme: "https".to_string() },
        kiri_core::opener::AllowedUrlScheme { scheme: "http".to_string() },
        kiri_core::opener::AllowedUrlScheme { scheme: "mailto".to_string() },
    ]
}

/// Host allowlist for `kiri.opener.open` file extensions.
pub fn opener_file_extensions() -> Vec<kiri_core::opener::AllowedFileExtension> {
    vec![
        kiri_core::opener::AllowedFileExtension { extension: "pdf".to_string() },
        kiri_core::opener::AllowedFileExtension { extension: "txt".to_string() },
        kiri_core::opener::AllowedFileExtension { extension: "md".to_string() },
    ]
}

/// Host policy for `kiri.autostart.*`. Default-deny: autostart is disabled
/// unless the host explicitly opts in.
pub fn autostart_policy() -> bool {
    false
}

/// Host allowlist for `kiri.shortcut.register`. Only the exact accelerators
/// below may bind, each mapped to a host-owned action.
pub fn shortcut_bindings() -> Vec<kiri_core::shortcut::ShortcutBinding> {
    vec![
        kiri_core::shortcut::ShortcutBinding {
            accelerator: "CmdOrCtrl+S".to_string(),
            action: "save".to_string(),
        },
        kiri_core::shortcut::ShortcutBinding {
            accelerator: "CmdOrCtrl+K".to_string(),
            action: "command-palette".to_string(),
        },
    ]
}

/// Host allowlist for `kiri.dialog.open`. Only pre-approved dialog kinds with
/// host-owned titles may show.
pub fn dialog_templates() -> Vec<kiri_core::dialog::DialogTemplate> {
    vec![
        kiri_core::dialog::DialogTemplate {
            kind: kiri_core::dialog::DialogKind::Message,
            title_template: "Update available: {0}".to_string(),
            args: 1,
            filters: vec![],
        },
        kiri_core::dialog::DialogTemplate {
            kind: kiri_core::dialog::DialogKind::OpenFile,
            title_template: "Open project".to_string(),
            args: 0,
            filters: vec!["kiri".to_string(), "json".to_string()],
        },
    ]
}

/// Host template allowlist for `kiri.notification.show`. Only pre-approved
/// template ids with bounded args may display.
pub fn notification_templates() -> Vec<kiri_core::notification::NotificationTemplate> {
    vec![
        kiri_core::notification::NotificationTemplate {
            id: "download-complete".to_string(),
            title: "Download finished: {0}".to_string(),
            body: "Saved to {1}".to_string(),
            args: 2,
        },
        kiri_core::notification::NotificationTemplate {
            id: "build-failed".to_string(),
            title: "Build failed".to_string(),
            body: "{0}".to_string(),
            args: 1,
        },
    ]
}

/// Host-pinned Ed25519 public key for the signed-update verifier (audit-18).
/// NEVER sourced from the frontend: a malicious or phished page cannot
/// substitute a key and accept an attacker-signed release. The matching
/// secret signs release assets at build time. Rotate only via a new pinned
/// build.
pub const HOST_PINNED_UPDATE_PUBLIC_KEY: &str =
    "333d58ae1e42ba2025b035666528d36430e0c14e13f3d5006c7f0fe22a9d3af6";
