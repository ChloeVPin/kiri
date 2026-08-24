//! Cross-platform host backend (macOS, Linux) built on `wry` + `tao`.
//!
//! This backend hosts the same blank frontend and records the same nine
//! startup markers on a monotonic clock, so the benchmark compares like for
//! like across the direct host and the wry/tao baseline. It shares the smoke
//! contract: exit 0 after `first_animation_frame` plus `--exit-after-ready-ms`,
//! exit 2 on watchdog.
//!
//! The WebView is created on the main thread, which is also where wry/tao
//! dispatch their events. The bridge script installed at document start posts
//! ready-phase markers over `window.chrome.webview.postMessage` (WebView2),
//! `window.ipc.postMessage` (wry), or the Tauri internals; on wry the
//! `with_ipc_handler` receives those messages and drives the markers.

#![cfg(not(target_os = "windows"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::borrow::Cow;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::http::header;
use wry::http::Response as WryResponse;
use wry::{PageLoadEvent, WebViewBuilder};

use kiri_core::caller::CallerRegistry;
use kiri_core::diagnostics::Diagnostics;
use kiri_core::resources::ResourceTable;
use kiri_core::security::{is_app_origin, is_navigation_allowed};
use kiri_core::wire::{WireRequest, WireResponse};

use crate::markers::{Marker, StartupMarkers};
use crate::output::write_startup_result;
use crate::HostOptions;

/// Serve one `kiri://localhost/<path>` request.
///
/// Pure window-free logic lives in `crate::assets`; this wrapper adapts it to
/// wry's `http::Response`. The frontend directory comes from
/// `HostOptions.frontend_dir`; if absent, the compile-time packed frontend
/// is served (including sub-assets). `Range` requests are
/// honored so large assets load incrementally (F-1 in the deep audit).
fn serve_kiri(
    options: &HostOptions,
    request_path: &str,
    range: Option<&str>,
    if_none_match: Option<&str>,
) -> WryResponse<Cow<'static, [u8]>> {
    use crate::assets::{
        response_headers, serve_checked, status_code, AssetResponse, ServeOptions,
    };
    let opts = ServeOptions { range, if_none_match, allow: &[] };
    let resp = match options.frontend_dir.as_ref() {
        Some(root) => serve_checked(root, request_path, &opts),
        None => crate::assets::serve_embedded(request_path, &opts),
    };
    let status = status_code(&resp);
    let mut builder = WryResponse::builder().status(status);
    for (k, v) in response_headers(&resp) {
        builder = builder.header(k, v);
    }
    let body: Cow<'static, [u8]> = match &resp {
        AssetResponse::Full { body, .. } | AssetResponse::Partial { body, .. } => {
            Cow::Owned(body.clone())
        }
        _ => Cow::Borrowed(b"".as_slice()),
    };
    builder.body(body).unwrap()
}
/// Post a control-plane response back to the page via the shared webview
/// slot. Best-effort: if the webview is not ready yet, the message is dropped
/// (the page can re-issue). Responses carry the request id for correlation.
fn post_response(slot: &Rc<RefCell<Option<wry::WebView>>>, response: &WireResponse) {
    if let Some(webview) = slot.borrow().as_ref() {
        let js = format!(
            "window.kiri && window.kiri.onResponse && window.kiri.onResponse({});",
            serde_json::to_string(response).unwrap_or_default()
        );
        let _ = webview.evaluate_script(&js);
    }
}

