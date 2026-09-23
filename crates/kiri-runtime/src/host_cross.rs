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
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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

fn ipc_benchmark_ready(markers: &StartupMarkers) -> bool {
    markers.has(Marker::FirstAnimationFrame) || markers.has(Marker::DomReady)
}

/// Reserved paths inside the `kiri://` app origin for the protocol_ring
/// transport (spike). Reachable only while the transport is engaged;
/// everything else under `/.kiri/` falls through to asset serving and 404s.
const PROTO_INVOKE_PATH: &str = "/.kiri/ipc/invoke";
const PROTO_PING_PATH: &str = "/.kiri/ipc/ping";

/// One parked protocol_ring request: the raw KRSL request frame plus the
/// async responder that must answer the fetch once the main event loop has
/// dispatched it through the ZcIpcGate. The wry async protocol handler may
/// run on any thread, so requests queue here and drain on the event loop
/// where the router and gate live.
struct ProtoInvoke {
    body: Vec<u8>,
    responder: wry::RequestAsyncResponder,
}

type ProtoInbox = Arc<Mutex<VecDeque<ProtoInvoke>>>;

/// Dispatch context the event loop needs to answer parked protocol_ring
/// requests: the same router, pipe gate, caller identity, and diagnostics
/// the postMessage IPC handler uses, plus reply-leg counters that get
/// merged into the bench artifact.
struct ProtoDrainCtx {
    router_cell: Rc<RefCell<Option<kiri_core::dispatch::Router>>>,
    zc_gate: Arc<Mutex<kiri_core::zc_ipc_gate::ZcIpcGate>>,
    caller: kiri_core::caller::CallerId,
    caller_caps: kiri_core::capabilities::CapabilityBits,
    diagnostics: Diagnostics,
    resources: std::sync::Arc<Mutex<ResourceTable<()>>>,
    window: std::sync::Arc<tao::window::Window>,
    options: HostOptions,
    menu_runner: crate::menu_dispatch::MenuDispatcherHandle,
    markers: Rc<RefCell<StartupMarkers>>,
    replies_ok: Rc<Cell<u32>>,
    replies_fallback: Rc<Cell<u32>>,
}

/// Dispatch context the wry async protocol callback uses to answer
/// `kiri.ping`-shaped fetches on the protocol thread itself, without the
/// parked-responder + event-loop hop `ProtoInbox` imposes. Every field is
/// `Send + Sync`: the inline router registers only main-thread-free
/// commands (today exactly `kiri.ping`), the gate is the SAME
/// `ZcIpcGate` object the postMessage and parked legs lock (one permit
/// lifecycle, one mint path), and the counters are atomics merged into the
/// bench artifact. Commands the inline router does not know still park and
/// drain on the event loop, so window/menu mutation never leaves the tao
/// thread.
struct ProtoInlineCtx {
    router: Arc<kiri_core::dispatch::Router>,
    zc_gate: Arc<Mutex<kiri_core::zc_ipc_gate::ZcIpcGate>>,
    caller: kiri_core::caller::CallerId,
    caller_caps: kiri_core::capabilities::CapabilityBits,
    diagnostics: Diagnostics,
    resources: Arc<Mutex<ResourceTable<()>>>,
    answered_inline: Arc<AtomicU32>,
    replies_fallback: Arc<AtomicU32>,
    parked: Arc<AtomicU32>,
    /// First-invoke markers the protocol thread cannot record into the
    /// `Rc<RefCell<StartupMarkers>>`; it stores the dispatch-time timestamps
    /// here and the event loop merges them with their original nanos.
    pending_dispatched_ns: Arc<AtomicU64>,
    pending_responded_ns: Arc<AtomicU64>,
}

fn proto_json_response(status: u16, value: &serde_json::Value) -> WryResponse<Cow<'static, [u8]>> {
    WryResponse::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Cow::Owned(serde_json::to_vec(value).unwrap_or_default()))
        .unwrap()
}

/// The ungated leg of the protocol_ring reply channel: a plain serialized
/// `WireResponse` as `application/json`, carrying exactly what the ordinary
/// postMessage wire would carry (gate-produced denial stubs, or the full
/// response when the reply-leg grant refused the binary frame). The binary
/// KRSL frame is only served under a live ZcIpcGrant.
fn proto_wire_json_response(response: &WireResponse) -> WryResponse<Cow<'static, [u8]>> {
    let value = serde_json::to_value(response).unwrap_or_default();
    proto_json_response(200, &value)
}

