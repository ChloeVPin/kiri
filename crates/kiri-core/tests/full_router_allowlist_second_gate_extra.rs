//! Gate-2 (host allowlist) enforcement for the double-gated surfaces that
//! `full_router_capability_enforcement.rs` does not cover. That file proves
//! gate 1 (an empty capability set denies every command with `Unauthorized`)
//! and, on its PR branch, gate 2 for http/shell/event/fs/opener. This file
//! builds the same host-equivalent router the native host wires in
//! `kiri_runtime::host_cross::build_host_router` (the same `with_*` service
//! constructors, with stub backends standing in for the OS runners), grants the
//! exact capability bit, and proves the host allowlist still denies the call
//! with `ScopeDenied` when the list is empty or holds only decoy entries.
//!
//! Covered surfaces (capability bit AND host allowlist, both required): store
//! namespaces, notification templates, dialog kinds, shortcut accelerators,
//! autostart host policy, deeplink schemes, tray item ids, sidecar names,
//! config keys, fs-watch targets, websocket URLs, menu item ids.
//!
//! Intentionally single-gate (capability bit only, no host allowlist exists by
//! design): clipboard read/write, path/os helpers, window controls. Structural
//! second gates, where the host-owned target is fixed so there is no allowlist
//! for the frontend to miss: window_state (reserved namespace), updater
//! (host-pinned Ed25519 key), plugin_list (host-owned inventory). `cli.args`
//! has a projection gate instead of a deny gate: undeclared flags/options are
//! filtered out of the response, covered below. The legacy EVENT_EMIT /
//! EVENT_LISTEN commands (ids 8/9) are capability-only on this branch; the
//! channel-allowlisted `kiri.event.*` surface (ids 56-58) is covered by the
//! sibling suite.

use std::sync::Arc;

use kiri_core::app_menu::MenuItem;
use kiri_core::caller::CallerId;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::config::AllowedConfigKey;
use kiri_core::deeplink::DeeplinkScheme;
use kiri_core::dialog::{DialogKind, DialogTemplate};
use kiri_core::dispatch::{capability_bit, command_id, Router};
use kiri_core::error::ErrorCode;
use kiri_core::fs_watch::{WatchKind, WatchTarget};
use kiri_core::notification::NotificationTemplate;
use kiri_core::shortcut::ShortcutBinding;
use kiri_core::sidecar::AllowedSidecar;
use kiri_core::store::StoreNamespace;
use kiri_core::trace::RingTraceSink;
use kiri_core::tray::TrayItem;
use kiri_core::wire::{WireRequest, WireResponse};
use serde_json::{json, Value};

// Stubs for host-owned backends. On the deny path the allowlist check rejects
// the dispatch before the runner runs; on the allow path the stub answers so
// the positive control proves the request passed both gates.

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

/// The host-owned allowlist contents for every double-gated surface under
/// test. `empty` proves default-deny; `decoy` proves the gate is an exact
/// host-owned match (a populated list still refuses an unlisted request while
/// serving the listed one); `granted` is the positive control.
struct Allow {
    notifications: Vec<NotificationTemplate>,
    dialogs: Vec<DialogTemplate>,
    shortcuts: Vec<ShortcutBinding>,
    autostart_permitted: bool,
    store_namespaces: Vec<StoreNamespace>,
    deeplinks: Vec<DeeplinkScheme>,
    tray_items: Vec<TrayItem>,
    sidecars: Vec<AllowedSidecar>,
    config_keys: Vec<AllowedConfigKey>,
    watch_targets: Vec<WatchTarget>,
    ws_urls: Vec<String>,
    menu_items: Vec<MenuItem>,
}

fn empty_allowlists() -> Allow {
    Allow {
        notifications: vec![],
        dialogs: vec![],
        shortcuts: vec![],
        autostart_permitted: false,
        store_namespaces: vec![],
        deeplinks: vec![],
        tray_items: vec![],
        sidecars: vec![],
        config_keys: vec![],
        watch_targets: vec![],
        ws_urls: vec![],
        menu_items: vec![],
    }
}

