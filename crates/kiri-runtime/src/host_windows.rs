//! Direct Win32 + WebView2 host (docs/06-windows.md, WINDOWS_MVP WP1).
//!
//! Threading rule (docs/03-threading-and-lifecycle.md): all window, COM, and
//! WebView2 work stays on the main UI thread. The WebView2 runtime delivers
//! callbacks by posting messages to that thread, so the message loop (or
//! `wait_for_async_operation`'s pump) must be running for creation callbacks
//! to fire.
//!
//! Bindings: `webview2-com` 0.39.1 (verified 2026-08-13) + `windows` 0.62. On
//! MSVC, webview2-com-sys links the WebView2LoaderStatic archive, so no
//! WebView2Loader.dll copy is needed (see docs/DECISIONS.md D-002).

#![cfg(target_os = "windows")]

use std::path::{Component, Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, PeekMessageW, PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, MSG,
    PM_REMOVE, SW_SHOW, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use kiri_core::caller::{CallerId, CallerRegistry};
use kiri_core::capabilities::CapabilityBits;
use kiri_core::diagnostics::Diagnostics;
use kiri_core::dispatch::Router;
use kiri_core::error::Error as KiriError;
use kiri_core::platform::EventBus;
use kiri_core::resources::ResourceTable;
use kiri_core::wire::{WireRequest, WireResponse};

use crate::markers::{Marker, StartupMarkers};
use crate::HostOptions;
/// Convert a null-terminated wide string to `String` (copy, does not free).
fn pwstr_to_string(ptr: windows::core::PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while unsafe { *ptr.0.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr.0, len) };
    String::from_utf16_lossy(slice)
}
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Run a script in the WebView and ignore its result. `webview2-com`
/// `ExecuteScript` needs a NUL-terminated wide-string script and a
/// completion handler.
fn exec_script(webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, js: &str) {
    let wide = to_wide(js);
    let handler = webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
        |_result: windows::core::Result<()>, _output: String| Ok(()),
    ));
    let _ =
        unsafe { webview.ExecuteScript(windows::core::PCWSTR::from_raw(wide.as_ptr()), &handler) };
}

/// Reply to a control-plane request without `ExecuteScript`. Calling
/// `ExecuteScript` from inside `WebMessageReceived` can stall on WebView2
/// (the through-webview IPC bench timed out on the first ping on
/// windows-latest). `PostWebMessageAsJson` is the documented reply path.
/// Payloads over 64KiB use `PostSharedBufferToScript` (T008) when the
/// runtime supports `ICoreWebView2Environment12`.
fn post_wire_response(
    env: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    response: &WireResponse,
) -> bool {
    if let Ok(json) = serde_json::to_string(response) {
        if json.len() > 64 * 1024 && post_shared_buffer(env, webview, json.as_bytes()) {
            return true;
        }
        let wide = to_wide(&json);
        let _ = unsafe { webview.PostWebMessageAsJson(PCWSTR::from_raw(wide.as_ptr())) };
        return false;
    }
    false
}

/// Copy `bytes` into a WebView2 shared buffer and hand it to script.
/// Returns false when the runtime is too old or the copy fails so the
/// caller can fall back to `PostWebMessageAsJson`.
fn post_shared_buffer(
    env: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    bytes: &[u8],
) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment12, ICoreWebView2_17, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
    };
    let Ok(env12) = env.cast::<ICoreWebView2Environment12>() else {
        return false;
    };
    let Ok(webview17) = webview.cast::<ICoreWebView2_17>() else {
        return false;
    };
    let Ok(buffer) = (unsafe { env12.CreateSharedBuffer(bytes.len() as u64) }) else {
        return false;
    };
    let mut dest: *mut u8 = std::ptr::null_mut();
    if unsafe { buffer.Buffer(&mut dest) }.is_err() || dest.is_null() {
        let _ = unsafe { buffer.Close() };
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len()) };
    let extra = to_wide(r#"{"kind":"wire","shared_buffer":true}"#);
    let posted = unsafe {
        webview17.PostSharedBufferToScript(
            &buffer,
            COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
            PCWSTR::from_raw(extra.as_ptr()),
        )
    };
    let _ = unsafe { buffer.Close() };
    posted.is_ok()
}

/// Serve a packed or disk asset for `kiri://localhost/*`.
fn handle_app_resource(
    env: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    args: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequestedEventArgs,
    frontend_dir: Option<&Path>,
) {
    use crate::assets::{
        response_headers, serve_checked, serve_embedded, status_code, ServeOptions,
    };
    use windows::Win32::UI::Shell::SHCreateMemStream;

    let Ok(request) = (unsafe { args.Request() }) else {
        return;
    };
    let mut uri = windows::core::PWSTR(std::ptr::null_mut());
    if unsafe { request.Uri(&mut uri) }.is_err() {
        return;
    }
    let url = pwstr_to_string(uri);
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(uri.0 as _)) };
    if !is_app_origin_url(&url) {
        return;
    }
    let path = crate::embed::request_path_from_https_origin(&url, APP_ORIGIN)
        .unwrap_or_else(|| "index.html".into());
    let opts = ServeOptions::default();
    let served = match frontend_dir {
        Some(root) => serve_checked(root, &path, &opts),
        None => serve_embedded(&path, &opts),
    };
    let status = i32::from(status_code(&served));
    let header_lines: Vec<String> =
        response_headers(&served).into_iter().map(|(k, v)| format!("{k}: {v}")).collect();
    let header_blob = header_lines.join("\r\n");
    let body: &[u8] = match &served {
        crate::assets::AssetResponse::Full { body, .. }
        | crate::assets::AssetResponse::Partial { body, .. } => body.as_slice(),
        _ => b"",
    };
    let Some(stream) = (unsafe { SHCreateMemStream(Some(body)) }) else {
        return;
    };
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        304 => "Not Modified",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "OK",
    };
    let reason_w = to_wide(reason);
    let headers_w = to_wide(&header_blob);
    let Ok(response) = (unsafe {
        env.CreateWebResourceResponse(
            &stream,
            status,
            PCWSTR::from_raw(reason_w.as_ptr()),
            PCWSTR::from_raw(headers_w.as_ptr()),
        )
    }) else {
        return;
    };
    let _ = unsafe { args.SetResponse(&response) };
}

