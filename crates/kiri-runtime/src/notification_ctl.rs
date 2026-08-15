//! Host-side `NotificationRunner` implementations that bridge the core
//! `kiri.notification.show` command surface to the real OS notification.
//!
//! The runner is the ONLY place that actually displays a notification, and it
//! only ever receives host-resolved plain text (title/body already substituted
//! from a host-owned template by the core `NotificationService`). JavaScript can
//! never reach the OS notification API with free-form content: every show has
//! passed the NOTIFICATION capability bit AND the host template allowlist before
//! this runs. That is the inversion of Tauri's notification plugin, which lets
//! the frontend send arbitrary title/body once the capability is present.
//!
//! The cross backend shells out to the platform notifier (osascript on macOS,
//! notify-send on Linux). The Windows backend uses a PowerShell BurntToast
//! call when available and degrades to a no-op otherwise so the host never
//! crashes on a machine without the module. The cfg split keeps each target
//! compiling only its own dependency set, mirroring the other controllers.

use kiri_core::error::{Error, Result};
use kiri_core::notification::NotificationRunner;

/// Display a resolved (host-owned) title/body on the OS.
fn show_native(title: &str, body: &str) -> Result<()> {
    let status = std::process::Command::new(notifier_binary())
        .args(notifier_args(title, body))
        .status()
        .map_err(|e| {
            Error::command_error(format!("kiri.notification: spawn notifier failed: {e}"))
        })?;
    if !status.success() {
        // A declined/failed notification is not a fatal control-plane error; the
        // host may be headless or the user may have denied permission.
        return Err(Error::command_error("kiri.notification: OS notifier returned non-zero"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn notifier_binary() -> &'static str {
    "osascript"
}
#[cfg(target_os = "macos")]
fn notifier_args(title: &str, body: &str) -> Vec<String> {
    vec![
        "-e".to_string(),
        format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        ),
    ]
}

#[cfg(all(not(target_os = "windows"), target_os = "linux"))]
fn notifier_binary() -> &'static str {
    "notify-send"
}
#[cfg(all(not(target_os = "windows"), target_os = "linux"))]
fn notifier_args(title: &str, body: &str) -> Vec<String> {
    vec![title.to_string(), body.to_string()]
}

#[cfg(target_os = "windows")]
fn notifier_binary() -> &'static str {
    "powershell"
}
#[cfg(target_os = "windows")]
fn notifier_args(title: &str, body: &str) -> Vec<String> {
    // BurntToast is the common Windows toast module; this is best-effort and
    // degrades gracefully in the runtime (a non-zero exit is mapped to an error
    // but does not crash the host).
    vec![
        "-NoProfile".to_string(),
        "-Command".to_string(),
        format!(
            "Import-Module BurntToast -ErrorAction Stop; New-BurntToastNotification -Text '{0}','{1}'",
            title.replace('\'', "''"),
            body.replace('\'', "''")
        ),
    ]
}

#[cfg(not(target_os = "windows"))]
pub mod cross_notify {
    use super::*;

    /// Real notification displayer for the macOS/Linux wry/tao backend (audit
    /// item 5, G-4b). The core has already enforced the NOTIFICATION capability
    /// and the host template allowlist before this runs.
    pub struct CrossNotificationRunner;

    impl Default for CrossNotificationRunner {
        fn default() -> Self {
            Self
        }
    }

    impl CrossNotificationRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl NotificationRunner for CrossNotificationRunner {
        fn show(&self, title: &str, body: &str) -> Result<()> {
            show_native(title, body)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_notify {
    use super::*;

    /// Real notification displayer for the Windows direct Win32 + WebView2
    /// backend (audit item 5, G-4b). Same enforcement contract as the cross
    /// backend.
    pub struct WinNotificationRunner;

    impl Default for WinNotificationRunner {
        fn default() -> Self {
            Self
        }
    }

    impl WinNotificationRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl NotificationRunner for WinNotificationRunner {
        fn show(&self, title: &str, body: &str) -> Result<()> {
            show_native(title, body)
        }
    }
}