/// Bridge script injected at document start. It only fires on the
/// application origin and uses whichever native bridge the host exposes.
///
/// It posts the startup `ready` markers and also installs `window.kiri.send`,
/// which the frontend uses to issue control-plane commands. The host answers
/// each `cmd` message with a `cmd_response` message carrying the same request
/// id so the page can correlate responses (T003).
const BRIDGE_SCRIPT: &str = r#"
    (function () {
      if (window.kiri) { return; }
      function post(o) {
        var s = JSON.stringify(o);
        if (window.chrome && window.chrome.webview && window.chrome.webview.postMessage) {
          window.chrome.webview.postMessage(s);
        } else if (window.ipc && window.ipc.postMessage) {
          window.ipc.postMessage(s);
        } else if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
          window.__TAURI_INTERNALS__.invoke('kiri_marker', { json: s });
        }
      }
      window.addEventListener('DOMContentLoaded', function () {
        post({ type: 'ready', phase: 'dom' });
      });
      requestAnimationFrame(function () {
        post({ type: 'ready', phase: 'frame' });
      });
      window.kiri = {
        pending: Object.create(null),
        send: function (req) { post({ type: 'cmd', request: req }); },
        onResponse: function (resp) {
          var id = resp && resp.request_id;
          var cb = window.kiri.pending[id];
          if (cb) {
            delete window.kiri.pending[id];
            cb(resp);
          }
        }
      };
    })();
"#;

/// The cross-platform host: window, WebView, event loop, and marker capture.
pub struct CrossHost;

impl CrossHost {
    /// Run one full session and record the startup markers.
    ///
    /// On macOS/Linux `tao::EventLoop::run` does not return; the host emits
    /// the startup result and exits the process directly (matching the wry/tao
    /// baseline). The function's return type is preserved for the shared
    /// facade, but in practice the smoke/watchdog paths terminate via
    /// `std::process::exit`.
    pub fn run(options: &HostOptions) -> Result<StartupMarkers, i32> {
        run_inner(options.clone())
    }
}

/// Monotonic clock in nanoseconds, relative to process start.
fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static T0: OnceLock<Instant> = OnceLock::new();
    let t0 = *T0.get_or_init(Instant::now);
    Instant::now().duration_since(t0).as_nanos() as u64
}

/// Record one marker into the shared map.
fn record(markers: &Rc<RefCell<StartupMarkers>>, marker: Marker) {
    markers.borrow_mut().record(marker, now_ns());
}

/// Host allowlist for tray menu item ids for the native tray (audit item 14).
/// Only these ids may appear in the native menu; labels and actions are host-owned.
fn tray_items() -> Vec<kiri_core::tray::TrayItem> {
    vec![
        kiri_core::tray::TrayItem {
            id: "show".to_string(),
            label: "Show Window".to_string(),
            action: "show".to_string(),
        },
        kiri_core::tray::TrayItem {
            id: "quit".to_string(),
            label: "Quit".to_string(),
            action: "quit".to_string(),
        },
    ]
}