/// The Windows native host: window, WebView2 lifecycle, and message loop.
pub struct WindowsHost;

/// Live Windows document origin. Same custom scheme as macOS/Linux so the
/// WebView2 HTTPS network stack is not initialized for first paint.
pub(crate) const APP_ORIGIN: &str = kiri_core::security::CROSS_APP_ORIGIN;
pub(crate) const FRONTEND_PAGE: &str = "index.html";

/// True for URLs served from the application origin (used to reject markers
/// from `about:blank` or any other origin).
fn is_app_origin_url(url: &str) -> bool {
    kiri_core::security::is_app_origin(url)
}

/// Timer IDs for the smoke-exit and watchdog paths.
const SMOKE_TIMER_ID: usize = 1;
const WATCHDOG_TIMER_ID: usize = 2;

/// Exit code for watchdog timeout in smoke/stress runs.
const EXIT_WATCHDOG: i32 = 2;

/// Monotonic clock in nanoseconds based on QueryPerformanceCounter.
pub(crate) fn qpc_now_ns() -> u64 {
    let mut freq: i64 = 0;
    let mut counter: i64 = 0;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut counter);
    }
    let freq = freq.max(1) as u128;
    let counter = counter.max(0) as u128;
    (counter * 1_000_000_000 / freq) as u64
}

/// Resolve a (possibly relative) directory to a lexical absolute path without
/// touching the filesystem. WebView2's `SetVirtualHostNameToFolderMapping`
/// requires an absolute folder path; `std::fs::canonicalize` would be fine
/// too but returns `\\?\`-prefixed paths on Windows.
fn absolute_lexical(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Per-run state stored behind the window's GWLP_USERDATA. Single-threaded by
/// design: only the UI thread touches it.
pub(crate) struct HostRuntime {
    pub options: HostOptions,
    pub markers: StartupMarkers,
    pub hwnd: HWND,
    pub caller: CallerId,
    pub caller_caps: CapabilityBits,
    /// Built on first `window.kiri.send()`, never before WebView2 init.
    pub router: Option<Router>,
    pub events: EventBus,
    pub diagnostics: Diagnostics,
    pub resources: std::sync::Arc<std::sync::Mutex<ResourceTable<()>>>,
    pub env: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    pub controller: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    pub webview: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    pub webview_token: i64,
    pub navigation_token: i64,
    pub resource_token: i64,
    /// Set once the first animation frame arrived in smoke mode; the smoke
    /// timer then posts WM_QUIT after `exit_after_ready_ms`.
    pub smoke_armed: bool,
    /// Set once the through-webview IPC bench script has been injected.
    pub ipc_bench_injected: bool,
    pub exit_code: i32,
    /// T008: replies that went through `PostSharedBufferToScript`.
    pub shared_buffer_ok: u32,
    /// T008: replies over 64 KiB that fell back to JSON.
    pub shared_buffer_fallback: u32,
}

impl HostRuntime {
    /// Reply to an unknown web message with a Kiri protocol error. This is
    /// the bridge stub: the real control plane (T003) replaces it.
    fn reply_protocol_error(&self, request_id: Option<u64>, command: Option<&str>) {
        let error = KiriError::protocol_error("command not implemented by this host slice")
            .with_command(command.unwrap_or("").to_string());
        let envelope = WireResponse::err(request_id.unwrap_or(0), error);
        let _ = post_wire_response(&self.env, &self.webview, &envelope);
    }

    /// Build the core plugin router (ping/diag/resources) on first send.
    fn ensure_core_router(&mut self) {
        if self.router.is_some() {
            return;
        }
        let started = qpc_now_ns();
        let router = crate::plugins::PluginHost::build_router_with_plugins(
            &self.diagnostics,
            &self.resources,
            self.caller,
            &crate::plugins::PluginManifest::empty(),
            &crate::plugins::PluginRegistry::empty(),
        );
        eprintln!(
            "[kiri] lazy-router: core plugins in {} ms",
            (qpc_now_ns().saturating_sub(started)) / 1_000_000
        );
        self.router = Some(router);
    }

    /// Attach the surface that owns `command_id` if it is not registered yet.
    fn ensure_surface(&mut self, command_id: u32) {
        use crate::router_surfaces::{surface_for_command, Surface};
        self.ensure_core_router();
        let Some(surface) = surface_for_command(command_id) else {
            return;
        };
        if surface == Surface::Core {
            return;
        }
        if self.router.as_ref().is_some_and(|r| r.is_known(command_id)) {
            return;
        }
        let started = qpc_now_ns();
        let router = self.router.take().unwrap_or_else(Router::new_empty);
        self.router = Some(attach_windows_surface(router, self, surface));
        eprintln!(
            "[kiri] lazy-router: attached {surface:?} in {} ms",
            (qpc_now_ns().saturating_sub(started)) / 1_000_000
        );
    }

    fn dispatch_cmd(&mut self, request: &WireRequest) -> WireResponse {
        if !self.markers.has(Marker::FirstInvokeDispatched) {
            self.markers.record(Marker::FirstInvokeDispatched, qpc_now_ns());
        }
        self.ensure_surface(request.command_id);
        let router = self.router.as_ref().expect("router after ensure_surface");
        let mut sink = self.diagnostics.clone();
        let response = router.dispatch(self.caller, &self.caller_caps, request, &mut sink);
        if !self.markers.has(Marker::FirstInvokeResponded) {
            self.markers.record(Marker::FirstInvokeResponded, qpc_now_ns());
        }
        response
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // The runtime stays alive until teardown after the message loop
            // ends; destroying the window posts WM_QUIT via WM_DESTROY.
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            let id = wparam.0;
            let Some(rt) = (unsafe { get_runtime(hwnd) }) else {
                return LRESULT(0);
            };
            if id == WATCHDOG_TIMER_ID {
                eprintln!(
                    "[kiri] watchdog: ready state not reached within {} ms",
                    rt.options.watchdog_ms
                );
                rt.exit_code = EXIT_WATCHDOG;
                unsafe { PostQuitMessage(EXIT_WATCHDOG) };
            } else if id == SMOKE_TIMER_ID && rt.smoke_armed {
                unsafe { PostQuitMessage(0) };
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn get_runtime(hwnd: HWND) -> Option<&'static mut HostRuntime> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if ptr == 0 {
        None
    } else {
        Some(&mut *(ptr as *mut HostRuntime))
    }
}

impl WindowsHost {
    /// Run one full host session: window, WebView2 lifecycle, bridge script,
    /// navigation, and message loop until quit. Returns the recorded startup
    /// markers.
    ///
    /// In `smoke` mode the host exits by itself shortly after the first
    /// animation frame marker; otherwise it runs until the window closes.
    pub fn run(options: &HostOptions) -> Result<StartupMarkers, i32> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(|e| {
                eprintln!("[kiri] CoInitializeEx: {e}");
                1
            })?;
            let result = run_host_inner(options);
            CoUninitialize();
            match result {
                Ok(markers) => Ok(markers),
                Err(err) => {
                    eprintln!("[kiri] host error: {err}");
                    Err(1)
                }
            }
        }
    }
}

