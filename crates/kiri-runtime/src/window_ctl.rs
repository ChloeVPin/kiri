//! Host-side `WindowController` implementations that bridge the core
//! `kiri.window.*` command surface to the real native window.
//!
//! The controller is the ONLY place that touches the native window handle, so
//! JavaScript can never reach it directly: every change flows through the
//! capability-gated core handler -> this controller -> native API. State is
//! mirrored in core's `WindowState` (updated here) so the control plane stays
//! authoritative without re-querying the OS.
//!
//! The cross backend (macOS/Linux) uses tao's `Window`; the Windows direct
//! backend uses a Win32 `HWND`. Each controller is compiled only for its
//! target so the correct native dependency is in scope.

#[cfg(not(target_os = "windows"))]
mod tao_ctl {
    use std::sync::Arc;

    use kiri_core::window::{WindowController, WindowState};

    /// Bridges `kiri.window.*` to a `tao::window::Window` (macOS/Linux cross
    /// backend). tao's `Window` is `Send + Sync` on both macOS and Linux, so
    /// an `Arc` keeps the controller `Send + Sync` (the `WindowController`
    /// trait requires it because the router is captured by the WebView IPC
    /// closure).
    pub struct TaoWindowController {
        window: Arc<tao::window::Window>,
    }

    impl TaoWindowController {
        pub fn new(window: Arc<tao::window::Window>) -> Self {
            Self { window }
        }
    }

    impl WindowController for TaoWindowController {
        fn set_title(&self, state: &mut WindowState, title: &str) {
            self.window.set_title(title);
            state.title = title.to_string();
        }
        fn show(&self, state: &mut WindowState) {
            self.window.set_visible(true);
            state.visible = true;
            state.minimized = false;
        }
        fn hide(&self, state: &mut WindowState) {
            self.window.set_visible(false);
            state.visible = false;
        }
        fn minimize(&self, state: &mut WindowState) {
            self.window.set_minimized(true);
            state.minimized = true;
            state.visible = true;
        }
        fn maximize(&self, state: &mut WindowState) {
            self.window.set_maximized(true);
            state.maximized = true;
        }
        fn restore(&self, state: &mut WindowState) {
            self.window.set_minimized(false);
            self.window.set_maximized(false);
            state.minimized = false;
            state.maximized = false;
        }
        fn close(&self, state: &mut WindowState) {
            // The native event loop owns teardown; we only record the request
            // and hide the window so the control plane reflects the intent.
            // The host event loop reads `close_requested` to exit.
            self.window.set_visible(false);
            state.close_requested = true;
            state.visible = false;
        }
        fn focus(&self, state: &mut WindowState) {
            self.window.set_focus();
            state.focused = true;
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use tao_ctl::TaoWindowController;

#[cfg(target_os = "windows")]
mod win {
    use kiri_core::window::{WindowController, WindowState};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, SetWindowTextW, ShowWindow, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE,
        SW_RESTORE, SW_SHOW,
    };

    /// `HWND` is a raw pointer and is neither `Send` nor `Sync` in the
    /// `windows` crate. The controller is only ever touched from the Windows
    /// UI thread (the WebView2 IPC handler runs there), so it is sound to
    /// declare it `Send + Sync` for storage behind `Arc<dyn WindowController>`.
    #[derive(Clone, Copy)]
    struct UiHwnd(HWND);
    unsafe impl Send for UiHwnd {}
    unsafe impl Sync for UiHwnd {}

    /// Bridges `kiri.window.*` to a Win32 `HWND` (WebView2 host). The native
    /// window handle is the ONLY state this controller touches; observable
    /// state is mirrored into core's `WindowState` so the control plane stays
    /// authoritative without re-querying the OS.
    pub struct WinWindowController {
        hwnd: UiHwnd,
    }

    impl WinWindowController {
        pub fn new(hwnd: HWND) -> Self {
            Self { hwnd: UiHwnd(hwnd) }
        }
    }

    impl WindowController for WinWindowController {
        fn set_title(&self, state: &mut WindowState, title: &str) {
            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                let _ = SetWindowTextW(self.hwnd.0, windows::core::PCWSTR(wide.as_ptr()));
            }
            state.title = title.to_string();
        }
        fn show(&self, state: &mut WindowState) {
            unsafe {
                let _ = ShowWindow(self.hwnd.0, SW_SHOW);
            }
            state.visible = true;
            state.minimized = false;
        }
        fn hide(&self, state: &mut WindowState) {
            unsafe {
                let _ = ShowWindow(self.hwnd.0, SW_HIDE);
            }
            state.visible = false;
        }
        fn minimize(&self, state: &mut WindowState) {
            unsafe {
                let _ = ShowWindow(self.hwnd.0, SW_MINIMIZE);
            }
            state.minimized = true;
            state.visible = true;
        }
        fn maximize(&self, state: &mut WindowState) {
            unsafe {
                let _ = ShowWindow(self.hwnd.0, SW_MAXIMIZE);
            }
            state.maximized = true;
        }
        fn restore(&self, state: &mut WindowState) {
            unsafe {
                let _ = ShowWindow(self.hwnd.0, SW_RESTORE);
            }
            state.minimized = false;
            state.maximized = false;
        }
        fn close(&self, state: &mut WindowState) {
            // The message loop owns teardown; mirror the intent and hide so the
            // control plane reflects it. The host loop handles WM_CLOSE to exit.
            state.close_requested = true;
            state.visible = false;
        }
        fn focus(&self, state: &mut WindowState) {
            unsafe {
                let _ = SetForegroundWindow(self.hwnd.0);
            }
            state.focused = true;
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::WinWindowController;