fn proto_protocol_error(request_id: u64, msg: &str) -> WryResponse<Cow<'static, [u8]>> {
    let err = WireResponse::err(request_id, kiri_core::error::Error::protocol_error(msg));
    let value = serde_json::to_value(&err).unwrap_or_default();
    proto_json_response(400, &value)
}

/// Build the production router on first use, shared by the postMessage IPC
/// handler and the protocol_ring drain so both legs dispatch identically.
fn ensure_host_router(
    router_cell: &Rc<RefCell<Option<kiri_core::dispatch::Router>>>,
    window: &std::sync::Arc<tao::window::Window>,
    diagnostics: &Diagnostics,
    resources: &std::sync::Arc<Mutex<ResourceTable<()>>>,
    options: &HostOptions,
    menu_runner: &crate::menu_dispatch::MenuDispatcherHandle,
) {
    if router_cell.borrow().is_some() {
        return;
    }
    let window_ctrl: std::sync::Arc<dyn kiri_core::window::WindowController> =
        std::sync::Arc::new(crate::window_ctl::TaoWindowController::new(window.clone()));
    let clipboard_ctrl: std::sync::Arc<dyn kiri_core::clipboard::ClipboardController> =
        std::sync::Arc::new(
            crate::clipboard_ctl::CrossClipboardController::new().expect("clipboard init"),
        );
    *router_cell.borrow_mut() = Some(build_host_router(
        window_ctrl,
        clipboard_ctrl,
        diagnostics,
        resources,
        options,
        std::sync::Arc::new(menu_runner.clone()),
    ));
}

/// Decode one KRSL request frame, run it through the same
/// `ZcIpcGate::dispatch_through_webview` lifecycle every through-webview
/// leg runs, and build the HTTP response: a binary KRSL frame when a live
/// `ZcIpcGrant` authorizes the reply leg, or the plain JSON wire response
/// otherwise (malformed frame, decode failure, gate denial, reply-leg
/// refusal). Returns `(response, true)` when the binary leg carried the
/// reply. Shared by the parked event-loop drain and the protocol-inline
/// answer path so both legs apply identical validation, gating, and reply
/// currency; the only difference is which router dispatches.
fn proto_frame_response(
    router: &kiri_core::dispatch::Router,
    gate: &Mutex<kiri_core::zc_ipc_gate::ZcIpcGate>,
    caller: kiri_core::caller::CallerId,
    caller_caps: &kiri_core::capabilities::CapabilityBits,
    diagnostics: &Diagnostics,
    resources: &Mutex<ResourceTable<()>>,
    body: &[u8],
) -> (WryResponse<Cow<'static, [u8]>>, bool) {
    use crate::ring_ipc as ring;
    let (req, payload_bytes) = match ring::read_request_frame(body) {
        Ok(v) => v,
        Err(e) => return (proto_protocol_error(0, e), false),
    };
    let payload: serde_json::Value = match req.codec {
        ring::CODEC_UTF8_STRING => match String::from_utf8(payload_bytes.to_vec()) {
            Ok(s) => serde_json::Value::String(s),
            Err(_) => {
                return (
                    proto_protocol_error(req.request_id, "protocol_ring payload is not utf-8"),
                    false,
                );
            }
        },
        ring::CODEC_JSON => match serde_json::from_slice::<serde_json::Value>(payload_bytes) {
            Ok(v) => v,
            Err(_) => {
                return (
                    proto_protocol_error(req.request_id, "protocol_ring payload is not json"),
                    false,
                );
            }
        },
        _ => {
            return (
                proto_protocol_error(req.request_id, "unsupported protocol_ring codec"),
                false,
            );
        }
    };
    let request = WireRequest {
        magic: kiri_core::header::MAGIC,
        version: kiri_core::header::PROTOCOL_VERSION,
        flags: kiri_core::header::ControlFlags::REQUEST.bits(),
        command_id: req.command_id,
        request_id: req.request_id,
        payload_len: serde_json::to_vec(&payload).unwrap_or_default().len() as u32,
        codec: req.codec,
        payload,
    };
    let mut sink = diagnostics.clone();
    // The same permit lifecycle the postMessage leg runs: mint requires the
    // capability bit AND the surface allowlist; the returned grant is the
    // only currency the binary reply frame accepts.
    let gated = gate.lock().unwrap().dispatch_through_webview(
        router,
        caller,
        caller_caps,
        &request,
        &mut sink,
        kiri_core::trace::MonotonicClock::now_ns(),
    );
    diagnostics.set_open_resources(resources.lock().unwrap().len() as u32);
    let echo = ring::response_echo_bytes(&gated.response);
    let json_body;
    let (flags, resp_body): (u8, &[u8]) = match echo {
        Some(bytes) => (ring::RESP_ECHO_STRING, bytes),
        None => {
            json_body = serde_json::to_vec(&gated.response).unwrap_or_default();
            (ring::RESP_JSON, json_body.as_slice())
        }
    };
    // Binary reply leg accepts the same currency as the Windows shared-slot
    // publish: a live ZcIpcGrant bound to this caller + command. A denied
    // request carries no grant, so its error stub rides the JSON leg below.
    let authorized = ring::ring_reply_authorized(
        gated.grant.as_ref(),
        caller,
        request.command_id,
        resp_body,
        kiri_core::trace::MonotonicClock::now_ns(),
    );
    if authorized {
        if let Ok(frame) = ring::encode_response_frame(req.seq, req.request_id, flags, resp_body) {
            let resp = WryResponse::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Cow::Owned(frame))
                .unwrap();
            return (resp, true);
        }
    }
    (proto_wire_json_response(&gated.response), false)
}