/// Host allowlist of tray menu item ids for the native tray (audit item 14).
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

unsafe fn run_host_inner(options: &HostOptions) -> Result<StartupMarkers, String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    };
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler,
        CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler,
        WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
    };

    let mut markers = StartupMarkers::new();
    markers.record(Marker::ProcessSpawnRequested, qpc_now_ns());
    markers.record(Marker::NativeEntry, qpc_now_ns());

    // Control-plane router for the native bridge (T003). Caller identity is
    // assigned by the native runtime, never by JavaScript; grant the ping
    // capability so control commands run from the trusted frontend.
    let mut registry = CallerRegistry::new();
    let caller = registry.register();
    let caller_caps = kiri_core::security::trusted_frontend_capabilities();
    let diagnostics = Diagnostics::new();
    let events = EventBus::new();
    // Shared generational resource table owned by the host. The resources plugin
    // binds this exact instance via the ABI context, so kiri.open/kiri.close
    // mutate it and keep the diagnostics open-resource count honest and dynamic.
    let resources: std::sync::Arc<std::sync::Mutex<ResourceTable<()>>> =
        std::sync::Arc::new(std::sync::Mutex::new(ResourceTable::<()>::new()));

    // ---- window creation (W0: native host) ----
    let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let hinstance = windows::Win32::Foundation::HINSTANCE(hmodule.0);
    let class_name = windows::core::HSTRING::from("KiriHostClass");
    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&window_class) == 0 {
        let last_error = windows::Win32::Foundation::GetLastError();
        // The class survives for the lifetime of the process; in-process
        // cycles (kiri-host-stress) hit ERROR_CLASS_ALREADY_EXISTS, which is
        // not an error for us.
        if last_error != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassW failed: {last_error:?}"));
        }
    }

    let title = windows::core::HSTRING::from(options.title.as_str());
    let hwnd = CreateWindowExW(
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
        PCWSTR::from_raw(class_name.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        options.width as i32,
        options.height as i32,
        None,
        None,
        Some(hinstance),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))?;
    markers.record(Marker::PlatformInit, qpc_now_ns());

    // Lazy Router (research #1/#7): plugin surfaces attach on first
    // `window.kiri.send()`, never before WebView2 environment creation.
    // Smoke (blank frontend) never invokes, so this gap is the 1–2s we
    // used to spend building fs/http/shell/tray/sidecar/... on the
    // critical path.
    markers.record(Marker::WebViewCreationRequested, qpc_now_ns());
    let env = {
        use std::{cell::RefCell, rc::Rc};
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment;
        let env_cell: Rc<RefCell<Option<ICoreWebView2Environment>>> = Rc::new(RefCell::new(None));
        let user_data = {
            let root = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Kiri")
                .join("WebView2");
            let _ = std::fs::create_dir_all(&root);
            windows::core::HSTRING::from(root.as_os_str())
        };
        let env_opts = webview2_com::CoreWebView2EnvironmentOptions::default();
        unsafe {
            let args = std::env::var("KIRI_WEBVIEW2_ARGS").unwrap_or_default();
            env_opts.set_additional_browser_arguments(args);
            env_opts.set_enable_tracking_prevention(false);
            env_opts.set_are_browser_extensions_enabled(false);
            // First-class `kiri://` scheme at environment create (research #2).
            // First-class `kiri://` scheme at environment create. The live
            // document navigates to kiri://localhost so Chromium does not
            // start the HTTPS network stack for first paint.
            let scheme = webview2_com::CoreWebView2CustomSchemeRegistration::new("kiri".into());
            scheme.set_treat_as_secure(true);
            scheme.set_has_authority_component(true);
            let scheme: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2CustomSchemeRegistration =
                scheme.into();
            env_opts.set_scheme_registrations(vec![Some(scheme)]);
        }
        let env_opts: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2EnvironmentOptions =
            env_opts.into();
        let done = CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        PCWSTR::from_raw(user_data.as_ptr()),
                        Some(&env_opts),
                        &handler,
                    )
                }
                .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new({
                let cell = env_cell.clone();
                move |result, environment| {
                    result?;
                    *cell.borrow_mut() = environment;
                    Ok(())
                }
            }),
        );
        done.map_err(|e| format!("environment creation failed: {e}"))?;
        let maybe_env = env_cell.borrow().clone();
        maybe_env.ok_or("environment creation returned no environment")?
    };
    let browser_version = {
        let mut version = windows::core::PWSTR(std::ptr::null_mut());
        let hr = unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::
            GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version)
        };
        if hr.is_ok() && !version.is_null() {
            let text = pwstr_to_string(version);
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(version.0 as _)) };
            Some(text)
        } else {
            None
        }
    };

    // ---- controller ----
    let controller = {
        use std::{cell::RefCell, rc::Rc};
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
        let controller_cell: Rc<RefCell<Option<ICoreWebView2Controller>>> =
            Rc::new(RefCell::new(None));
        let env_for_controller = env.clone();
        let done = CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                env_for_controller
                    .CreateCoreWebView2Controller(hwnd, &handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new({
                let cell = controller_cell.clone();
                move |result, controller| {
                    result?;
                    *cell.borrow_mut() = controller;
                    Ok(())
                }
            }),
        );
        done.map_err(|e| format!("controller creation failed: {e}"))?;
        let maybe_controller = controller_cell.borrow().clone();
        maybe_controller.ok_or("controller creation returned no controller")?
    };

    let webview = controller.CoreWebView2().map_err(|e| format!("controller.CoreWebView2: {e}"))?;

    let mut client_rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut client_rect);
    controller.SetBounds(client_rect).map_err(|e| format!("SetBounds: {e}"))?;
    controller.SetIsVisible(true).map_err(|e| format!("SetIsVisible: {e}"))?;

    // ---- bridge script (bridge-ready marker) ----
    // The bridge runs on every document including the initial `about:blank`
    // page; it must only emit markers from the application origin.
    let bridge_script = r#"
        (function () {
          if (window.location.origin !== 'kiri://localhost') { return; }
          window.kiri = {
            pending: Object.create(null),
            post: function (o) { window.chrome.webview.postMessage(o); },
            send: function (req) { window.chrome.webview.postMessage({ type: 'cmd', request: req }); },
            onResponse: function (resp) {
              var id = resp && resp.request_id;
              var cb = window.kiri.pending[id];
              if (cb) {
                delete window.kiri.pending[id];
                cb(resp);
              }
            }
          };
          if (window.chrome && window.chrome.webview && window.chrome.webview.addEventListener) {
            window.chrome.webview.addEventListener('message', function (e) {
              var d = e.data;
              if (typeof d === 'string') {
                try { d = JSON.parse(d); } catch (err) { return; }
              }
              if (d && d.request_id !== undefined) {
                window.kiri.onResponse(d);
              }
            });
            window.chrome.webview.addEventListener('sharedbufferreceived', function (e) {
              try {
                var buf = e.getBuffer();
                var text = new TextDecoder('utf-8').decode(new Uint8Array(buf));
                window.chrome.webview.releaseBuffer(buf);
                var d = JSON.parse(text);
                if (d && d.request_id !== undefined) {
                  window.kiri.onResponse(d);
                }
              } catch (err) {}
            });
          }
          function postDom() {
            window.kiri.post({ type: 'ready', phase: 'dom' });
          }
          if (document.readyState === 'loading') {
            document.addEventListener('DOMContentLoaded', postDom);
          } else {
            postDom();
          }
          requestAnimationFrame(function () {
            window.kiri.post({ type: 'ready', phase: 'frame' });
          });
        })();
    "#;
    let script_h = windows::core::HSTRING::from(bridge_script);
    let script_pwstr = PCWSTR::from_raw(script_h.as_ptr());
    let webview_for_script = webview.clone();
    {
        let done = AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                webview_for_script
                    .AddScriptToExecuteOnDocumentCreated(script_pwstr, &handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(|result, _script_id| {
                result?;
                Ok(())
            }),
        );
        done.map_err(|e| format!("AddScriptToExecuteOnDocumentCreated: {e}"))?;
        markers.record(Marker::BridgeReady, qpc_now_ns());
    }

    // Serve kiri://localhost from memory (embed) or the optional disk
    // frontend. Virtual-host https mapping is gone: it still paid the
    // Chromium network-stack tax on first navigation.
    let mut resource_token: i64 = 0;
    let frontend_dir = match options.frontend_dir.as_ref() {
        Some(dir) => {
            let abs = absolute_lexical(dir);
            if !abs.is_dir() {
                return Err(format!("frontend directory does not exist: {}", abs.display()));
            }
            Some(abs)
        }
        None => None,
    };
    for pattern in [format!("{APP_ORIGIN}/*"), APP_ORIGIN.to_string()] {
        let filter = to_wide(&pattern);
        webview
            .AddWebResourceRequestedFilter(
                PCWSTR::from_raw(filter.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
            .map_err(|e| format!("AddWebResourceRequestedFilter {pattern}: {e}"))?;
    }
    let env_for_res = env.clone();
    let dir_for_res = frontend_dir.clone();
    let res_handler = WebResourceRequestedEventHandler::create(Box::new(move |_s, args| {
        if let Some(args) = args {
            handle_app_resource(&env_for_res, args, dir_for_res.as_deref());
        }
        Ok(())
    }));
    webview
        .add_WebResourceRequested(&res_handler, &mut resource_token)
        .map_err(|e| format!("add_WebResourceRequested: {e}"))?;

    // ---- event handlers ----
    let nav_handler = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
        let Some(args) = args else {
            eprintln!("[kiri] NavigationCompleted: handler fired without args");
            return Ok(());
        };
        let mut ok = windows::core::BOOL::default();
        if args.IsSuccess(&mut ok).is_err() || !ok.as_bool() {
            eprintln!("[kiri] NavigationCompleted: navigation failed");
            return Ok(());
        }
        // Only the application-origin navigation counts as webview_ready;
        // the initial about:blank navigation completes before any navigate
        // and must not arm the startup contract.
        let is_app = sender.is_some_and(
            |s: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2| {
                let mut source = windows::core::PWSTR(std::ptr::null_mut());
                if unsafe { s.Source(&mut source) }.is_err() {
                    return false;
                }
                let source_copy = pwstr_to_string(source);
                unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(source.0 as _)) }
                is_app_origin_url(&source_copy)
            },
        );
        if is_app {
            if let Some(rt) = unsafe { get_runtime(hwnd) } {
                // First signal wins: when delivery is prompt this is the real
                // navigation completion (before dom); when the event lags the
                // dom message, the fallback in handle_web_message already
                // recorded at dom time, which is closer to the true value
                // than the late delivery timestamp.
                if !rt.markers.has(Marker::WebViewReady) {
                    rt.markers.record(Marker::WebViewReady, qpc_now_ns());
                }
            } else {
                eprintln!("[kiri] NavigationCompleted: runtime missing");
            }
        }
        Ok(())
    }));
    let mut navigation_token: i64 = 0;
    webview
        .add_NavigationCompleted(&nav_handler, &mut navigation_token)
        .map_err(|e| format!("add_NavigationCompleted: {e}"))?;

    let msg_handler = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            handle_web_message(args, hwnd);
        }
        Ok(())
    }));
    let mut webview_token: i64 = 0;
    webview
        .add_WebMessageReceived(&msg_handler, &mut webview_token)
        .map_err(|e| format!("add_WebMessageReceived: {e}"))?;

    // ---- runtime state behind the window ----
    let runtime = Box::new(HostRuntime {
        options: options.clone(),
        markers,
        hwnd,
        caller,
        caller_caps,
        router: None,
        events,
        diagnostics,
        resources,
        env,
        controller,
        webview,
        webview_token,
        navigation_token,
        resource_token,
        smoke_armed: false,
        ipc_bench_injected: false,
        exit_code: 0,
        shared_buffer_ok: 0,
        shared_buffer_fallback: 0,
    });
    if let Some(version) = browser_version {
        eprintln!("[kiri] WebView2 runtime version: {version}");
    }
    // Watchdog armed for smoke runs so CI cannot hang.
    if (runtime.options.smoke || runtime.options.ipc_bench) && runtime.options.watchdog_ms > 0 {
        let _ =
            unsafe { SetTimer(Some(hwnd), WATCHDOG_TIMER_ID, runtime.options.watchdog_ms, None) };
    }
    // The runtime pointer is owned by this function; it must not be read
    // back from the window after WM_CLOSE destroyed it.
    let runtime_ptr = Box::into_raw(runtime);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, runtime_ptr as isize);

    // ---- navigate to the local application origin ----
    let url = format!("{APP_ORIGIN}/{FRONTEND_PAGE}");
    eprintln!("[kiri] navigate {url}");
    let url_h = windows::core::HSTRING::from(url.as_str());
    if let Some(rt) = unsafe { get_runtime(hwnd) } {
        rt.webview
            .Navigate(PCWSTR::from_raw(url_h.as_ptr()))
            .map_err(|e| format!("Navigate: {e}"))?;
    } else {
        return Err("runtime not installed before navigate".into());
    }

    let _ = ShowWindow(hwnd, SW_SHOW);

    // ---- message loop ----
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        let _ = DispatchMessageW(&msg);
    }

    // ---- teardown (docs/03 shutdown sequence) ----
    let exit_code = unsafe { (*runtime_ptr).exit_code };
    let rt = unsafe { Box::from_raw(runtime_ptr) };
    let _ = rt.webview.remove_WebMessageReceived(rt.webview_token);
    let _ = rt.webview.remove_NavigationCompleted(rt.navigation_token);
    if rt.resource_token != 0 {
        let _ = rt.webview.remove_WebResourceRequested(rt.resource_token);
    }
    let _ = rt.controller.Close();
    let markers_out = rt.markers;
    let _ = rt.env; // explicit drop before CoUninitialize (called by run)
                    // Destroy the hosting window; in smoke mode the loop exits on WM_QUIT
                    // from the smoke timer, never via WM_CLOSE, so the window must be torn
                    // down here. WM_DESTROY posts WM_QUIT.
    let _ = DestroyWindow(hwnd);
    // Drain posted messages (WM_QUIT, WebView2 controller-close completion)
    // so an in-process next cycle starts with a clean queue; a stale WM_QUIT
    // makes the next wait_with_pump return TaskCanceled.
    let mut drained = MSG::default();
    while unsafe { PeekMessageW(&mut drained, None, 0, 0, PM_REMOVE).as_bool() } {}

    if exit_code != 0 {
        return Err(format!("host exited with code {exit_code}"));
    }
    Ok(markers_out)
}

