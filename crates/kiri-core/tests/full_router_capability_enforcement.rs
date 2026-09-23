//! Full-router capability and allowlist (double-gate) enforcement tests.
//!
//! Builds a host-equivalent router: every with_* surface wired with the same
//! simple service constructors the native host uses (no runtime or host
//! handles), then asserts two independent gates:
//!
//! 1. Empty capability set: every command id is denied as Unauthorized.
//! 2. Capability granted + empty or wrong allowlist: representative
//!    double-gated commands are denied as ScopeDenied.
//!
//! Gate (1) alone is not double-gating. Gate (2) is the proof that removing the
//! allowlist second gate while leaving capabilities in place would fail CI.
//!
//! Surfaces covered by gate (2) on main: HTTP host allowlist, shell command
//! allowlist, event channel allowlist (with_event EVENT_PUBLISH/SUBSCRIBE),
//! fs PathScope (flags + escape), opener scheme/extension allowlist.
//!
//! Known gaps on main (not asserted here; depend on open PRs):
//! - Legacy EVENT_EMIT/LISTEN (ids 8/9) are capability-only; channel allowlist
//!   for those paths is tracked in PR #24.
//! - Shell exact-argv tightening is tracked in PR #25 (prefix match already
//!   denies empty/wrong programs on main).
//! - HTTP method fail-closed GET-only is tracked in PR #26 (host allowlist
//!   second gate is already enforced on main).

use std::sync::Arc;

use kiri_core::caller::CallerId;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::dispatch::{capability_bit, command_id, Router};
use kiri_core::error::ErrorCode;
use kiri_core::event::{AllowedChannel, EventAllowlist};
use kiri_core::opener::{AllowedFileExtension, AllowedUrlScheme};
use kiri_core::shell::AllowedCommand;
use kiri_core::trace::RingTraceSink;
use kiri_core::wire::WireRequest;
use serde_json::json;

// Stubs for host-owned backends. Capability-deny tests never invoke them.
// Double-gate tests reach the allowlist check before the stub body matters;
// stubs still must exist so the router can be constructed headlessly.

struct StubWindow;
impl kiri_core::window::WindowController for StubWindow {
    fn set_title(&self, _s: &mut kiri_core::window::WindowState, _t: &str) {}
    fn show(&self, _s: &mut kiri_core::window::WindowState) {}
    fn hide(&self, _s: &mut kiri_core::window::WindowState) {}
    fn minimize(&self, _s: &mut kiri_core::window::WindowState) {}
    fn maximize(&self, _s: &mut kiri_core::window::WindowState) {}
    fn restore(&self, _s: &mut kiri_core::window::WindowState) {}
    fn close(&self, _s: &mut kiri_core::window::WindowState) {}
    fn focus(&self, _s: &mut kiri_core::window::WindowState) {}
}

struct StubClipboard;
impl kiri_core::clipboard::ClipboardController for StubClipboard {
    fn read(
        &self,
        _s: &mut kiri_core::clipboard::ClipboardState,
    ) -> kiri_core::error::Result<String> {
        Ok(String::new())
    }
    fn write(&self, _s: &mut kiri_core::clipboard::ClipboardState, _t: &str) {}
}

struct StubHttpClient;
impl kiri_core::http::HttpClient for StubHttpClient {
    fn fetch(
        &self,
        _req: kiri_core::http::HttpRequest,
    ) -> kiri_core::error::Result<kiri_core::http::HttpResponse> {
        Ok(kiri_core::http::HttpResponse { status: 0, headers: vec![], body: vec![] })
    }
}

struct StubShellRunner;
impl kiri_core::shell::ShellRunner for StubShellRunner {
    fn run(
        &self,
        _p: &str,
        _a: &[String],
    ) -> kiri_core::error::Result<kiri_core::shell::ShellOutput> {
        Ok(kiri_core::shell::ShellOutput { exit_code: 0, stdout: vec![], stderr: vec![] })
    }
}

struct StubDialogRunner;
impl kiri_core::dialog::DialogRunner for StubDialogRunner {
    fn show(
        &self,
        _k: kiri_core::dialog::DialogKind,
        _t: &str,
    ) -> kiri_core::error::Result<kiri_core::dialog::DialogResult> {
        Ok(kiri_core::dialog::DialogResult { kind: String::new(), confirmed: false, paths: vec![] })
    }
}