/// Dispatch one parked protocol request and answer its fetch on the event
/// loop. Every path responds: a parked fetch is never left hanging by a
/// decode or gate failure.
fn handle_proto_invoke(ctx: &ProtoDrainCtx, item: ProtoInvoke) {
    if !ctx.markers.borrow().has(Marker::FirstInvokeDispatched) {
        record(&ctx.markers, Marker::FirstInvokeDispatched);
    }
    ensure_host_router(
        &ctx.router_cell,
        &ctx.window,
        &ctx.diagnostics,
        &ctx.resources,
        &ctx.options,
        &ctx.menu_runner,
    );
    let (resp, binary) = {
        let router_cell = ctx.router_cell.borrow();
        proto_frame_response(
            router_cell.as_ref().unwrap(),
            &ctx.zc_gate,
            ctx.caller,
            &ctx.caller_caps,
            &ctx.diagnostics,
            &ctx.resources,
            &item.body,
        )
    };
    if !ctx.markers.borrow().has(Marker::FirstInvokeResponded) {
        record(&ctx.markers, Marker::FirstInvokeResponded);
    }
    item.responder.respond(resp);
    if binary {
        ctx.replies_ok.set(ctx.replies_ok.get().saturating_add(1));
    } else {
        ctx.replies_fallback.set(ctx.replies_fallback.get().saturating_add(1));
    }
}

/// Answer one protocol-inline fetch inside the wry async protocol callback,
/// on whatever thread WebKit invoked it on. This is the hop the spike
/// removes: no ProtoInbox park, no EventLoopProxy wake, no event-loop
/// drain; the same gated dispatch + reply-leg authorization runs here and
/// `responder.respond` completes the fetch directly.
fn handle_proto_inline(ctx: &ProtoInlineCtx, body: Vec<u8>, responder: wry::RequestAsyncResponder) {
    let dispatched = ctx.pending_dispatched_ns.load(Ordering::Relaxed);
    if dispatched == 0 {
        ctx.pending_dispatched_ns
            .compare_exchange(0, now_ns(), Ordering::Relaxed, Ordering::Relaxed)
            .ok();
    }
    let (resp, binary) = proto_frame_response(
        &ctx.router,
        &ctx.zc_gate,
        ctx.caller,
        &ctx.caller_caps,
        &ctx.diagnostics,
        &ctx.resources,
        &body,
    );
    let responded = ctx.pending_responded_ns.load(Ordering::Relaxed);
    if responded == 0 {
        ctx.pending_responded_ns
            .compare_exchange(0, now_ns(), Ordering::Relaxed, Ordering::Relaxed)
            .ok();
    }
    responder.respond(resp);
    if binary {
        ctx.answered_inline.fetch_add(1, Ordering::Relaxed);
    } else {
        ctx.replies_fallback.fetch_add(1, Ordering::Relaxed);
    }
}