/// Build the production control-plane router shared by the live host and
/// the registration regression test. Takes the window and clipboard
/// controllers so the test can pass headless no-op stubs without opening a
/// WebView. Returns the fully-wired router: every catalog command id must
/// resolve on this exact construction.
pub(crate) fn build_host_router(
    window: std::sync::Arc<dyn kiri_core::window::WindowController>,
    clipboard_ctrl: std::sync::Arc<dyn kiri_core::clipboard::ClipboardController>,
    diagnostics: &Diagnostics,
    resources: &std::sync::Arc<Mutex<kiri_core::resources::ResourceTable<()>>>,
    options: &HostOptions,
) -> kiri_core::dispatch::Router {
    let events = kiri_core::platform::EventBus::new();
    let caller = CallerRegistry::new().register();
    // Host-owned fs scope: a bounded sandbox under the temp dir. The host is
    // the only authority that can widen it; the frontend cannot.
    let mut fs_scope =
        kiri_core::capabilities::PathScope::new(std::env::temp_dir().join("kiri-fs"));
    fs_scope.read = true;
    fs_scope.write = true;
    let _ = std::fs::create_dir_all(&fs_scope.root);
    crate::plugins::PluginHost::build_router_with_plugins(
        diagnostics,
        resources,
        caller,
        &crate::plugins::PluginManifest::empty(),
        &crate::plugins::PluginRegistry::empty(),
    )
    // R-3: JS-surface commands (kiri.platform.*, kiri.app.*, kiri.event.*).
    .with_platform(events.clone())
    .with_fs_service(
        kiri_core::fs::FsService::new(fs_scope, kiri_core::limits::Limits::default()).with_glob(
            kiri_core::capabilities::GlobScope::new(crate::host_policy::fs_glob_patterns()),
        ),
    )
    // G-5: kiri.window.* surface backed by the real native window.
    .with_window(window, Arc::new(Mutex::new(kiri_core::window::WindowState::new(&options.title))))
    // G-6: kiri.clipboard.* surface backed by the real OS clipboard.
    .with_clipboard(
        clipboard_ctrl,
        Arc::new(Mutex::new(kiri_core::clipboard::ClipboardState::new())),
    )
    // G-7: kiri.path.* / kiri.os.* surface (audit item 2). Pure path
    // math plus read-only OS directory discovery, capability-gated (PATH).
    .with_path(kiri_core::path::PathService::new(kiri_core::path::PathState::new()))
    // G-3: kiri.http.get surface (audit item 3). Capability-gated (HTTP) and
    // constrained to a host allowlist so a granted capability still cannot
    // reach an unapproved origin; responses are bulk-capped like kiri.fs.
    .with_http(kiri_core::http::HttpService::new(
        std::sync::Arc::new(kiri_core::http::StdHttpClient),
        kiri_core::http::HostAllowlist::new(crate::host_policy::http_allow_hosts()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4: kiri.shell.run surface (audit item 4). Capability-gated (SHELL)
    // and constrained to a host allowlist so a granted capability still
    // cannot spawn an unapproved program; output is bulk-capped like kiri.fs.
    .with_shell(kiri_core::shell::ShellService::new(
        std::sync::Arc::new(crate::shell_ctl::CrossShellRunner::new()),
        kiri_core::shell::ShellAllowlist::new(crate::host_policy::shell_allow_commands()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4b: kiri.notification.show surface (audit item 5). Capability-gated
    // (NOTIFICATION) and constrained to a host template allowlist so a
    // granted capability still cannot render arbitrary title/body; only
    // pre-approved templates with bounded args may show.
    .with_notification(kiri_core::notification::NotificationService::new(
        std::sync::Arc::new(crate::notification_ctl::cross_notify::CrossNotificationRunner::new()),
        kiri_core::notification::NotificationAllowlist::new(
            crate::host_policy::notification_templates(),
        ),
        kiri_core::limits::Limits::default(),
    ))
    // G-4c: kiri.dialog.open surface (audit item 7). Capability-gated
    // (DIALOG) and constrained to a host allowlist of dialog kinds with a
    // host-owned title, so a granted capability still cannot open an
    // arbitrary native prompt; only pre-approved dialog kinds may show.
    .with_dialog(kiri_core::dialog::DialogService::new(
        std::sync::Arc::new(crate::dialog_ctl::CrossDialogRunner::new()),
        kiri_core::dialog::DialogAllowlist::new(crate::host_policy::dialog_templates()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4d: kiri.shortcut.register surface (audit item 8). Capability-gated
    // (SHORTCUT) and constrained to a host allowlist of exact accelerators mapped
    // to host-owned actions, so a granted capability still cannot register an
    // arbitrary global hotkey; only pre-approved accelerators may bind.
    .with_shortcut(kiri_core::shortcut::ShortcutService::new(
        std::sync::Arc::new(crate::shortcut_ctl::CrossShortcutRunner::new()),
        kiri_core::shortcut::ShortcutAllowlist::new(crate::host_policy::shortcut_bindings()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4e: kiri.autostart.set/get surface (audit item 9). Capability-gated
    // (AUTOSTART) and bounded to a host policy (default-deny). Even when the
    // policy permits it, the runner only registers the host's own binary, so a
    // granted capability still cannot persist an arbitrary executable. This
    // exceeds Tauri's autostart plugin, which lets the frontend enable login
    // launch freely once the capability is present.
    .with_autostart(kiri_core::autostart::AutostartService::new(
        std::sync::Arc::new(crate::autostart_ctl::CrossAutostartRunner::new()),
        kiri_core::autostart::AutostartAllowlist::new(crate::host_policy::autostart_policy()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4f: kiri.store.get/set surface (audit item 10). Capability-gated (STORE)
    // and bounded to a host allowlist of namespaces, so a granted capability still
    // cannot read/write outside an approved namespace. This exceeds Tauri's store
    // plugin, which lets the frontend read/write the whole store once the capability
    // is present (a cross-feature data-leak surface).
    .with_store(kiri_core::store::StoreService::new(
        std::sync::Arc::new(crate::store_ctl::CrossStoreBackend::new()),
        kiri_core::store::StoreAllowlist::new(crate::host_policy::store_namespaces()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4g: kiri.deeplink.register surface (audit item 11). Capability-gated
    // (DEEPLINK) and bounded to a host allowlist of exact schemes, so a granted
    // capability still cannot squat on an arbitrary URI scheme. This exceeds
    // Tauri's deep-link plugin, which lets the frontend register any scheme once
    // the capability is present (a scheme-squatting surface).
    .with_deeplink(kiri_core::deeplink::DeeplinkService::new(
        std::sync::Arc::new(crate::deeplink_ctl::cross_deeplink::CrossDeeplinkRunner::new()),
        kiri_core::deeplink::DeeplinkAllowlist::new(crate::host_policy::deeplink_schemes()),
        kiri_core::limits::Limits::default(),
    ))
    // G-2c: kiri.opener.open surface (audit item 12). Capability-gated (OPENER)
    // and bounded to a host allowlist of exact URL schemes and file extensions, so a
    // granted capability still cannot launch an arbitrary URL scheme or file. This
    // exceeds Tauri's opener plugin, which opens arbitrary URLs/files once the
    // capability is present (a scheme/file-launch surface).
    .with_opener(kiri_core::opener::OpenerService::new(
        std::sync::Arc::new(crate::opener_ctl::cross_opener::CrossOpenerRunner::new()),
        kiri_core::opener::OpenerAllowlist::new(
            crate::host_policy::opener_url_schemes(),
            crate::host_policy::opener_file_extensions(),
        ),
        kiri_core::limits::Limits::default(),
    ))
    // G-2d: kiri.window.state.save/load surface (audit item 13). Capability-gated
    // (WINDOW_STATE) and confined to a fixed, frontend-unaddressable host store, so a
    // granted capability still cannot read/write arbitrary state. This exceeds Tauri's
    // window-state plugin, which persists to a frontend-readable/writable JSON without a
    // second capability gate.
    .with_window_state(kiri_core::window_state::WindowStateService::new(
        std::sync::Arc::new(
            crate::window_state_ctl::cross_window_state::CrossWindowStateBackend::new(),
        ),
        kiri_core::limits::Limits::default(),
    ))
    // G-6: kiri.tray.setMenu/invoke surface (audit item 14). Capability-gated
    // (TRAY) and bounded to a host allowlist of item ids, so a granted capability
    // still cannot draw an arbitrary native menu. This exceeds Tauri's tray, which
    // lets the frontend build the native menu freely once the capability is present.
    .with_tray(kiri_core::tray::TrayService::new(
        std::sync::Arc::new(crate::tray_ctl::cross_tray::CrossTrayBackend::new()),
        kiri_core::tray::TrayAllowlist::new(tray_items()),
        kiri_core::limits::Limits::default(),
    ))
    // G-6: kiri.sidecar.spawn/stop/list surface (audit item 15). Capability-gated
    // (SIDECAR) and bounded to a host allowlist of exact sidecar names, so a
    // granted capability still cannot fork an unapproved binary or pass arbitrary
    // argv. This exceeds Tauri's sidecar API, which lets the frontend name an
    // arbitrary companion executable once the capability is present.
    .with_sidecar(kiri_core::sidecar::SidecarService::new(
        std::sync::Arc::new(crate::sidecar_ctl::cross_sidecar::CrossSidecarRunner::new()),
        kiri_core::sidecar::SidecarAllowlist::new(crate::host_policy::sidecar_allow()),
        kiri_core::sidecar::SidecarTable::new(),
        kiri_core::limits::Limits::default(),
    ))
    // audit-16: kiri.event.publish/subscribe/channels (restricted,
    // channel-allowlisted). Capability-gated (EVENT) and bounded to a host
    // allowlist of exact channel names, so a granted capability still cannot
    // forge or snoop cross-module events. This exceeds Tauri's unrestricted
    // event module on the security axis.
    .with_event(kiri_core::event::EventService::new(
        std::sync::Arc::new(events.clone()),
        kiri_core::event::EventAllowlist::new(crate::host_policy::event_channels()),
        kiri_core::limits::Limits::default(),
    ))
    // audit-17: kiri.config.get/keys (restricted, key-allowlisted). Capability-gated
    // (CONFIG) and bounded to a host allowlist of exact key paths, so a granted
    // capability still cannot read arbitrary host config. This exceeds Tauri's
    // unrestricted getConfig() on the security axis.
    .with_config(kiri_core::config::ConfigService::new(
        std::sync::Arc::new(kiri_core::config::MapConfigBackend::new({
            let mut m = std::collections::HashMap::new();
            m.insert("app.name".to_string(), serde_json::json!("Kiri"));
            m.insert("app.version".to_string(), serde_json::json!(env!("CARGO_PKG_VERSION")));
            m.insert("window.theme".to_string(), serde_json::json!("system"));
            m
        })),
        kiri_core::config::ConfigAllowlist::new(crate::host_policy::config_keys()),
        kiri_core::limits::Limits::default(),
    ))
    .with_updater(
        kiri_core::updater_surface::UpdaterService::new(
            crate::host_policy::HOST_PINNED_UPDATE_PUBLIC_KEY,
            kiri_core::update::Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("valid package version"),
            kiri_core::limits::Limits::default(),
        )
        .with_feed(crate::update_feed::fetch_pinned_release_manifest),
    )
    .with_cli(kiri_core::cli::CliService::new(std::env::args().collect::<Vec<String>>()))
    .with_fs_watch(kiri_core::fs_watch::FsWatchService::new(
        Arc::new(crate::fs_watch_ctl::NativeFsWatchBackend::new()),
        kiri_core::fs_watch::FsWatchAllowlist::new(crate::host_policy::fs_watch_targets()),
        kiri_core::limits::Limits::default(),
    ))
    .with_ws(kiri_core::websocket::WsService::new(
        Arc::new(crate::ws_ctl::NativeWsBackend::new()),
        kiri_core::websocket::WsAllowlist::new(crate::host_policy::ws_allow_urls()),
        kiri_core::limits::Limits::default(),
    ))
    .with_menu(kiri_core::app_menu::MenuService::new(
        Arc::new(kiri_core::app_menu::DisabledMenu),
        kiri_core::app_menu::MenuAllowlist::new(vec![]),
        kiri_core::limits::Limits::default(),
    ))
}

fn run_inner(options: HostOptions) -> Result<StartupMarkers, i32> {
    let markers = Rc::new(RefCell::new(StartupMarkers::new()));
    record(&markers, Marker::ProcessSpawnRequested);
    record(&markers, Marker::NativeEntry);

    let event_loop = EventLoop::new();
    let window = std::sync::Arc::new(
        WindowBuilder::new()
            .with_title(options.title.clone())
            .with_inner_size(tao::dpi::LogicalSize::new(
                options.width as f64,
                options.height as f64,
            ))
            .build(&event_loop)
            .map_err(|e| {
                eprintln!("[kiri] window creation failed: {e}");
                1
            })?,
    );
    record(&markers, Marker::PlatformInit);

    record(&markers, Marker::WebViewCreationRequested);

    // Control-plane identity is assigned by the native runtime, never by JS.
    // The full Router is built on first `window.kiri.send()` (research #1)
    // so WebView creation is not blocked by plugin construction.
    let mut registry = CallerRegistry::new();
    let caller = registry.register();
    let caller_caps = kiri_core::security::trusted_frontend_capabilities();
    let diagnostics = Diagnostics::new();
    let resources: std::sync::Arc<Mutex<ResourceTable<()>>> =
        std::sync::Arc::new(Mutex::new(ResourceTable::<()>::new()));
    let router_cell: Rc<RefCell<Option<kiri_core::dispatch::Router>>> = Rc::new(RefCell::new(None));
    let smoke = options.smoke;
    let ipc_bench = options.ipc_bench;
    let ipc_bench_runs = options.ipc_bench_runs;
    let ipc_bench_sizes = options.ipc_bench_sizes.clone();
    let ipc_bench_out = options.ipc_bench_out.clone();
    let markers_out = options.markers_out.clone();
    let exit_after_ready_ms = options.exit_after_ready_ms as u128;
    let watchdog_ms = options.watchdog_ms as u128;
    let ipc_bench_done = Rc::new(Cell::new(false));
    let ipc_bench_injected = Rc::new(Cell::new(false));

    // Shared slot so the IPC handler can post responses back to the webview
    // once it exists (the handler is created on the builder before build()).
    let webview_slot: Rc<RefCell<Option<wry::WebView>>> = Rc::new(RefCell::new(None));
    let webview = WebViewBuilder::new()
        .with_asynchronous_custom_protocol("kiri".into(), {
            let options = options.clone();
            move |_id, request, responder| {
                std::thread::spawn({
                    let options = options.clone();
                    move || {
                        let path = request.uri().path().to_string();
                        let range = request
                            .headers()
                            .get(header::RANGE)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let if_none_match = request
                            .headers()
                            .get(header::IF_NONE_MATCH)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let response =
                            serve_kiri(&options, &path, range.as_deref(), if_none_match.as_deref());
                        responder.respond(response);
                    }
                });
            }
        })
        .with_navigation_handler(|url| is_navigation_allowed(&url))
        .with_url("kiri://localhost/index.html")
        .with_initialization_script(BRIDGE_SCRIPT)
        .with_on_page_load_handler({
            let markers = markers.clone();
            move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    record(&markers, Marker::WebViewReady);
                }
            }
        })
        .with_ipc_handler({
            let markers = markers.clone();
            let router_cell = router_cell.clone();
            let window_for_router = window.clone();
            let options_for_router = options.clone();
            let webview_slot = webview_slot.clone();
            let diagnostics = diagnostics.clone();
            let resources = resources.clone();
            let ipc_bench_done = ipc_bench_done.clone();
            let ipc_bench_out = ipc_bench_out.clone();
            move |msg| {
                // Origin check: wry builds the IPC Request from the calling
                // frame's document URL (uri), with no Origin header. We judge
                // the request URI instead. Only messages whose document URL is
                // the application origin are handled; a remote page or subframe
                // is rejected as defense in depth, mirroring the Windows
                // is_app_origin_url gate in handle_web_message.
                let doc_url = msg.uri().to_string();
                if !is_app_origin(&doc_url) {
                    return;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(msg.body()) else {
                    return;
                };
                if value.get("type").and_then(|t| t.as_str()) == Some("ipc_bench") {
                    match crate::ipc_bench::write_result(ipc_bench_out.as_ref(), &value) {
                        Ok(()) => ipc_bench_done.set(true),
                        Err(e) => {
                            eprintln!("[kiri] through-webview ipc bench failed: {e}");
                            std::process::exit(1);
                        }
                    }
                    return;
                }
                // Control-plane command: dispatch through kiri-core and post
                // the wire response back to the page (T003).
                if let Some(req_val) = value.get("request") {
                    let Ok(request) = serde_json::from_value::<WireRequest>(req_val.clone()) else {
                        let err = WireResponse::err(
                            0,
                            kiri_core::error::Error::protocol_error("malformed command request"),
                        );
                        post_response(&webview_slot, &err);
                        return;
                    };
                    let mut sink = diagnostics.clone();
                    if !markers.borrow().has(Marker::FirstInvokeDispatched) {
                        record(&markers, Marker::FirstInvokeDispatched);
                    }
                    if router_cell.borrow().is_none() {
                        let window_ctrl: std::sync::Arc<dyn kiri_core::window::WindowController> =
                            std::sync::Arc::new(crate::window_ctl::TaoWindowController::new(
                                window_for_router.clone(),
                            ));
                        let clipboard_ctrl: std::sync::Arc<
                            dyn kiri_core::clipboard::ClipboardController,
                        > = std::sync::Arc::new(
                            crate::clipboard_ctl::CrossClipboardController::new()
                                .expect("clipboard init"),
                        );
                        *router_cell.borrow_mut() = Some(build_host_router(
                            window_ctrl,
                            clipboard_ctrl,
                            &diagnostics,
                            &resources,
                            &options_for_router,
                        ));
                    }
                    let response = router_cell.borrow().as_ref().unwrap().dispatch(
                        caller,
                        &caller_caps,
                        &request,
                        &mut sink,
                    );
                    if !markers.borrow().has(Marker::FirstInvokeResponded) {
                        record(&markers, Marker::FirstInvokeResponded);
                    }
                    // Reflect any resource churn so the panel stays honest.
                    diagnostics.set_open_resources(resources.lock().unwrap().len() as u32);
                    post_response(&webview_slot, &response);
                    return;
                }
                match value.get("phase").and_then(|p| p.as_str()) {
                    Some("dom") => {
                        // Recover webview_ready if page-load lagged (parity
                        // with the Windows host's dom-message fallback).
                        if !markers.borrow().has(Marker::WebViewReady) {
                            record(&markers, Marker::WebViewReady);
                        }
                        record(&markers, Marker::DomReady);
                        record(&markers, Marker::AppReady);
                    }
                    Some("frame") => {
                        record(&markers, Marker::FirstAnimationFrame);
                    }
                    _ => {}
                }
            }
        })
        .build(&*window)
        .map_err(|e| {
            eprintln!("[kiri] webview build failed: {e}");
            1
        })?;
    *webview_slot.borrow_mut() = Some(webview);
    record(&markers, Marker::BridgeReady);

    let t0 = Instant::now();
    let mut smoke_armed = false;
    let mut frame_at: Option<Instant> = None;

    // The webview lives in `webview_slot` (owned by this closure) so it stays
    // alive for the lifetime of the session; the IPC handler posts responses
    // through the same slot.
    // Event-loop-independent watchdog (T011). The in-loop check below only
    // runs when events arrive; if the loop is starved (headless display with
    // no frame pumping) it would never fire and the smoke run would hang.
    // This thread guarantees the process terminates after watchdog_ms.
    if (smoke || ipc_bench) && options.watchdog_ms > 0 {
        let wd_ms = options.watchdog_ms as u64;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(wd_ms));
            eprintln!("[kiri] watchdog: ready state not reached within the watchdog");
            std::process::exit(2);
        });
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *control_flow = ControlFlow::Exit;
        } else {
            // Keep the webview alive for the whole loop (owned by webview_slot).
            let _ = webview_slot.borrow();
            let _ = event;
        }

        if smoke || ipc_bench {
            let elapsed = t0.elapsed().as_millis();
            if watchdog_ms > 0 && elapsed > watchdog_ms {
                eprintln!("[kiri] watchdog: ready state not reached within the watchdog");
                std::process::exit(2);
            }
            let has_frame = markers.borrow().has(Marker::FirstAnimationFrame);
            if has_frame && !smoke_armed {
                smoke_armed = true;
                frame_at = Some(Instant::now());
            }
            if ipc_bench && has_frame && !ipc_bench_injected.get() {
                if let Some(webview) = webview_slot.borrow().as_ref() {
                    ipc_bench_injected.set(true);
                    let script = crate::ipc_bench::kiri_script(
                        ipc_bench_runs,
                        crate::ipc_bench::DEFAULT_WARMUP,
                        &ipc_bench_sizes,
                    );
                    if let Err(e) = webview.evaluate_script(&script) {
                        eprintln!("[kiri] failed to inject ipc bench: {e}");
                        std::process::exit(1);
                    }
                }
            }
            if ipc_bench {
                if ipc_bench_done.get() {
                    let recorded = markers.borrow().clone_markers();
                    write_startup_result(&recorded, markers_out.as_ref());
                    std::process::exit(0);
                }
            } else if let Some(frame) = frame_at {
                if frame.elapsed().as_millis() > exit_after_ready_ms {
                    // Emit the startup result (stdout + optional file) and exit
                    // cleanly. tao's macOS loop does not return, so this is the
                    // single emission point for the cross backend.
                    let recorded = markers.borrow().clone_markers();
                    write_startup_result(&recorded, markers_out.as_ref());
                    std::process::exit(0);
                }
            }
        }
    });
}

// All host allowlists (http, shell, fs glob, sidecar, event, config, store,
// deeplink, opener, autostart, shortcut, dialog, notification, and the pinned
// update key) live in `crate::host_policy` so both backends share identical
// security posture. See host_policy.rs.

#[cfg(test)]
mod host_router_regression_tests {
    use super::build_host_router;
    use crate::HostOptions;
    use kiri_core::clipboard::{ClipboardController, ClipboardState};
    use kiri_core::diagnostics::Diagnostics;
    use kiri_core::resources::ResourceTable;
    use kiri_core::window::{WindowController, WindowState};
    use std::sync::{Arc, Mutex};

    // Headless no-op controllers so the production router can be built in a
    // test without opening a window or touching the OS clipboard.
    struct StubWindow;
    impl WindowController for StubWindow {
        fn set_title(&self, _s: &mut WindowState, _t: &str) {}
        fn show(&self, _s: &mut WindowState) {}
        fn hide(&self, _s: &mut WindowState) {}
        fn minimize(&self, _s: &mut WindowState) {}
        fn maximize(&self, _s: &mut WindowState) {}
        fn restore(&self, _s: &mut WindowState) {}
        fn close(&self, _s: &mut WindowState) {}
        fn focus(&self, _s: &mut WindowState) {}
    }

    struct StubClipboard;
    impl ClipboardController for StubClipboard {
        fn read(&self, _state: &mut ClipboardState) -> kiri_core::error::Result<String> {
            Ok(String::new())
        }
        fn write(&self, _state: &mut ClipboardState, _text: &str) {
            // no-op in headless tests
        }
    }

    #[test]
    fn production_router_registers_every_catalog_command() {
        let window_ctrl: Arc<dyn WindowController> = Arc::new(StubWindow);
        let clipboard_ctrl: Arc<dyn ClipboardController> = Arc::new(StubClipboard);
        let router = build_host_router(
            window_ctrl,
            clipboard_ctrl,
            &Diagnostics::new(),
            &std::sync::Arc::new(Mutex::new(ResourceTable::<()>::new())),
            &HostOptions::default(),
        );

        // Iterate the single source of truth for the command catalog. Every
        // catalog id must resolve on the production router construction; if a
        // surface is dropped from build_host_router this fails loudly instead
        // of silently returning ProtocolError for an "unknown command".
        let mut missing = Vec::new();
        for cmd in kiri_core::commands::COMMANDS.iter() {
            if !router.is_known(cmd.id) {
                missing.push((cmd.id, cmd.name));
            }
        }
        assert!(missing.is_empty(), "production router is missing catalog commands: {:?}", missing);
    }

    #[test]
    fn production_router_registers_cli_fs_watch_ws_menu() {
        let window_ctrl: Arc<dyn WindowController> = Arc::new(StubWindow);
        let clipboard_ctrl: Arc<dyn ClipboardController> = Arc::new(StubClipboard);
        let router = build_host_router(
            window_ctrl,
            clipboard_ctrl,
            &Diagnostics::new(),
            &std::sync::Arc::new(Mutex::new(ResourceTable::<()>::new())),
            &HostOptions::default(),
        );

        // The four surfaces that were previously only wired in the test-only
        // router (commands 66-73) must now be present on the real host router.
        for id in [
            kiri_core::dispatch::command_id::CLI_ARGS,
            kiri_core::dispatch::command_id::FS_WATCH,
            kiri_core::dispatch::command_id::FS_UNWATCH,
            kiri_core::dispatch::command_id::WS_CONNECT,
            kiri_core::dispatch::command_id::WS_SEND,
            kiri_core::dispatch::command_id::WS_CLOSE,
            kiri_core::dispatch::command_id::MENU_SET,
            kiri_core::dispatch::command_id::MENU_INVOKE,
        ] {
            assert!(router.is_known(id), "command id {} must be registered", id);
        }
    }
}