/// Populated allowlists whose entries never match the fixture probes, so a
/// denial cannot be attributed to an empty list.
fn decoy_allowlists() -> Allow {
    Allow {
        notifications: vec![NotificationTemplate {
            id: "other-template".to_string(),
            title: "Other".to_string(),
            body: "Other body".to_string(),
            args: 0,
        }],
        dialogs: vec![DialogTemplate {
            kind: DialogKind::Confirm,
            title_template: "Confirm?".to_string(),
            args: 0,
            filters: vec![],
        }],
        shortcuts: vec![ShortcutBinding {
            accelerator: "CmdOrCtrl+K".to_string(),
            action: "palette".to_string(),
        }],
        autostart_permitted: false,
        store_namespaces: vec![StoreNamespace { prefix: "other.ns".to_string() }],
        deeplinks: vec![DeeplinkScheme { scheme: "other-scheme".to_string() }],
        tray_items: vec![TrayItem {
            id: "show".to_string(),
            label: "Show".to_string(),
            action: "show".to_string(),
        }],
        sidecars: vec![AllowedSidecar { name: "indexer".to_string(), args: vec![] }],
        config_keys: vec![AllowedConfigKey { key: "window.theme".to_string() }],
        watch_targets: vec![WatchTarget { path: "/data/other".to_string(), kind: WatchKind::All }],
        ws_urls: vec!["wss://other.example.com/x".to_string()],
        menu_items: vec![MenuItem {
            id: "new".to_string(),
            label: "New".to_string(),
            action: "new".to_string(),
        }],
    }
}

/// Populated allowlists holding exactly the fixture entries the probes target.
fn granted_allowlists() -> Allow {
    Allow {
        notifications: vec![NotificationTemplate {
            id: "download-complete".to_string(),
            title: "Download finished: {0}".to_string(),
            body: "Saved".to_string(),
            args: 1,
        }],
        dialogs: vec![DialogTemplate {
            kind: DialogKind::Message,
            title_template: "Notice".to_string(),
            args: 0,
            filters: vec![],
        }],
        shortcuts: vec![ShortcutBinding {
            accelerator: "CmdOrCtrl+S".to_string(),
            action: "save".to_string(),
        }],
        autostart_permitted: true,
        store_namespaces: vec![StoreNamespace { prefix: "app.prefs".to_string() }],
        deeplinks: vec![DeeplinkScheme { scheme: "kiri-app".to_string() }],
        tray_items: vec![TrayItem {
            id: "quit".to_string(),
            label: "Quit".to_string(),
            action: "quit".to_string(),
        }],
        sidecars: vec![AllowedSidecar { name: "helper".to_string(), args: vec![] }],
        config_keys: vec![AllowedConfigKey { key: "app.name".to_string() }],
        watch_targets: vec![WatchTarget { path: "/data/app".to_string(), kind: WatchKind::All }],
        ws_urls: vec!["wss://api.example.com/feed".to_string()],
        menu_items: vec![MenuItem {
            id: "quit".to_string(),
            label: "Quit".to_string(),
            action: "quit".to_string(),
        }],
    }
}

/// Host-equivalent router: the same `with_*` chain `build_host_router` uses on
/// the native host, with stub backends and the allowlists under test. Surfaces
/// whose gate-2 coverage lives in the sibling suite (http, shell, opener,
/// event) are wired with empty allowlists so the router stays the full
/// production shape.
fn host_router(allow: &Allow) -> Router {
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
            kiri_core::notification::NotificationAllowlist::new(allow.notifications.clone()),
            limits.clone(),
        ))
        .with_dialog(kiri_core::dialog::DialogService::new(
            Arc::new(StubDialogRunner),
            kiri_core::dialog::DialogAllowlist::new(allow.dialogs.clone()),
            limits.clone(),
        ))
        .with_shortcut(kiri_core::shortcut::ShortcutService::new(
            Arc::new(StubShortcutRunner),
            kiri_core::shortcut::ShortcutAllowlist::new(allow.shortcuts.clone()),
            limits.clone(),
        ))
        .with_autostart(kiri_core::autostart::AutostartService::new(
            Arc::new(StubAutostartRunner),
            kiri_core::autostart::AutostartAllowlist::new(allow.autostart_permitted),
            limits.clone(),
        ))
        .with_store(kiri_core::store::StoreService::new(
            Arc::new(StubStoreBackend),
            kiri_core::store::StoreAllowlist::new(allow.store_namespaces.clone()),
            limits.clone(),
        ))
        .with_deeplink(kiri_core::deeplink::DeeplinkService::new(
            Arc::new(StubDeeplinkRunner),
            kiri_core::deeplink::DeeplinkAllowlist::new(allow.deeplinks.clone()),
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
            kiri_core::tray::TrayAllowlist::new(allow.tray_items.clone()),
            limits.clone(),
        ))
        .with_sidecar(kiri_core::sidecar::SidecarService::new(
            Arc::new(StubSidecarRunner),
            kiri_core::sidecar::SidecarAllowlist::new(allow.sidecars.clone()),
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
            kiri_core::config::ConfigAllowlist::new(allow.config_keys.clone()),
            limits.clone(),
        ))
        .with_updater(kiri_core::updater_surface::UpdaterService::new(
            "00".to_string(),
            kiri_core::update::Version::parse("0.0.0").unwrap(),
            limits.clone(),
        ))
        .with_cli(kiri_core::cli::CliService::new(vec![
            "kiri-test".to_string(),
            "--secret=s3cr3t".to_string(),
            "--verbose".to_string(),
        ]))
        .with_fs_watch(kiri_core::fs_watch::FsWatchService::new(
            Arc::new(StubFsWatch),
            kiri_core::fs_watch::FsWatchAllowlist::new(allow.watch_targets.clone()),
            limits.clone(),
        ))
        .with_ws(kiri_core::websocket::WsService::new(
            Arc::new(StubWs),
            kiri_core::websocket::WsAllowlist::new(allow.ws_urls.clone()),
            limits.clone(),
        ))
        .with_menu(kiri_core::app_menu::MenuService::new(
            Arc::new(StubMenu),
            kiri_core::app_menu::MenuAllowlist::new(allow.menu_items.clone()),
            limits.clone(),
        ))
        .with_plugin_inventory(kiri_core::plugin_inventory::PluginInventory::empty())
}