/// Merge first-invoke markers the protocol-inline path stored from the
/// protocol thread. Runs on the event loop, which owns the `Rc` markers;
/// the recorded timestamps are the ones captured at dispatch time.
fn merge_proto_inline_markers(markers: &Rc<RefCell<StartupMarkers>>, ctx: &ProtoInlineCtx) {
    let dispatched = ctx.pending_dispatched_ns.swap(0, Ordering::Relaxed);
    if dispatched != 0 && !markers.borrow().has(Marker::FirstInvokeDispatched) {
        markers.borrow_mut().record(Marker::FirstInvokeDispatched, dispatched);
    }
    let responded = ctx.pending_responded_ns.swap(0, Ordering::Relaxed);
    if responded != 0 && !markers.borrow().has(Marker::FirstInvokeResponded) {
        markers.borrow_mut().record(Marker::FirstInvokeResponded, responded);
    }
}

/// Answer every parked protocol request. Called once per event-loop
/// iteration; the queue is empty whenever the transport is not engaged or
/// no fetch is in flight, so the cost is one uncontended lock.
fn drain_proto_inbox(inbox: &ProtoInbox, ctx: &ProtoDrainCtx) {
    loop {
        let item = inbox.lock().unwrap().pop_front();
        match item {
            Some(item) => handle_proto_invoke(ctx, item),
            None => break,
        }
    }
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
      var domPosted = false;
      function postDom() {
        if (domPosted) return;
        domPosted = true;
        post({ type: 'ready', phase: 'dom' });
      }
      window.addEventListener('DOMContentLoaded', postDom);
      if (document.readyState !== 'loading') {
        postDom();
      }
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
        },
        onMenuAction: function (_action) {},
        onUpdateAvailable: function (_info) {}
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