/// Handle one WebView2 web message. Ready-phase messages drive the startup
/// markers; anything else receives a protocol error (bridge stub).
fn handle_web_message(
    args: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebMessageReceivedEventArgs,
    hwnd: HWND,
) {
    // Origin check: only messages from the application origin are handled.
    // The bridge script runs on about:blank too; its messages must be
    // rejected here as defense in depth.
    let mut source = windows::core::PWSTR(std::ptr::null_mut());
    if unsafe { args.Source(&mut source) }.is_err() {
        return;
    }
    let source_copy = pwstr_to_string(source);
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(source.0 as _)) };
    if !is_app_origin_url(&source_copy) {
        return;
    }

    let mut message = windows::core::PWSTR(std::ptr::null_mut());
    if unsafe { args.WebMessageAsJson(&mut message) }.is_err() {
        return;
    }
    let json = pwstr_to_string(message);
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(message.0 as _)) };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return;
    };

    let Some(rt) = (unsafe { get_runtime(hwnd) }) else {
        return;
    };

    if value.get("type").and_then(|t| t.as_str()) == Some("ipc_bench") {
        let mut report = value;
        if let Some(obj) = report.as_object_mut() {
            obj.insert("shared_buffer_replies_ok".into(), serde_json::json!(rt.shared_buffer_ok));
            obj.insert(
                "shared_buffer_replies_fallback".into(),
                serde_json::json!(rt.shared_buffer_fallback),
            );
        }
        match crate::ipc_bench::write_result(rt.options.ipc_bench_out.as_ref(), &report) {
            Ok(()) => unsafe { PostQuitMessage(0) },
            Err(e) => {
                eprintln!("[kiri] through-webview ipc bench failed: {e}");
                rt.exit_code = 1;
                unsafe { PostQuitMessage(1) };
            }
        }
        return;
    }

    // Control-plane command: dispatch through kiri-core and post the
    // wire response back to the page (T003).
    if let Some(req_val) = value.get("request") {
        match serde_json::from_value::<WireRequest>(req_val.clone()) {
            Ok(request) => {
                let response = rt.dispatch_cmd(&request);
                rt.diagnostics.set_open_resources(rt.resources.lock().unwrap().len() as u32);
                let used = post_wire_response(&rt.env, &rt.webview, &response);
                if used {
                    rt.shared_buffer_ok = rt.shared_buffer_ok.saturating_add(1);
                } else if serde_json::to_string(&response).map(|s| s.len()).unwrap_or(0) > 64 * 1024
                {
                    rt.shared_buffer_fallback = rt.shared_buffer_fallback.saturating_add(1);
                }
            }
            Err(_) => {
                let err =
                    WireResponse::err(0, KiriError::protocol_error("malformed command request"));
                let _ = post_wire_response(&rt.env, &rt.webview, &err);
            }
        }
        return;
    }

    if value.get("type").and_then(|t| t.as_str()) == Some("ready") {
        let phase = value.get("phase").and_then(|p| p.as_str());
        match phase {
            Some("dom") => {
                // The page parsed, so its navigation completed; if the
                // NavigationCompleted event was delivered late (observed on
                // WebView2 150/151: delivery can lag the frame message past
                // the smoke exit), recover the marker here.
                if !rt.markers.has(Marker::WebViewReady) {
                    rt.markers.record(Marker::WebViewReady, qpc_now_ns());
                }
                rt.markers.record(Marker::DomReady, qpc_now_ns());
                rt.markers.record(Marker::AppReady, qpc_now_ns());
            }
            Some("frame") => {
                rt.markers.record(Marker::FirstAnimationFrame, qpc_now_ns());
                if rt.options.ipc_bench && !rt.ipc_bench_injected {
                    rt.ipc_bench_injected = true;
                    let script = crate::ipc_bench::kiri_script(
                        rt.options.ipc_bench_runs,
                        crate::ipc_bench::DEFAULT_WARMUP,
                        &rt.options.ipc_bench_sizes,
                    );
                    exec_script(&rt.webview, &script);
                } else if rt.options.smoke && !rt.smoke_armed {
                    rt.smoke_armed = true;
                    let _ = unsafe {
                        SetTimer(Some(hwnd), SMOKE_TIMER_ID, rt.options.exit_after_ready_ms, None)
                    };
                }
            }
            _ => {}
        }
        return;
    }

    let request_id = value.get("requestId").and_then(|r| r.as_u64());
    let command = value.get("command").and_then(|c| c.as_str());
    rt.reply_protocol_error(request_id, command);
}