struct StubOpenerRunner;
impl kiri_core::opener::OpenerRunner for StubOpenerRunner {
    fn open(&self, _t: &kiri_core::opener::OpenTarget) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubTrayRunner;
impl kiri_core::tray::TrayRunner for StubTrayRunner {
    fn set_menu(&self, _i: &[kiri_core::tray::TrayItem]) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn invoke(&self, _id: &str, _a: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubSidecarRunner;
impl kiri_core::sidecar::SidecarRunner for StubSidecarRunner {
    fn spawn(
        &self,
        _n: &str,
        _p: &str,
        _a: &[String],
    ) -> kiri_core::error::Result<kiri_core::sidecar::SidecarOutput> {
        Ok(kiri_core::sidecar::SidecarOutput { exit_code: 0, stdout: vec![], stderr: vec![] })
    }
}

struct StubShortcutRunner;
impl kiri_core::shortcut::ShortcutRunner for StubShortcutRunner {
    fn register(&self, _a: &str, _act: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubAutostartRunner;
impl kiri_core::autostart::AutostartRunner for StubAutostartRunner {
    fn set_enabled(&self, _e: bool) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn is_enabled(&self) -> kiri_core::error::Result<bool> {
        Ok(false)
    }
}

struct StubDeeplinkRunner;
impl kiri_core::deeplink::DeeplinkRunner for StubDeeplinkRunner {
    fn register(&self, _s: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubEventBusBackend;
impl kiri_core::event::EventBusBackend for StubEventBusBackend {
    fn subscribe(&self, _c: &str) -> u64 {
        1
    }
    fn publish(&self, _c: &str, _p: serde_json::Value) {}
    fn drain(&self, _s: u64) -> Vec<serde_json::Value> {
        vec![]
    }
}

struct StubConfigBackend;
impl kiri_core::config::ConfigBackend for StubConfigBackend {
    fn get(&self, _k: &str) -> Option<serde_json::Value> {
        None
    }
}

struct StubFsWatch;
impl kiri_core::fs_watch::FsWatchBackend for StubFsWatch {
    fn watch(&self, _t: &kiri_core::fs_watch::WatchTarget) -> kiri_core::error::Result<u64> {
        Ok(1)
    }
    fn unwatch(&self, _id: u64) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn drain(&self, _id: u64) -> Vec<kiri_core::fs_watch::WatchEvent> {
        vec![]
    }
}

struct StubWs;
impl kiri_core::websocket::WsBackend for StubWs {
    fn open(&self, _url: &str) -> kiri_core::error::Result<u64> {
        Ok(1)
    }
    fn send(&self, _id: u64, _m: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn close(&self, _id: u64) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn drain(&self, _id: u64) -> Vec<kiri_core::websocket::WsMessage> {
        vec![]
    }
}

struct StubMenu;
impl kiri_core::app_menu::MenuRunner for StubMenu {
    fn set_menu(&self, _i: &[kiri_core::app_menu::MenuItem]) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn invoke(&self, _id: &str, _a: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubStoreBackend;
impl kiri_core::store::StoreBackend for StubStoreBackend {
    fn get(&self, _ns: &str, _k: &str) -> kiri_core::error::Result<Option<serde_json::Value>> {
        Ok(None)
    }
    fn set(&self, _ns: &str, _k: &str, _v: serde_json::Value) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

struct StubWindowStateBackend;
impl kiri_core::window_state::WindowStateBackend for StubWindowStateBackend {
    fn save(&self, _g: &kiri_core::window_state::Geometry) -> kiri_core::error::Result<()> {
        Ok(())
    }
    fn load(&self) -> kiri_core::error::Result<Option<kiri_core::window_state::Geometry>> {
        Ok(None)
    }
}

struct StubNotificationRunner;
impl kiri_core::notification::NotificationRunner for StubNotificationRunner {
    fn show(&self, _t: &str, _b: &str) -> kiri_core::error::Result<()> {
        Ok(())
    }
}

/// Host-policy seeds for the second gate. Empty vecs / deny flags match a
/// host that registered the surface but installed no allowlist entries.
struct AllowlistSeeds {
    http_hosts: Vec<String>,
    shell_commands: Vec<AllowedCommand>,
    event_channels: Vec<AllowedChannel>,
    opener_url_schemes: Vec<AllowedUrlScheme>,
    opener_file_extensions: Vec<AllowedFileExtension>,
    /// When true, PathScope allows reads under the temp root (escape tests).
    fs_read: bool,
    fs_write: bool,
}

impl Default for AllowlistSeeds {
    fn default() -> Self {
        Self {
            http_hosts: vec![],
            shell_commands: vec![],
            event_channels: vec![],
            opener_url_schemes: vec![],
            opener_file_extensions: vec![],
            fs_read: false,
            fs_write: false,
        }
    }
}

fn full_router() -> Router {
    full_router_with(AllowlistSeeds::default())
}

fn full_router_with(seeds: AllowlistSeeds) -> Router {
    let limits = kiri_core::limits::Limits::default();
    let caller = kiri_core::caller::CallerRegistry::new().register();
    let diag = kiri_core::diagnostics::Diagnostics::new();

    let mut fs_scope = kiri_core::capabilities::PathScope::new(std::env::temp_dir());
    fs_scope.read = seeds.fs_read;
    fs_scope.write = seeds.fs_write;

    Router::new()
        .with_diagnostics(diag.clone())
        .with_resources(diag.clone(), caller)
        .with_platform(kiri_core::platform::EventBus::new())
        .with_fs_service(kiri_core::fs::FsService::new(fs_scope, limits.clone()))
        .with_window(
            Arc::new(StubWindow),
            Arc::new(std::sync::Mutex::new(kiri_core::window::WindowState::new("kiri"))),
        )
        .with_clipboard(
            Arc::new(StubClipboard),
            Arc::new(std::sync::Mutex::new(kiri_core::clipboard::ClipboardState::new())),
        )
        .with_path(kiri_core::path::PathService::new(kiri_core::path::PathState::new()))
        .with_http(kiri_core::http::HttpService::new(
            Arc::new(StubHttpClient),
            kiri_core::http::HostAllowlist::new(seeds.http_hosts),
            limits.clone(),
        ))
        .with_shell(kiri_core::shell::ShellService::new(
            Arc::new(StubShellRunner),
            kiri_core::shell::ShellAllowlist::new(seeds.shell_commands),
            limits.clone(),
        ))
        .with_notification(kiri_core::notification::NotificationService::new(
            Arc::new(StubNotificationRunner),
            kiri_core::notification::NotificationAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_dialog(kiri_core::dialog::DialogService::new(
            Arc::new(StubDialogRunner),
            kiri_core::dialog::DialogAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_shortcut(kiri_core::shortcut::ShortcutService::new(
            Arc::new(StubShortcutRunner),
            kiri_core::shortcut::ShortcutAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_autostart(kiri_core::autostart::AutostartService::new(
            Arc::new(StubAutostartRunner),
            kiri_core::autostart::AutostartAllowlist::new(false),
            limits.clone(),
        ))
        .with_store(kiri_core::store::StoreService::new(
            Arc::new(StubStoreBackend),
            kiri_core::store::StoreAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_deeplink(kiri_core::deeplink::DeeplinkService::new(
            Arc::new(StubDeeplinkRunner),
            kiri_core::deeplink::DeeplinkAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_opener(kiri_core::opener::OpenerService::new(
            Arc::new(StubOpenerRunner),
            kiri_core::opener::OpenerAllowlist::new(
                seeds.opener_url_schemes,
                seeds.opener_file_extensions,
            ),
            limits.clone(),
        ))
        .with_window_state(kiri_core::window_state::WindowStateService::new(
            Arc::new(StubWindowStateBackend),
            limits.clone(),
        ))
        .with_tray(kiri_core::tray::TrayService::new(
            Arc::new(StubTrayRunner),
            kiri_core::tray::TrayAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_sidecar(kiri_core::sidecar::SidecarService::new(
            Arc::new(StubSidecarRunner),
            kiri_core::sidecar::SidecarAllowlist::new(vec![]),
            kiri_core::sidecar::SidecarTable::new(),
            limits.clone(),
        ))
        .with_event(kiri_core::event::EventService::new(
            Arc::new(StubEventBusBackend),
            EventAllowlist::new(seeds.event_channels),
            limits.clone(),
        ))
        .with_config(kiri_core::config::ConfigService::new(
            Arc::new(StubConfigBackend),
            kiri_core::config::ConfigAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_updater(kiri_core::updater_surface::UpdaterService::new(
            "00".to_string(),
            kiri_core::update::Version::parse("0.0.0").unwrap(),
            limits.clone(),
        ))
        .with_cli(kiri_core::cli::CliService::new(std::env::args().collect::<Vec<String>>()))
        .with_fs_watch(kiri_core::fs_watch::FsWatchService::new(
            Arc::new(StubFsWatch),
            kiri_core::fs_watch::FsWatchAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_ws(kiri_core::websocket::WsService::new(
            Arc::new(StubWs),
            kiri_core::websocket::WsAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_menu(kiri_core::app_menu::MenuService::new(
            Arc::new(StubMenu),
            kiri_core::app_menu::MenuAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_plugin_inventory(kiri_core::plugin_inventory::PluginInventory::empty())
}

fn caps(bits: &[u32]) -> CapabilityBits {
    let mut c = CapabilityBits::empty();
    for bit in bits {
        c.set(*bit);
    }
    c
}

fn assert_scope_denied(router: &Router, granted: &CapabilityBits, id: u32, payload: serde_json::Value) {
    let req = WireRequest::new(id, id as u64, 1, payload);
    let mut sink = RingTraceSink::new(16);
    let resp = router.dispatch(CallerId(1), granted, &req, &mut sink);
    assert!(
        resp.error.is_some(),
        "command id {id} must be denied when capability is granted but allowlist/scope fails"
    );
    assert_eq!(
        resp.error.as_ref().unwrap().code,
        ErrorCode::ScopeDenied,
        "command id {id} denied for the wrong reason (expected ScopeDenied, got {:?})",
        resp.error
    );
}

// ---------------------------------------------------------------------------
// Gate 1: empty capabilities -> Unauthorized (not double-gating by itself)
// ---------------------------------------------------------------------------

#[test]
fn every_command_denied_without_capabilities() {
    let router = full_router();
    let empty = CapabilityBits::empty();
    let caller = CallerId(1);

    for id in 1u32..=74 {
        let req = WireRequest::new(id, id as u64, 1, json!(null));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(caller, &empty, &req, &mut sink);
        assert!(
            resp.error.is_some(),
            "command id {id} MUST be denied with empty capabilities",
        );
        assert_eq!(
            resp.error.as_ref().unwrap().code,
            ErrorCode::Unauthorized,
            "command id {id} denied for the wrong reason (expected Unauthorized)",
        );
    }
}

#[test]
fn ping_allowed_only_with_ping_capability() {
    let router = full_router();

    let denied = router.dispatch(
        CallerId(1),
        &CapabilityBits::empty(),
        &WireRequest::new(command_id::PING, 1, 1, json!(null)),
        &mut RingTraceSink::new(16),
    );
    assert!(denied.error.is_some());

    let mut caps = CapabilityBits::empty();
    caps.set(capability_bit::PING);
    let allowed = router.dispatch(
        CallerId(1),
        &caps,
        &WireRequest::new(command_id::PING, 2, 1, json!({ "hello": "world" })),
        &mut RingTraceSink::new(16),
    );
    assert!(allowed.error.is_none(), "ping must succeed with PING capability: {:?}", allowed.error);
    assert_eq!(allowed.payload.as_ref().unwrap()["echo"], json!({ "hello": "world" }));
}

// ---------------------------------------------------------------------------
// Gate 2: capability granted + empty allowlist -> ScopeDenied
// ---------------------------------------------------------------------------

#[test]
fn http_get_denied_when_capability_granted_but_host_allowlist_empty() {
    let router = full_router();
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::HTTP]),
        command_id::HTTP_GET,
        json!({ "url": "http://api.example.com/v1" }),
    );
}

#[test]
fn shell_run_denied_when_capability_granted_but_command_allowlist_empty() {
    let router = full_router();
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::SHELL]),
        command_id::SHELL_RUN,
        json!({ "program": "/usr/bin/echo", "args": ["hello"] }),
    );
}

#[test]
fn event_publish_denied_when_capability_granted_but_channel_allowlist_empty() {
    let router = full_router();
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::EVENT]),
        command_id::EVENT_PUBLISH,
        json!({ "event": "ping", "payload": { "x": 1 } }),
    );
}

#[test]
fn event_subscribe_denied_when_capability_granted_but_channel_allowlist_empty() {
    let router = full_router();
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::EVENT]),
        command_id::EVENT_SUBSCRIBE,
        json!({ "event": "ping" }),
    );
}

#[test]
fn opener_open_denied_when_capability_granted_but_scheme_allowlist_empty() {
    let router = full_router();
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::OPENER]),
        command_id::OPENER_OPEN,
        json!({ "target": "https://kiri.dev" }),
    );
}

#[test]
fn fs_read_denied_when_capability_granted_but_path_scope_flags_deny() {
    // Default seeds leave PathScope.read = false / write = false. A path under
    // the temp root still fails the second gate on access flags.
    let router = full_router();
    let path = std::env::temp_dir().join("kiri-double-gate-probe.txt");
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::FS]),
        command_id::FS_READ,
        json!({ "path": path.to_string_lossy() }),
    );
}

// ---------------------------------------------------------------------------
// Gate 2: capability granted + wrong allowlist -> ScopeDenied
// ---------------------------------------------------------------------------

#[test]
fn http_get_denied_when_capability_granted_but_host_not_on_allowlist() {
    let router = full_router_with(AllowlistSeeds {
        http_hosts: vec!["allowed.example.com".to_string()],
        ..AllowlistSeeds::default()
    });
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::HTTP]),
        command_id::HTTP_GET,
        json!({ "url": "http://evil.example.net/exfil" }),
    );
}

#[test]
fn shell_run_denied_when_capability_granted_but_program_not_on_allowlist() {
    let router = full_router_with(AllowlistSeeds {
        shell_commands: vec![AllowedCommand {
            program: "/usr/bin/echo".to_string(),
            args: vec!["hello".to_string()],
        }],
        ..AllowlistSeeds::default()
    });
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::SHELL]),
        command_id::SHELL_RUN,
        json!({ "program": "/bin/sh", "args": ["-c", "id"] }),
    );
}

#[test]
fn event_publish_denied_when_capability_granted_but_channel_not_on_allowlist() {
    let router = full_router_with(AllowlistSeeds {
        event_channels: vec![AllowedChannel { name: "ping".to_string() }],
        ..AllowlistSeeds::default()
    });
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::EVENT]),
        command_id::EVENT_PUBLISH,
        json!({ "event": "secrets", "payload": { "token": "x" } }),
    );
}

#[test]
fn opener_open_denied_when_capability_granted_but_scheme_not_on_allowlist() {
    let router = full_router_with(AllowlistSeeds {
        opener_url_schemes: vec![AllowedUrlScheme { scheme: "https".to_string() }],
        opener_file_extensions: vec![AllowedFileExtension { extension: "pdf".to_string() }],
        ..AllowlistSeeds::default()
    });
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::OPENER]),
        command_id::OPENER_OPEN,
        json!({ "target": "ssh://host" }),
    );
}

#[test]
fn opener_open_file_denied_when_capability_granted_but_extension_not_on_allowlist() {
    let router = full_router_with(AllowlistSeeds {
        opener_url_schemes: vec![AllowedUrlScheme { scheme: "https".to_string() }],
        opener_file_extensions: vec![AllowedFileExtension { extension: "pdf".to_string() }],
        ..AllowlistSeeds::default()
    });
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::OPENER]),
        command_id::OPENER_OPEN,
        json!({ "target": "/tmp/run.exe" }),
    );
}

#[test]
fn fs_read_denied_when_capability_granted_but_path_escapes_scope() {
    let router = full_router_with(AllowlistSeeds {
        fs_read: true,
        fs_write: false,
        ..AllowlistSeeds::default()
    });
    // Absolute path outside the temp-dir PathScope root.
    assert_scope_denied(
        &router,
        &caps(&[capability_bit::FS]),
        command_id::FS_READ,
        json!({ "path": "/etc/passwd" }),
    );
}
