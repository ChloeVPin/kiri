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

use std::cell::RefCell;
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
use kiri_core::capabilities::PathScope;
use kiri_core::diagnostics::Diagnostics;
use kiri_core::platform::EventBus;
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
/// `HostOptions.frontend_dir`; if absent, the embedded blank page is served for
/// the index path (sub-asset requests 404, same as before). `Range` requests are
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
    let root = match options.frontend_dir.as_ref() {
        Some(d) => d.clone(),
        None => {
            // No frontend dir: only the embedded index page is available.
            if request_path.trim_start_matches('/').is_empty() {
                const FALLBACK: &str = include_str!("../../../examples/blank/index.html");
                return WryResponse::builder()
                    .status(200)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .body(Cow::Borrowed(FALLBACK.as_bytes()))
                    .unwrap();
            }
            return WryResponse::builder()
                .status(404)
                .header("Content-Type", "text/plain")
                .body(Cow::Borrowed(b"not found".as_slice()))
                .unwrap();
        }
    };

    let resp =
        serve_checked(&root, request_path, &ServeOptions { range, if_none_match, allow: &[] });
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
        send: function (req) { post({ type: 'cmd', request: req }); }
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

    // Control-plane router for the native bridge (T003). The caller identity
    // is assigned by the native runtime, never by JavaScript; grant the ping
    // capability so control commands can run from the trusted frontend.
    let mut registry = CallerRegistry::new();
    let caller = registry.register();
    let caller_caps = kiri_core::security::trusted_frontend_capabilities();
    let diagnostics = Diagnostics::new();
    let events = EventBus::new();
    // Shared generational resource table owned by the host. The resources plugin
    // binds this exact instance via the ABI context, so kiri.open/kiri.close
    // mutate it and keep the diagnostics open-resource count honest and dynamic.
    let resources: Arc<Mutex<ResourceTable<()>>> = Arc::new(Mutex::new(ResourceTable::<()>::new()));

    // Host-owned fs scope: a bounded sandbox under the temp dir. The host is
    // the only authority that can widen it; the frontend cannot.
    let mut fs_scope = PathScope::new(std::env::temp_dir().join("kiri-fs"));
    fs_scope.read = true;
    fs_scope.write = true;

    let router = crate::plugins::PluginHost::build_router_with_plugins(
        &diagnostics,
        &resources,
        caller,
    )
    // R-3: JS-surface commands (kiri.platform.*, kiri.app.*, kiri.event.*).
    .with_platform(events)
    .with_fs_service(
        kiri_core::fs::FsService::new(fs_scope, kiri_core::limits::Limits::default())
            .with_glob(kiri_core::capabilities::GlobScope::new(fs_glob_patterns())),
    )
    // G-5: kiri.window.* surface backed by the real native window.
    .with_window(
        Arc::new(crate::window_ctl::TaoWindowController::new(window.clone())),
        Arc::new(Mutex::new(kiri_core::window::WindowState::new(&options.title))),
    )
    // G-6: kiri.clipboard.* surface backed by the real OS clipboard.
    .with_clipboard(
        Arc::new(crate::clipboard_ctl::CrossClipboardController::new().expect("clipboard init")),
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
        kiri_core::http::HostAllowlist::new(http_allow_hosts()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4: kiri.shell.run surface (audit item 4). Capability-gated (SHELL)
    // and constrained to a host allowlist so a granted capability still
    // cannot spawn an unapproved program; output is bulk-capped like kiri.fs.
    .with_shell(kiri_core::shell::ShellService::new(
        std::sync::Arc::new(crate::shell_ctl::CrossShellRunner::new()),
        kiri_core::shell::ShellAllowlist::new(shell_allow_commands()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4b: kiri.notification.show surface (audit item 5). Capability-gated
    // (NOTIFICATION) and constrained to a host template allowlist so a
    // granted capability still cannot render arbitrary title/body; only
    // pre-approved templates with bounded args may show.
    .with_notification(kiri_core::notification::NotificationService::new(
        std::sync::Arc::new(crate::notification_ctl::cross_notify::CrossNotificationRunner::new()),
        kiri_core::notification::NotificationAllowlist::new(notification_templates()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4c: kiri.dialog.open surface (audit item 7). Capability-gated
    // (DIALOG) and constrained to a host allowlist of dialog kinds with a
    // host-owned title, so a granted capability still cannot open an
    // arbitrary native prompt; only pre-approved dialog kinds may show.
    .with_dialog(kiri_core::dialog::DialogService::new(
        std::sync::Arc::new(crate::dialog_ctl::CrossDialogRunner::new()),
        kiri_core::dialog::DialogAllowlist::new(dialog_templates()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4d: kiri.shortcut.register surface (audit item 8). Capability-gated
    // (SHORTCUT) and constrained to a host allowlist of exact accelerators mapped
    // to host-owned actions, so a granted capability still cannot register an
    // arbitrary global hotkey; only pre-approved accelerators may bind.
    .with_shortcut(kiri_core::shortcut::ShortcutService::new(
        std::sync::Arc::new(crate::shortcut_ctl::CrossShortcutRunner::new()),
        kiri_core::shortcut::ShortcutAllowlist::new(shortcut_bindings()),
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
        kiri_core::autostart::AutostartAllowlist::new(autostart_policy()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4f: kiri.store.get/set surface (audit item 10). Capability-gated (STORE)
    // and bounded to a host allowlist of namespaces, so a granted capability still
    // cannot read/write outside an approved namespace. This exceeds Tauri's store
    // plugin, which lets the frontend read/write the whole store once the capability
    // is present (a cross-feature data-leak surface).
    .with_store(kiri_core::store::StoreService::new(
        std::sync::Arc::new(crate::store_ctl::CrossStoreBackend::new()),
        kiri_core::store::StoreAllowlist::new(store_namespaces()),
        kiri_core::limits::Limits::default(),
    ))
    // G-4g: kiri.deeplink.register surface (audit item 11). Capability-gated
    // (DEEPLINK) and bounded to a host allowlist of exact schemes, so a granted
    // capability still cannot squat on an arbitrary URI scheme. This exceeds
    // Tauri's deep-link plugin, which lets the frontend register any scheme once
    // the capability is present (a scheme-squatting surface).
    .with_deeplink(kiri_core::deeplink::DeeplinkService::new(
        std::sync::Arc::new(crate::deeplink_ctl::cross_deeplink::CrossDeeplinkRunner::new()),
        kiri_core::deeplink::DeeplinkAllowlist::new(deeplink_schemes()),
        kiri_core::limits::Limits::default(),
    ));
    let smoke = options.smoke;
    let markers_out = options.markers_out.clone();
    let exit_after_ready_ms = options.exit_after_ready_ms as u128;
    let watchdog_ms = options.watchdog_ms as u128;

    // Shared slot so the IPC handler can post responses back to the webview
    // once it exists (the handler is created on the builder before build()).
    let webview_slot: Rc<RefCell<Option<wry::WebView>>> = Rc::new(RefCell::new(None));
    let webview = WebViewBuilder::new()
        .with_custom_protocol("kiri".into(), {
            let options = options.clone();
            move |_id, request| {
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
                serve_kiri(&options, &path, range.as_deref(), if_none_match.as_deref())
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
            let router = router.clone();
            let webview_slot = webview_slot.clone();
            let diagnostics = diagnostics.clone();
            let resources = resources.clone();
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
                    let response = router.dispatch(caller, &caller_caps, &request, &mut sink);
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
    if smoke {
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

        if smoke {
            let elapsed = t0.elapsed().as_millis();
            if elapsed > watchdog_ms {
                eprintln!("[kiri] watchdog: ready state not reached within the watchdog");
                std::process::exit(2);
            }
            let has_frame = markers.borrow().has(Marker::FirstAnimationFrame);
            if has_frame && !smoke_armed {
                smoke_armed = true;
                frame_at = Some(Instant::now());
            }
            if let Some(frame) = frame_at {
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

/// Host-allowlist for kiri.http.get. Default-deny: only these hosts may be
/// fetched even when the HTTP capability is granted. Expanded per-app config
/// in a later task; for now this is the seed allowlist that proves the
/// exceed-Tauri security axis (Tauri's http plugin has no host allowlist).
fn http_allow_hosts() -> Vec<String> {
    vec!["api.example.com".to_string(), "127.0.0.1".to_string(), "localhost".to_string()]
}

/// Host allowlist for `kiri.shell.run` (audit item 4, G-4). Default-deny:
/// only the exact program + arg prefix below may spawn. Mirrors Tauri's shell
/// plugin use but inverts its trust model: Tauri grants arbitrary execution
/// when the capability is present; Kiri refuses every command that is not an
/// explicit allowlist entry. The seed entry is a harmless readonly probe.
/// Host glob allowlist for `kiri.fs.*` (audit item 6, fs glob scope). Default-deny
/// relative to the fs root: only paths matching a pattern may be touched. This
/// matches Tauri v2's `fs` plugin scope granularity (e.g. `data/**/*.json`)
/// while keeping the FS capability gate. Empty = root-only (seed uses a safe
/// read-only data scope).
fn fs_glob_patterns() -> Vec<String> {
    vec!["data/**".to_string(), "config/*.json".to_string(), "*.log".to_string()]
}

fn shell_allow_commands() -> Vec<kiri_core::shell::AllowedCommand> {
    vec![kiri_core::shell::AllowedCommand {
        program: "echo".to_string(),
        args: vec!["kiri-probe".to_string()],
    }]
}

/// Host template allowlist for `kiri.notification.show` (audit item 5, G-4b).
/// Default-deny: only the exact template ids below may display, and the frontend
/// may only supply bounded positional args. The host owns the title/body text,
/// inverting Tauri's notification plugin trust model (Tauri lets the frontend send
/// arbitrary title/body once the capability is present).
/// Host allowlist for `kiri.dialog.open` (audit item 7, G-4c). Default-deny:
/// only the exact dialog kinds below may open, each with a host-owned title
/// template and bounded args (file pickers additionally restrict extensions).
/// Inverts Tauri's dialog plugin trust model: a granted DIALOG capability still
/// cannot render a free-form native prompt; only pre-approved kinds may show.
/// Host allowlist for `kiri.shortcut.register` (audit item 8, G-4d). Default-deny:
/// only the exact accelerators below may bind, each mapped to a host-owned action;
/// the frontend cannot supply or alter the accelerator or action. Inverts Tauri's
/// global-shortcut plugin trust model: a granted SHORTCUT capability still cannot
/// register an arbitrary global hotkey, so a malicious frontend cannot hijack desktop
/// combos (e.g. Cmd+Q) globally.
/// Host policy for `kiri.autostart.*` (audit item 9, G-4e). Default-deny: autostart
/// is disabled unless the host explicitly opts in. The frontend can only toggle
/// `enabled`; it cannot choose which executable persists (the runner registers only
/// the host's own binary). Inverts Tauri's autostart plugin trust model, which lets
/// the frontend enable launch-at-login freely once the capability is present.
/// Host allowlist for `kiri.store.*` (audit item 10, G-4f). Default-deny: only the
/// exact namespaces below may be addressed; the frontend cannot reach other namespaces
/// (e.g. `auth.session`). Inverts Tauri's store plugin trust model, which lets the
/// frontend read/write the whole store once the capability is present.
fn store_namespaces() -> Vec<kiri_core::store::StoreNamespace> {
    vec![kiri_core::store::StoreNamespace { prefix: "app.prefs".to_string() }]
}

fn deeplink_schemes() -> Vec<kiri_core::deeplink::DeeplinkScheme> {
    vec![kiri_core::deeplink::DeeplinkScheme { scheme: "kiri-app".to_string() }]
}

fn autostart_policy() -> bool {
    false
}

fn shortcut_bindings() -> Vec<kiri_core::shortcut::ShortcutBinding> {
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

fn dialog_templates() -> Vec<kiri_core::dialog::DialogTemplate> {
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

fn notification_templates() -> Vec<kiri_core::notification::NotificationTemplate> {
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