fn menu_items() -> Vec<kiri_core::app_menu::MenuItem> {
    tray_items()
        .into_iter()
        .map(|item| kiri_core::app_menu::MenuItem {
            id: item.id,
            label: item.label,
            action: item.action,
        })
        .collect()
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
    menu_runner: std::sync::Arc<dyn kiri_core::app_menu::MenuRunner>,
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
        menu_runner,
        kiri_core::app_menu::MenuAllowlist::new(menu_items()),
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
    let (menu_dispatcher, menu_runner) = crate::menu_dispatch::MenuDispatcher::new();
    let native_menu =
        std::rc::Rc::new(std::cell::RefCell::new(crate::native_menu::NativeMenu::new()));
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
    // Fail-closed gate for the through-webview pipe: one permit object carries
    // the capability-bit AND host-allowlist decision (kiri_core::zc_ipc_gate).
    // Behind one Mutex so the postMessage leg, the parked protocol drain, and
    // the protocol-inline answer path all mint + redeem through the same gate.
    let zc_gate = Arc::new(Mutex::new(crate::host_policy::zc_ipc_gate()));
    let smoke = options.smoke;
    let ipc_bench = options.ipc_bench;
    let ipc_bench_runs = options.ipc_bench_runs;
    let ipc_bench_sizes = options.ipc_bench_sizes.clone();
    let ipc_bench_transport = options.ipc_bench_transport;
    let ipc_bench_out = options.ipc_bench_out.clone();
    let markers_out = options.markers_out.clone();
    let exit_after_ready_ms = options.exit_after_ready_ms as u128;
    let watchdog_ms = options.watchdog_ms as u128;
    let ipc_bench_done = Rc::new(Cell::new(false));
    let ipc_bench_injected = Rc::new(Cell::new(false));
    let menu_smoke_done = Rc::new(Cell::new(false));

    // protocol_ring / protocol_inline transports (opt-in spikes): fetches to
    // `kiri://localhost/.kiri/ipc/invoke` are answered after dispatch through
    // the same ZcIpcGate the postMessage pipe uses. protocol_ring parks the
    // responder here and drains it on the event loop (the proxy wakes a
    // waiting loop). protocol_inline answers kiri.ping-shaped invokes inside
    // the protocol callback itself and only parks commands the inline
    // router does not know, so the hop the spike targets is skipped.
    let proto_engaged = matches!(
        ipc_bench_transport,
        crate::ipc_bench::IpcBenchTransport::ProtocolRing
            | crate::ipc_bench::IpcBenchTransport::ProtocolInline
    );
    let proto_inbox: ProtoInbox = Arc::new(Mutex::new(VecDeque::new()));
    let proto_replies_ok = Rc::new(Cell::new(0u32));
    let proto_replies_fallback = Rc::new(Cell::new(0u32));
    let event_proxy = event_loop.create_proxy();

    // protocol_inline dispatch context. The inline router is a dedicated
    // `Router::new()`: it registers exactly the commands safe to dispatch
    // off the tao thread (kiri.ping today; anything that touches window,
    // menu, tray, clipboard, or other main-thread state stays parked).
    // `required_bits` for kiri.ping is the same bit the production router
    // registers, so mint + redeem + grant binding are identical on both
    // legs.
    let inline_ctx: Option<Arc<ProtoInlineCtx>> =
        if ipc_bench_transport == crate::ipc_bench::IpcBenchTransport::ProtocolInline {
            Some(Arc::new(ProtoInlineCtx {
                router: Arc::new(kiri_core::dispatch::Router::new()),
                zc_gate: zc_gate.clone(),
                caller,
                caller_caps,
                diagnostics: diagnostics.clone(),
                resources: resources.clone(),
                answered_inline: Arc::new(AtomicU32::new(0)),
                replies_fallback: Arc::new(AtomicU32::new(0)),
                parked: Arc::new(AtomicU32::new(0)),
                pending_dispatched_ns: Arc::new(AtomicU64::new(0)),
                pending_responded_ns: Arc::new(AtomicU64::new(0)),
            }))
        } else {
            None
        };
    let inline_counters = inline_ctx.clone();

    // Shared slot so the IPC handler can post responses back to the webview
    // once it exists (the handler is created on the builder before build()).
    let webview_slot: Rc<RefCell<Option<wry::WebView>>> = Rc::new(RefCell::new(None));
    let webview = WebViewBuilder::new()
        .with_asynchronous_custom_protocol("kiri".into(), {
            let options = options.clone();
            let proto_inbox = proto_inbox.clone();
            let event_proxy = event_proxy.clone();
            let inline_ctx = inline_ctx.clone();
            move |_id, request, responder| {
                let path = request.uri().path().to_string();
                if proto_engaged {
                    if path == PROTO_INVOKE_PATH && request.method().as_str() == "POST" {
                        let body = request.into_body();
                        if let Some(inline) = inline_ctx.as_ref() {
                            // Answer-on-protocol-thread: invoke frames for
                            // commands the inline router knows (kiri.ping)
                            // are dispatched through the shared ZcIpcGate and
                            // answered right here. Unknown or malformed
                            // frames park for the event-loop drain, which
                            // owns the full production router.
                            let inlineable = crate::ring_ipc::read_request_frame(&body)
                                .map(|(req, _)| inline.router.is_known(req.command_id))
                                .unwrap_or(false);
                            if inlineable {
                                handle_proto_inline(inline, body, responder);
                                return;
                            }
                            inline.parked.fetch_add(1, Ordering::Relaxed);
                        }
                        proto_inbox.lock().unwrap().push_back(ProtoInvoke { body, responder });
                        let _ = event_proxy.send_event(());
                        return;
                    }
                    if path == PROTO_PING_PATH {
                        responder.respond(proto_json_response(
                            200,
                            &serde_json::json!({
                                "ok": true,
                                "transport": ipc_bench_transport.as_str(),
                            }),
                        ));
                        return;
                    }
                }
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
            let menu_runner = menu_runner.clone();
            let webview_slot = webview_slot.clone();
            let zc_gate = zc_gate.clone();
            let diagnostics = diagnostics.clone();
            let resources = resources.clone();
            let ipc_bench_done = ipc_bench_done.clone();
            let ipc_bench_out = ipc_bench_out.clone();
            let menu_smoke_done = menu_smoke_done.clone();
            let proto_replies_ok = proto_replies_ok.clone();
            let proto_replies_fallback = proto_replies_fallback.clone();
            let inline_counters = inline_counters.clone();
            move |msg| {
                if std::env::var_os("KIRI_DEBUG").is_some() {
                    // Debug mode must not turn into a payload logger. IPC
                    // bodies may contain application data or secrets.
                    eprintln!(
                        "[kiri-debug] IPC message uri={} body_bytes={}",
                        msg.uri(),
                        msg.body().len()
                    );
                }
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
                let Ok(mut value) = serde_json::from_str::<serde_json::Value>(msg.body()) else {
                    return;
                };
                if value.get("type").and_then(|t| t.as_str()) == Some("ipc_bench") {
                    // Merge the host-side protocol reply counters so the
                    // artifact proves which leg each reply actually took:
                    // parked drain replies, inline (hop-free) replies, and
                    // inline requests that still parked for the drain.
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "protocol_ring_replies_ok".into(),
                            serde_json::json!(proto_replies_ok.get()),
                        );
                        obj.insert(
                            "protocol_ring_replies_fallback".into(),
                            serde_json::json!(proto_replies_fallback.get()),
                        );
                        let (inline_ok, inline_fb, inline_parked) = match inline_counters.as_ref() {
                            Some(c) => (
                                c.answered_inline.load(Ordering::Relaxed),
                                c.replies_fallback.load(Ordering::Relaxed),
                                c.parked.load(Ordering::Relaxed),
                            ),
                            None => (0, 0, 0),
                        };
                        obj.insert(
                            "protocol_inline_answered_inline".into(),
                            serde_json::json!(inline_ok),
                        );
                        obj.insert(
                            "protocol_inline_replies_fallback".into(),
                            serde_json::json!(inline_fb),
                        );
                        obj.insert(
                            "protocol_inline_parked".into(),
                            serde_json::json!(inline_parked),
                        );
                    }
                    match crate::ipc_bench::write_result(ipc_bench_out.as_ref(), &value) {
                        Ok(()) => ipc_bench_done.set(true),
                        Err(e) => {
                            eprintln!("[kiri] through-webview ipc bench failed: {e}");
                            std::process::exit(1);
                        }
                    }
                    return;
                }
                if value.get("type").and_then(|t| t.as_str()) == Some("menu_smoke") {
                    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    if ok {
                        menu_smoke_done.set(true);
                    } else {
                        eprintln!("[kiri] menu smoke failed: {}", value);
                        std::process::exit(1);
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
                    ensure_host_router(
                        &router_cell,
                        &window_for_router,
                        &diagnostics,
                        &resources,
                        &options_for_router,
                        &menu_runner,
                    );
                    // Dispatch through the unified pipe gate: the permit is
                    // minted only when the capability bit AND the surface's
                    // host allowlist both admit, and the returned grant is the
                    // authority a gated reply leg would require.
                    let gated = zc_gate.lock().unwrap().dispatch_through_webview(
                        router_cell.borrow().as_ref().unwrap(),
                        caller,
                        &caller_caps,
                        &request,
                        &mut sink,
                        kiri_core::trace::MonotonicClock::now_ns(),
                    );
                    let response = gated.response;
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
                        // Recover webview_ready/dom_ready if the frame
                        // message arrived without a prior dom message
                        // (parity with the Windows host's frame fallback).
                        if !markers.borrow().has(Marker::WebViewReady) {
                            record(&markers, Marker::WebViewReady);
                        }
                        if !markers.borrow().has(Marker::DomReady) {
                            record(&markers, Marker::DomReady);
                            record(&markers, Marker::AppReady);
                        }
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

    // Parked-drain context: fetches the inline path does not answer are
    // dispatched on this thread with the same router, gate, and caller the
    // postMessage leg uses.
    let proto_ctx = ProtoDrainCtx {
        router_cell: router_cell.clone(),
        zc_gate: zc_gate.clone(),
        caller,
        caller_caps,
        diagnostics: diagnostics.clone(),
        resources: resources.clone(),
        window: window.clone(),
        options: options.clone(),
        menu_runner: menu_runner.clone(),
        markers: markers.clone(),
        replies_ok: proto_replies_ok.clone(),
        replies_fallback: proto_replies_fallback.clone(),
    };

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
        let native_menu = native_menu.clone();
        let window_for_menu = window.clone();
        menu_dispatcher
            .drain(|operation| native_menu.borrow_mut().replace(&window_for_menu, operation));
        if proto_engaged {
            drain_proto_inbox(&proto_inbox, &proto_ctx);
            if let Some(inline) = inline_ctx.as_ref() {
                merge_proto_inline_markers(&markers, inline);
            }
        }
        // IPC benchmark injection and completion are checked outside the
        // WebView callback. Wake periodically so a quiet WebView cannot starve
        // those checks after the first animation frame.
        *control_flow = if ipc_bench {
            // Benchmark injection/completion must continue even when the
            // WebView emits no native events between asynchronous callbacks.
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        };

        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *control_flow = ControlFlow::Exit;
        } else {
            // Keep the webview alive for the whole loop (owned by webview_slot).
            let _ = webview_slot.borrow();
            let _ = event;
        }

        while let Ok(menu_event) = muda::MenuEvent::receiver().try_recv() {
            if let Some((id, action)) = native_menu.borrow().action_for(&menu_event.id) {
                let payload = serde_json::json!({ "id": id, "action": action });
                if let Ok(serialized) = serde_json::to_string(&payload) {
                    if let Some(webview) = webview_slot.borrow().as_ref() {
                        let _ = webview.evaluate_script(&format!(
                            "window.kiri && window.kiri.onMenuAction && window.kiri.onMenuAction({serialized});"
                        ));
                    }
                }
            }
        }

        if smoke || ipc_bench {
            let elapsed = t0.elapsed().as_millis();
            if watchdog_ms > 0 && elapsed > watchdog_ms {
                eprintln!("[kiri] watchdog: ready state not reached within the watchdog");
                std::process::exit(2);
            }
            let has_frame = markers.borrow().has(Marker::FirstAnimationFrame);
            let ipc_ready = ipc_benchmark_ready(&markers.borrow());
            if has_frame && !smoke_armed {
                smoke_armed = true;
                frame_at = Some(Instant::now());
            }
            if ipc_bench && ipc_ready && !ipc_bench_injected.get() {
                if let Some(webview) = webview_slot.borrow().as_ref() {
                    ipc_bench_injected.set(true);
                    if std::env::var_os("KIRI_DEBUG").is_some() {
                        eprintln!("[kiri-debug] injecting IPC benchmark");
                    }
                    if ipc_bench_transport == crate::ipc_bench::IpcBenchTransport::RingZerocopy {
                        // The reusable WebView2 slot arena does not exist off
                        // Windows; run the same bench on the default wire and
                        // record the fallback honestly.
                        eprintln!(
                            "[kiri] ring_zerocopy transport requires the Windows WebView2 host; \
                             running ipc bench on the default wire"
                        );
                    }
                    let script_transport = if proto_engaged {
                        ipc_bench_transport
                    } else {
                        crate::ipc_bench::IpcBenchTransport::Default
                    };
                    let script = crate::ipc_bench::kiri_script(
                        ipc_bench_runs,
                        crate::ipc_bench::DEFAULT_WARMUP,
                        &ipc_bench_sizes,
                        script_transport,
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

    // The protocol-inline answer path and the parked drain share
    // proto_frame_response; these tests pin the frame-level contract both
    // legs obey: binary KRSL reply only under a live grant, JSON wire
    // response otherwise.
    fn ping_caps() -> kiri_core::capabilities::CapabilityBits {
        let mut c = kiri_core::capabilities::CapabilityBits::empty();
        c.set(kiri_core::dispatch::capability_bit::PING);
        c
    }

    fn ping_frame(request_id: u64, payload: &[u8]) -> Vec<u8> {
        use crate::ring_ipc as ring;
        let mut frame = vec![0u8; ring::SLOT_HEADER_BYTES + payload.len()];
        frame[ring::SLOT_HEADER_BYTES..].copy_from_slice(payload);
        frame[0..4].copy_from_slice(&ring::SLOT_MAGIC.to_le_bytes());
        frame[4] = ring::STATE_REQUEST;
        frame[6..8].copy_from_slice(&ring::CODEC_UTF8_STRING.to_le_bytes());
        frame[8..16].copy_from_slice(&9u64.to_le_bytes());
        frame[16..24].copy_from_slice(&request_id.to_le_bytes());
        frame[24..28].copy_from_slice(&kiri_core::dispatch::command_id::PING.to_le_bytes());
        frame[28..32].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame
    }

    #[test]
    fn proto_frame_response_answers_ping_on_binary_leg_under_grant() {
        use crate::ring_ipc as ring;
        let router = kiri_core::dispatch::Router::new();
        let gate = Mutex::new(kiri_core::zc_ipc_gate::ZcIpcGate::new());
        let diagnostics = Diagnostics::new();
        let resources = Mutex::new(ResourceTable::<()>::new());
        let (resp, binary) = super::proto_frame_response(
            &router,
            &gate,
            kiri_core::caller::CallerId(1),
            &ping_caps(),
            &diagnostics,
            &resources,
            &ping_frame(77, b"abc"),
        );
        assert!(binary, "granted ping must take the binary leg");
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/octet-stream");
        let body = resp.body();
        assert_eq!(&body[0..4], b"KRSL");
        assert_eq!(body[4], ring::STATE_RESPONSE);
        assert_eq!(body[5], ring::RESP_ECHO_STRING);
        assert_eq!(u64::from_le_bytes(body[16..24].try_into().unwrap()), 77);
        let plen = u32::from_le_bytes(body[28..32].try_into().unwrap()) as usize;
        assert_eq!(&body[32..32 + plen], b"abc");
    }

    #[test]
    fn proto_frame_response_denied_ping_fails_closed_to_json() {
        let router = kiri_core::dispatch::Router::new();
        let gate = Mutex::new(kiri_core::zc_ipc_gate::ZcIpcGate::new());
        let diagnostics = Diagnostics::new();
        let resources = Mutex::new(ResourceTable::<()>::new());
        let (resp, binary) = super::proto_frame_response(
            &router,
            &gate,
            kiri_core::caller::CallerId(1),
            &kiri_core::capabilities::CapabilityBits::empty(),
            &diagnostics,
            &resources,
            &ping_frame(78, b"abc"),
        );
        assert!(!binary, "denied dispatch must not take the binary leg");
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/json");
        let v: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(v["request_id"], 78);
        assert!(v.get("error").is_some());
    }

    #[test]
    fn proto_frame_response_malformed_frame_is_json_protocol_error() {
        let router = kiri_core::dispatch::Router::new();
        let gate = Mutex::new(kiri_core::zc_ipc_gate::ZcIpcGate::new());
        let diagnostics = Diagnostics::new();
        let resources = Mutex::new(ResourceTable::<()>::new());
        let (resp, binary) = super::proto_frame_response(
            &router,
            &gate,
            kiri_core::caller::CallerId(1),
            &ping_caps(),
            &diagnostics,
            &resources,
            &[0u8; 4],
        );
        assert!(!binary);
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers().get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn inline_router_covers_exactly_main_thread_free_commands() {
        // The inline router answers without the event loop, so it may only
        // know commands whose dispatch never touches window/menu/clipboard
        // or other main-thread state. Today that is exactly kiri.ping.
        let router = kiri_core::dispatch::Router::new();
        assert!(router.is_known(kiri_core::dispatch::command_id::PING));
        for cmd in kiri_core::commands::COMMANDS.iter() {
            if cmd.id != kiri_core::dispatch::command_id::PING {
                assert!(
                    !router.is_known(cmd.id),
                    "inline router must not know {} (id {}): it would dispatch off the tao thread",
                    cmd.name,
                    cmd.id
                );
            }
        }
        // Mint authority must match the production router for inline ids:
        // required_bits is what the gate checks at mint.
        let production = build_host_router(
            Arc::new(StubWindow),
            Arc::new(StubClipboard),
            &Diagnostics::new(),
            &std::sync::Arc::new(Mutex::new(ResourceTable::<()>::new())),
            &HostOptions::default(),
            Arc::new(kiri_core::app_menu::DisabledMenu),
        );
        assert_eq!(
            router.required_bits(kiri_core::dispatch::command_id::PING),
            production.required_bits(kiri_core::dispatch::command_id::PING),
            "inline and production routers must require the same bits for kiri.ping"
        );
    }

    #[test]
    fn ipc_benchmark_can_arm_after_dom_ready_without_animation_frame() {
        let mut markers = super::StartupMarkers::new();
        markers.record(super::Marker::DomReady, 10);
        assert!(super::ipc_benchmark_ready(&markers));
    }

    #[test]
    fn ipc_benchmark_does_not_arm_before_dom_or_animation_frame() {
        let markers = super::StartupMarkers::new();
        assert!(!super::ipc_benchmark_ready(&markers));
    }

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
            Arc::new(kiri_core::app_menu::DisabledMenu),
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
            Arc::new(kiri_core::app_menu::DisabledMenu),
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