/// One gate-2 probe: a command id, the capability bit that passes gate 1, and
/// two payload shapes. `granted` addresses the fixture entry installed by
/// `granted_allowlists`; `decoy` addresses the entry installed by
/// `decoy_allowlists`.
struct Case {
    name: &'static str,
    id: u32,
    bit: u32,
    granted: Value,
    decoy: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "store.get",
            id: command_id::STORE_GET,
            bit: capability_bit::STORE,
            granted: json!({ "namespace": "app.prefs", "key": "theme" }),
            decoy: json!({ "namespace": "other.ns", "key": "theme" }),
        },
        Case {
            name: "store.set",
            id: command_id::STORE_SET,
            bit: capability_bit::STORE,
            granted: json!({ "namespace": "app.prefs", "key": "theme", "value": "dark" }),
            decoy: json!({ "namespace": "other.ns", "key": "theme", "value": "dark" }),
        },
        Case {
            name: "notification.show",
            id: command_id::NOTIFY,
            bit: capability_bit::NOTIFICATION,
            granted: json!({ "template": "download-complete", "args": ["report.pdf"] }),
            decoy: json!({ "template": "other-template", "args": [] }),
        },
        Case {
            name: "dialog.open",
            id: command_id::DIALOG_OPEN,
            bit: capability_bit::DIALOG,
            granted: json!({ "kind": "message", "args": [] }),
            decoy: json!({ "kind": "confirm", "args": [] }),
        },
        Case {
            name: "shortcut.register",
            id: command_id::SHORTCUT_REGISTER,
            bit: capability_bit::SHORTCUT,
            granted: json!({ "accelerator": "CmdOrCtrl+S" }),
            decoy: json!({ "accelerator": "CmdOrCtrl+K" }),
        },
        Case {
            name: "deeplink.register",
            id: command_id::DEEPLINK_REGISTER,
            bit: capability_bit::DEEPLINK,
            granted: json!({ "scheme": "kiri-app" }),
            decoy: json!({ "scheme": "other-scheme" }),
        },
        Case {
            name: "tray.setMenu",
            id: command_id::TRAY_SET_MENU,
            bit: capability_bit::TRAY,
            granted: json!({ "ids": ["quit"] }),
            decoy: json!({ "ids": ["show"] }),
        },
        Case {
            name: "tray.invoke",
            id: command_id::TRAY_INVOKE,
            bit: capability_bit::TRAY,
            granted: json!({ "id": "quit" }),
            decoy: json!({ "id": "show" }),
        },
        Case {
            name: "sidecar.spawn",
            id: command_id::SIDECAR_SPAWN,
            bit: capability_bit::SIDECAR,
            granted: json!({ "name": "helper" }),
            decoy: json!({ "name": "indexer" }),
        },
        Case {
            name: "config.get",
            id: command_id::CONFIG_GET,
            bit: capability_bit::CONFIG,
            granted: json!({ "key": "app.name" }),
            decoy: json!({ "key": "window.theme" }),
        },
        Case {
            name: "fs.watch",
            id: command_id::FS_WATCH,
            bit: capability_bit::FS,
            granted: json!({ "path": "/data/app" }),
            decoy: json!({ "path": "/data/other" }),
        },
        Case {
            name: "ws.connect",
            id: command_id::WS_CONNECT,
            bit: capability_bit::WS,
            granted: json!({ "url": "wss://api.example.com/feed" }),
            decoy: json!({ "url": "wss://other.example.com/x" }),
        },
        Case {
            name: "menu.set",
            id: command_id::MENU_SET,
            bit: capability_bit::MENU,
            granted: json!({ "ids": ["quit"] }),
            decoy: json!({ "ids": ["new"] }),
        },
        Case {
            name: "menu.invoke",
            id: command_id::MENU_INVOKE,
            bit: capability_bit::MENU,
            granted: json!({ "id": "quit" }),
            decoy: json!({ "id": "new" }),
        },
    ]
}

