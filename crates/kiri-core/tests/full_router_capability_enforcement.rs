//! DECISIVE double-gating enforcement test (headless, macOS-runnable).
//!
//! Builds the FULL production-equivalent router: every with_* surface wired
//! with the same simple service constructors the native host uses (no runtime
//! or host handles), then dispatches every command id 1..=61 with an EMPTY
//! capability set and asserts each is DENIED. This proves, deterministically
//! and without launching a WebView, that the central capability authority is
//! the one gate on every control-plane command - the security axis on which
//! Kiri exceeds Tauri (Tauri gates individual plugins but a granted plugin
//! capability is effectively all-or-nothing; Kiri denies everything by default
//! and grants nothing unless the native host assigns the exact bit).

use std::sync::Arc;

use kiri_core::caller::CallerId;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::dispatch::{command_id, Router};
use kiri_core::trace::RingTraceSink;
use kiri_core::wire::WireRequest;
use serde_json::json;

// Stubs for host-owned backends. They are never invoked because the capability
// check rejects the dispatch before any handler runs. They exist only so the
// router can be constructed headlessly.

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

fn full_router() -> Router {
    let limits = kiri_core::limits::Limits::default();
    let caller = kiri_core::caller::CallerRegistry::new().register();
    let diag = kiri_core::diagnostics::Diagnostics::new();

    Router::new()
        .with_diagnostics(diag.clone())
        .with_resources(diag.clone(), caller)
        .with_platform(kiri_core::platform::EventBus::new())
        .with_fs_service(kiri_core::fs::FsService::new(
            kiri_core::capabilities::PathScope::new(std::env::temp_dir()),
            limits.clone(),
        ))
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
            kiri_core::http::HostAllowlist::new(vec![]),
            limits.clone(),
        ))
        .with_shell(kiri_core::shell::ShellService::new(
            Arc::new(StubShellRunner),
            kiri_core::shell::ShellAllowlist::new(vec![]),
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
            kiri_core::opener::OpenerAllowlist::new(vec![], vec![]),
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
            kiri_core::event::EventAllowlist::new(vec![]),
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
            "command id {id} MUST be denied with empty capabilities (double-gating violation)",
        );
        assert_eq!(
            resp.error.as_ref().unwrap().code,
            kiri_core::error::ErrorCode::Unauthorized,
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
    caps.set(kiri_core::dispatch::capability_bit::PING);
    let allowed = router.dispatch(
        CallerId(1),
        &caps,
        &WireRequest::new(command_id::PING, 2, 1, json!({ "hello": "world" })),
        &mut RingTraceSink::new(16),
    );
    assert!(allowed.error.is_none(), "ping must succeed with PING capability: {:?}", allowed.error);
    assert_eq!(allowed.payload.as_ref().unwrap()["echo"], json!({ "hello": "world" }));
}