/// Attach one production surface to an already-built core router.
fn attach_windows_surface(
    router: Router,
    rt: &HostRuntime,
    surface: crate::router_surfaces::Surface,
) -> Router {
    use crate::router_surfaces::Surface;
    match surface {
        Surface::Core => router,
        Surface::Platform => router.with_platform(rt.events.clone()),
        Surface::Fs => {
            let mut fs_scope =
                kiri_core::capabilities::PathScope::new(std::env::temp_dir().join("kiri-fs"));
            fs_scope.read = true;
            fs_scope.write = true;
            let _ = std::fs::create_dir_all(&fs_scope.root);
            router.with_fs_service(
                kiri_core::fs::FsService::new(fs_scope, kiri_core::limits::Limits::default())
                    .with_glob(kiri_core::capabilities::GlobScope::new(fs_glob_patterns())),
            )
        }
        Surface::Window => router.with_window(
            std::sync::Arc::new(crate::window_ctl::WinWindowController::new(rt.hwnd)),
            std::sync::Arc::new(std::sync::Mutex::new(kiri_core::window::WindowState::new(
                &rt.options.title,
            ))),
        ),
        Surface::Clipboard => router.with_clipboard(
            std::sync::Arc::new(
                crate::clipboard_ctl::WinClipboardController::new().expect("clipboard init"),
            ),
            std::sync::Arc::new(std::sync::Mutex::new(kiri_core::clipboard::ClipboardState::new())),
        ),
        Surface::Path => {
            router.with_path(kiri_core::path::PathService::new(kiri_core::path::PathState::new()))
        }
        Surface::Http => router.with_http(kiri_core::http::HttpService::new(
            std::sync::Arc::new(kiri_core::http::StdHttpClient),
            kiri_core::http::HostAllowlist::new(http_allow_hosts()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Shell => router.with_shell(kiri_core::shell::ShellService::new(
            std::sync::Arc::new(crate::shell_ctl::WinShellRunner::new()),
            kiri_core::shell::ShellAllowlist::new(shell_allow_commands()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Notification => {
            router.with_notification(kiri_core::notification::NotificationService::new(
                std::sync::Arc::new(
                    crate::notification_ctl::win_notify::WinNotificationRunner::new(),
                ),
                kiri_core::notification::NotificationAllowlist::new(notification_templates()),
                kiri_core::limits::Limits::default(),
            ))
        }
        Surface::Dialog => router.with_dialog(kiri_core::dialog::DialogService::new(
            std::sync::Arc::new(crate::dialog_ctl::WinDialogRunner::new()),
            kiri_core::dialog::DialogAllowlist::new(dialog_templates()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Shortcut => router.with_shortcut(kiri_core::shortcut::ShortcutService::new(
            std::sync::Arc::new(crate::shortcut_ctl::WinShortcutRunner::new()),
            kiri_core::shortcut::ShortcutAllowlist::new(shortcut_bindings()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Autostart => router.with_autostart(kiri_core::autostart::AutostartService::new(
            std::sync::Arc::new(crate::autostart_ctl::WinAutostartRunner::new()),
            kiri_core::autostart::AutostartAllowlist::new(autostart_policy()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Store => router.with_store(kiri_core::store::StoreService::new(
            std::sync::Arc::new(crate::store_ctl::WinStoreBackend::new()),
            kiri_core::store::StoreAllowlist::new(store_namespaces()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Deeplink => router.with_deeplink(kiri_core::deeplink::DeeplinkService::new(
            std::sync::Arc::new(crate::deeplink_ctl::win_deeplink::WinDeeplinkRunner::new()),
            kiri_core::deeplink::DeeplinkAllowlist::new(deeplink_schemes()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Opener => router.with_opener(kiri_core::opener::OpenerService::new(
            std::sync::Arc::new(crate::opener_ctl::win_opener::WinOpenerRunner::new()),
            kiri_core::opener::OpenerAllowlist::new(opener_url_schemes(), opener_file_extensions()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::WindowState => {
            router.with_window_state(kiri_core::window_state::WindowStateService::new(
                std::sync::Arc::new(
                    crate::window_state_ctl::win_window_state::WinWindowStateBackend::new(),
                ),
                kiri_core::limits::Limits::default(),
            ))
        }
        Surface::Tray => router.with_tray(kiri_core::tray::TrayService::new(
            std::sync::Arc::new(crate::tray_ctl::win_tray::WinTrayBackend::new()),
            kiri_core::tray::TrayAllowlist::new(tray_items()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Sidecar => router.with_sidecar(kiri_core::sidecar::SidecarService::new(
            std::sync::Arc::new(crate::sidecar_ctl::win_sidecar::WinSidecarRunner::new()),
            kiri_core::sidecar::SidecarAllowlist::new(sidecar_allow()),
            kiri_core::sidecar::SidecarTable::new(),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Event => router.with_event(kiri_core::event::EventService::new(
            std::sync::Arc::new(rt.events.clone()),
            kiri_core::event::EventAllowlist::new(event_channels()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Config => router.with_config(kiri_core::config::ConfigService::new(
            std::sync::Arc::new(kiri_core::config::MapConfigBackend::new({
                let mut m = std::collections::HashMap::new();
                m.insert("app.name".to_string(), serde_json::json!("Kiri"));
                m.insert("app.version".to_string(), serde_json::json!(env!("CARGO_PKG_VERSION")));
                m.insert("window.theme".to_string(), serde_json::json!("system"));
                m
            })),
            kiri_core::config::ConfigAllowlist::new(config_keys()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Updater => router.with_updater(
            kiri_core::updater_surface::UpdaterService::new(
                HOST_PINNED_UPDATE_PUBLIC_KEY,
                kiri_core::update::Version::parse(env!("CARGO_PKG_VERSION"))
                    .expect("valid package version"),
                kiri_core::limits::Limits::default(),
            )
            .with_feed(crate::update_feed::fetch_pinned_release_manifest),
        ),
        Surface::Cli => router
            .with_cli(kiri_core::cli::CliService::new(std::env::args().collect::<Vec<String>>())),
        Surface::FsWatch => router.with_fs_watch(kiri_core::fs_watch::FsWatchService::new(
            std::sync::Arc::new(crate::fs_watch_ctl::NativeFsWatchBackend::new()),
            kiri_core::fs_watch::FsWatchAllowlist::new(crate::host_policy::fs_watch_targets()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Ws => router.with_ws(kiri_core::websocket::WsService::new(
            std::sync::Arc::new(crate::ws_ctl::NativeWsBackend::new()),
            kiri_core::websocket::WsAllowlist::new(crate::host_policy::ws_allow_urls()),
            kiri_core::limits::Limits::default(),
        )),
        Surface::Menu => router.with_menu(kiri_core::app_menu::MenuService::new(
            std::sync::Arc::new(kiri_core::app_menu::DisabledMenu),
            kiri_core::app_menu::MenuAllowlist::new(vec![]),
            kiri_core::limits::Limits::default(),
        )),
    }
}

/// Host-allowlist for kiri.http.get. Default-deny: only these hosts may be
/// fetched even when the HTTP capability is granted. Expanded per-app config
/// in a later task; for now this is the seed allowlist that proves the
/// exceed-Tauri security axis (Tauri's http plugin has no host allowlist).
fn http_allow_hosts() -> Vec<String> {
    vec!["api.example.com".to_string(), "127.0.0.1".to_string(), "localhost".to_string()]
}

/// Host allowlist for `kiri.shell.run` (audit item 4, G-4). Default-deny:
/// only the exact program + arg prefix below may spawn. Inverts Tauri's shell
/// plugin trust model: arbitrary execution is refused unless explicitly listed.
/// Host glob allowlist for `kiri.fs.*` (audit item 6, fs glob scope). Same model
/// as the cross backend: only paths matching a pattern relative to the fs root may
/// be touched, so a granted FS capability is narrowed to safe shapes.
fn fs_glob_patterns() -> Vec<String> {
    vec!["data/**".to_string(), "config/*.json".to_string(), "*.log".to_string()]
}

fn shell_allow_commands() -> Vec<kiri_core::shell::AllowedCommand> {
    vec![kiri_core::shell::AllowedCommand {
        program: "echo".to_string(),
        args: vec!["kiri-probe".to_string()],
    }]
}

/// Host allowlist of sidecar binary names (audit item 15, G-6). Only these
/// exact names may be spawned by the frontend; argv is forced to the
/// host-declared prefix. Never exposes a path to JavaScript.
fn sidecar_allow() -> Vec<kiri_core::sidecar::AllowedSidecar> {
    vec![kiri_core::sidecar::AllowedSidecar {
        name: "kiri-helper".to_string(),
        args: vec!["--mode".to_string(), "fast".to_string()],
    }]
}

/// Host allowlist of event channel names (audit item 16). Only these exact
/// channel names may be published/subscribed by the frontend; the host owns the
/// channel namespace. Never lets the frontend forge or snoop cross-module
/// events. Inverts Tauri's unrestricted event module.
fn event_channels() -> Vec<kiri_core::event::AllowedChannel> {
    vec![
        kiri_core::event::AllowedChannel { name: "ping".to_string() },
        kiri_core::event::AllowedChannel { name: "update".to_string() },
        kiri_core::event::AllowedChannel { name: "diag".to_string() },
    ]
}

/// Host key allowlist for `kiri.config.get` (audit item 17). Default-deny: only
/// the exact key paths below may be read by the frontend. Inverts Tauri's
/// getConfig trust model: a granted CONFIG capability still cannot read arbitrary
/// host config; only pre-approved key paths may be read.
/// Host-pinned Ed25519 public key for the signed-update verifier (audit-18).
/// NEVER sourced from the frontend: a malicious or phished page cannot substitute
/// a key and accept an attacker-signed release. The matching secret signs release
/// assets at build time. Rotate only via a new pinned build.
const HOST_PINNED_UPDATE_PUBLIC_KEY: &str =
    "333d58ae1e42ba2025b035666528d36430e0c14e13f3d5006c7f0fe22a9d3af6";

fn config_keys() -> Vec<kiri_core::config::AllowedConfigKey> {
    vec![
        kiri_core::config::AllowedConfigKey { key: "app.name".to_string() },
        kiri_core::config::AllowedConfigKey { key: "app.version".to_string() },
        kiri_core::config::AllowedConfigKey { key: "window.theme".to_string() },
    ]
}

/// Host template allowlist for `kiri.notification.show` (audit item 5, G-4b).
/// Default-deny: only the exact template ids below may display, and the frontend
/// may only supply bounded positional args. The host owns the title/body text.
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

fn opener_url_schemes() -> Vec<kiri_core::opener::AllowedUrlScheme> {
    vec![
        kiri_core::opener::AllowedUrlScheme { scheme: "https".to_string() },
        kiri_core::opener::AllowedUrlScheme { scheme: "http".to_string() },
        kiri_core::opener::AllowedUrlScheme { scheme: "mailto".to_string() },
    ]
}

fn opener_file_extensions() -> Vec<kiri_core::opener::AllowedFileExtension> {
    vec![
        kiri_core::opener::AllowedFileExtension { extension: "pdf".to_string() },
        kiri_core::opener::AllowedFileExtension { extension: "txt".to_string() },
        kiri_core::opener::AllowedFileExtension { extension: "md".to_string() },
    ]
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
