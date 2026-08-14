//! Kiri Windows runtime: direct Win32 + WebView2 host.
//!
//! The real implementation is gated behind `cfg(target_os = "windows")`.
//! On other hosts this crate provides a stub that reports the platform
//! contract explicitly, so the workspace builds and tests everywhere while
//! the Windows code is validated by the windows-latest CI job and on
//! Windows hardware.

#[cfg(target_os = "windows")]
mod host;
#[cfg(target_os = "windows")]
mod markers;
#[cfg(target_os = "windows")]
mod startup;

#[cfg(not(target_os = "windows"))]
mod stub {
    /// Returns an explicit description of why this crate is a stub on the
    /// current host. This crate is Windows-only by design (Windows-first MVP,
    /// docs/15-roadmap.md Phase 0-6).
    pub fn platform_contract() -> String {
        format!(
            "kiri-runtime-windows stub on {}: Win32 + WebView2 code is \
             gated behind cfg(target_os=\"windows\"). Build with \
             --target x86_64-pc-windows-msvc or on a Windows host.",
            std::env::consts::OS
        )
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub::platform_contract;

#[cfg(target_os = "windows")]
pub use host::{HostOptions, WindowsHost};

#[cfg(target_os = "windows")]
pub use startup::{host_options_from_args, require_smoke_markers, run_session};

/// Convert a null-terminated wide string to `String` (copy, does not free).
#[cfg(target_os = "windows")]
pub(crate) fn pwstr_to_string(ptr: windows::core::PWSTR) -> String {
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

/// Version of the WebView2 COM bindings this crate was verified against.
pub const WEBVIEW2_COM_BINDINGS: &str = "0.39.1";
