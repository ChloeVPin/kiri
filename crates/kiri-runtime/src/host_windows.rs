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
use kiri_core::capabilities::PathScope;
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
/// Run a script in the WebView and ignore its result. `webview2-com`
/// `ExecuteScript` needs a wide-string script and a completion handler.
fn exec_script(webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, js: &str) {
    let wide: Vec<u16> = js.encode_utf16().collect();
    let handler = webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
        |_result: windows::core::Result<()>, _output: String| Ok(()),
    ));
    let _ =
        unsafe { webview.ExecuteScript(windows::core::PCWSTR::from_raw(wide.as_ptr()), &handler) };
}

/// The Windows native host: window, WebView2 lifecycle, and message loop.
pub struct WindowsHost;

/// Virtual host used for the local application origin (WebView2 virtual host
/// mapping; gives the page a proper https origin instead of `file://`).
pub(crate) const VIRTUAL_HOST_NAME: &str = "app.local";
pub(crate) const FRONTEND_PAGE: &str = "index.html";
pub(crate) const APP_ORIGIN: &str = "https://app.local";

/// True for URLs served from the application origin (used to reject markers
/// from `about:blank` or any other origin).
fn is_app_origin_url(url: &str) -> bool {
    url == APP_ORIGIN || url.starts_with(&format!("{APP_ORIGIN}/"))
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
    pub caller: CallerId,
    pub caller_caps: CapabilityBits,
    pub router: Router,
    pub diagnostics: Diagnostics,
    pub resources: std::sync::Arc<std::sync::Mutex<ResourceTable<()>>>,
    pub env: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    pub controller: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    pub webview: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    pub webview_token: i64,
    pub navigation_token: i64,
    /// Set once the first animation frame arrived in smoke mode; the smoke
    /// timer then posts WM_QUIT after `exit_after_ready_ms`.
    pub smoke_armed: bool,
    pub exit_code: i32,
}