fn granted(bit: u32) -> CapabilityBits {
    let mut caps = CapabilityBits::empty();
    caps.set(bit);
    caps
}

fn call(router: &Router, caps: &CapabilityBits, id: u32, payload: Value) -> WireResponse {
    let req = WireRequest::new(id, 1, 1, payload);
    router.dispatch(CallerId(1), caps, &req, &mut RingTraceSink::new(16))
}

fn code(resp: &WireResponse) -> Option<ErrorCode> {
    resp.error.as_ref().map(|e| e.code)
}

#[test]
fn gate2_empty_allowlist_denies_after_capability_grant() {
    let router = host_router(&empty_allowlists());
    for case in cases() {
        for (label, payload) in [("fixture", &case.granted), ("decoy", &case.decoy)] {
            let resp = call(&router, &granted(case.bit), case.id, payload.clone());
            assert_eq!(
                code(&resp),
                Some(ErrorCode::ScopeDenied),
                "{} ({label}): granted capability must still hit the allowlist gate, got {:?}",
                case.name,
                resp.error,
            );
        }
    }
}

#[test]
fn gate2_decoy_allowlist_denies_unlisted_but_serves_listed() {
    let router = host_router(&decoy_allowlists());
    for case in cases() {
        let denied = call(&router, &granted(case.bit), case.id, case.granted.clone());
        assert_eq!(
            code(&denied),
            Some(ErrorCode::ScopeDenied),
            "{}: a populated allowlist must still refuse the unlisted fixture, got {:?}",
            case.name,
            denied.error,
        );
        let allowed = call(&router, &granted(case.bit), case.id, case.decoy.clone());
        assert!(
            allowed.error.is_none(),
            "{}: the listed decoy entry must pass both gates, got {:?}",
            case.name,
            allowed.error,
        );
    }
}

#[test]
fn gate2_matching_allowlist_entry_passes_both_gates() {
    let router = host_router(&granted_allowlists());
    for case in cases() {
        let resp = call(&router, &granted(case.bit), case.id, case.granted.clone());
        assert!(
            resp.error.is_none(),
            "{}: fixture entry on the allowlist must succeed, got {:?}",
            case.name,
            resp.error,
        );
    }
}

#[test]
fn gate1_runs_before_gate2() {
    // Same empty-allowlist router: without the capability bit the denial is
    // Unauthorized (gate 1); with the bit the denial is ScopeDenied (gate 2).
    // The two codes prove the gates are distinct and ordered.
    let router = host_router(&empty_allowlists());
    for case in cases() {
        let resp = call(&router, &CapabilityBits::empty(), case.id, case.granted.clone());
        assert_eq!(
            code(&resp),
            Some(ErrorCode::Unauthorized),
            "{}: missing capability must deny at gate 1, got {:?}",
            case.name,
            resp.error,
        );
    }
}

#[test]
fn autostart_host_policy_denies_even_with_capability() {
    // The autostart second gate is a host policy boolean, not a list: denied
    // policy plus a granted capability must still refuse both set and get.
    let denied = host_router(&empty_allowlists());
    let set = call(
        &denied,
        &granted(capability_bit::AUTOSTART),
        command_id::AUTOSTART_SET,
        json!({ "enabled": true }),
    );
    assert_eq!(code(&set), Some(ErrorCode::ScopeDenied));
    let get =
        call(&denied, &granted(capability_bit::AUTOSTART), command_id::AUTOSTART_GET, json!({}));
    assert_eq!(code(&get), Some(ErrorCode::ScopeDenied));

    let permitted = host_router(&granted_allowlists());
    let ok = call(
        &permitted,
        &granted(capability_bit::AUTOSTART),
        command_id::AUTOSTART_SET,
        json!({ "enabled": true }),
    );
    assert!(ok.error.is_none(), "permitted policy must pass both gates: {:?}", ok.error);
}

#[test]
fn cli_args_second_gate_is_host_allowlist_projection() {
    // cli.args never denies on its allowlist; the host-owned projection drops
    // undeclared flags/options instead. The fixture argv carries --verbose and
    // --secret=s3cr3t; with an empty allowlist neither may reach the
    // structured view returned to the frontend.
    let router = host_router(&empty_allowlists());
    let resp = call(&router, &granted(capability_bit::CLI), command_id::CLI_ARGS, json!({}));
    assert!(resp.error.is_none(), "cli.args must pass with the CLI bit: {:?}", resp.error);
    let payload = resp.payload.as_ref().unwrap();
    assert_eq!(payload["flags"], json!([]));
    assert_eq!(payload["options"], json!({}));
}