impl HostRuntime {
    /// Reply to an unknown web message with a Kiri protocol error. This is
    /// the bridge stub: the real control plane (T003) replaces it.
    fn reply_protocol_error(&self, request_id: Option<u64>, command: Option<&str>) {
        let error = KiriError::protocol_error("command not implemented by this host slice")
            .with_command(command.unwrap_or("").to_string());
        let envelope = WireResponse::err(request_id.unwrap_or(0), error);
        if let Ok(json) = serde_json::to_string(&envelope) {
            let wide: Vec<u16> = json.encode_utf16().collect();
            let _ = unsafe { self.webview.PostWebMessageAsJson(PCWSTR::from_raw(wide.as_ptr())) };
        }
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

unsafe fn run_host_inner(options: &HostOptions) -> Result<StartupMarkers, String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
    };
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler,
        CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler,
        WebMessageReceivedEventHandler,
    };

    let mut markers = StartupMarkers::new();
    markers.record(Marker::ProcessSpawnRequested, qpc_now_ns());
    markers.record(Marker::NativeEntry, qpc_now_ns());

    // Control-plane router for the native bridge (T003). Caller identity is
    // assigned by the native runtime, never by JavaScript; grant the ping
    // capability so control commands run from the trusted frontend.
    let mut registry = CallerRegistry::new();
    let caller = registry.register();
    let mut caller_caps = CapabilityBits::empty();
    caller_caps.set(kiri_core::dispatch::capability_bit::PING);
    caller_caps.set(kiri_core::dispatch::capability_bit::DIAGNOSTICS);
    caller_caps.set(kiri_core::dispatch::capability_bit::RESOURCES);
    let diagnostics = Diagnostics::new();
    let events = EventBus::new();
    // Shared generational resource table owned by the host. The resources plugin
    // binds this exact instance via the ABI context, so kiri.open/kiri.close
    // mutate it and keep the diagnostics open-resource count honest and dynamic.
    let resources: std::sync::Arc<std::sync::Mutex<ResourceTable<()>>> =
        std::sync::Arc::new(std::sync::Mutex::new(ResourceTable::<()>::new()));

    // Host-owned fs scope: a bounded sandbox under the temp dir. The host is
    // the only authority that can widen it; the frontend cannot.
    let mut fs_scope = PathScope::new(std::env::temp_dir().join("kiri-fs"));
    fs_scope.read = true;
    fs_scope.write = true;

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

    // Control-plane router (built after the window so it can own the native
    // window handle). Caller identity is assigned by the native runtime, never
    // by JavaScript. G-5: kiri.window.* surface backed by the real HWND.
    let router =
        crate::plugins::PluginHost::build_router_with_plugins(&diagnostics, &resources, caller)
            // R-3: JS-surface commands (kiri.platform.*, kiri.app.*, kiri.event.*).
            .with_platform(events)
            .with_fs(fs_scope, kiri_core::limits::Limits::default())
            .with_window(
                std::sync::Arc::new(crate::window_ctl::WinWindowController::new(hwnd)),
                std::sync::Arc::new(std::sync::Mutex::new(kiri_core::window::WindowState::new(
                    &options.title,
                ))),
            );

    // ---- WebView2 environment (W1: WebView2 shell) ----
    markers.record(Marker::WebViewCreationRequested, qpc_now_ns());
    let env = {
        use std::{cell::RefCell, rc::Rc};
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment;
        let env_cell: Rc<RefCell<Option<ICoreWebView2Environment>>> = Rc::new(RefCell::new(None));
        let done = CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(|handler| {
                unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        PCWSTR::null(),
                        None,
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
          if (window.location.origin !== 'https://app.local') { return; }
          window.kiri = {
            post: function (o) { window.chrome.webview.postMessage(o); },
            send: function (req) { window.chrome.webview.postMessage({ type: 'cmd', request: req }); }
          };
          window.addEventListener('DOMContentLoaded', function () {
            window.kiri.post({ type: 'ready', phase: 'dom' });
          });
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

    // ---- virtual host mapping for the local application origin ----
    let webview3 = webview
        .cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3>()
        .map_err(|e| format!("cast to ICoreWebView2_3: {e}"))?;
    let host_name = windows::core::HSTRING::from(VIRTUAL_HOST_NAME);
    let frontend_dir = options.frontend_dir.as_ref().ok_or("frontend_dir is required")?;
    let frontend_dir = absolute_lexical(frontend_dir);
    if !frontend_dir.is_dir() {
        return Err(format!("frontend directory does not exist: {}", frontend_dir.display()));
    }
    let folder = windows::core::HSTRING::from(frontend_dir.as_os_str());
    webview3
        .SetVirtualHostNameToFolderMapping(
            PCWSTR::from_raw(host_name.as_ptr()),
            PCWSTR::from_raw(folder.as_ptr()),
            COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
        )
        .map_err(|e| format!("SetVirtualHostNameToFolderMapping: {e}"))?;

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
        caller,
        caller_caps,
        router,
        diagnostics,
        resources,
        env,
        controller,
        webview,
        webview_token,
        navigation_token,
        smoke_armed: false,
        exit_code: 0,
    });
    if let Some(version) = browser_version {
        eprintln!("[kiri] WebView2 runtime version: {version}");
    }
    // Watchdog armed for smoke runs so CI cannot hang.
    if runtime.options.smoke {
        let _ =
            unsafe { SetTimer(Some(hwnd), WATCHDOG_TIMER_ID, runtime.options.watchdog_ms, None) };
    }
    // The runtime pointer is owned by this function; it must not be read
    // back from the window after WM_CLOSE destroyed it.
    let runtime_ptr = Box::into_raw(runtime);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, runtime_ptr as isize);

    // ---- navigate to the local application origin ----
    let url = format!("https://{VIRTUAL_HOST_NAME}/{FRONTEND_PAGE}");
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

    // Control-plane command: dispatch through kiri-core and post the
    // wire response back to the page (T003).
    if let Some(req_val) = value.get("request") {
        match serde_json::from_value::<WireRequest>(req_val.clone()) {
            Ok(request) => {
                let mut sink = rt.diagnostics.clone();
                let response = rt.router.dispatch(rt.caller, &rt.caller_caps, &request, &mut sink);
                rt.diagnostics.set_open_resources(rt.resources.lock().unwrap().len() as u32);
                let js = format!(
                    "window.kiri && window.kiri.onResponse && window.kiri.onResponse({});",
                    serde_json::to_string(&response).unwrap_or_default()
                );
                exec_script(&rt.webview, &js);
            }
            Err(_) => {
                let err =
                    WireResponse::err(0, KiriError::protocol_error("malformed command request"));
                let js = format!(
                    "window.kiri && window.kiri.onResponse && window.kiri.onResponse({});",
                    serde_json::to_string(&err).unwrap_or_default()
                );
                exec_script(&rt.webview, &js);
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
                if rt.options.smoke && !rt.smoke_armed {
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
