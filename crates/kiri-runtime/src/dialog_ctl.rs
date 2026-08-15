//! Host-side `DialogRunner` implementations that bridge the core
//! `kiri.dialog.open` command surface to the real OS native dialog.
//!
//! The runner is the ONLY place that actually opens a native dialog, and it only
//! ever receives a host-owned, allowlisted title (the core `DialogService` has
//! already enforced the DIALOG capability bit AND the host allowlist of dialog
//! kinds). JavaScript can never open a free-form native prompt: every dialog has
//! passed the host allowlist before this runs. That inverts Tauri's dialog
//! plugin, which lets the frontend open arbitrary native dialogs once the
//! capability is present (a spoofing/social-engineering surface).
//!
//! The cross backend shells out to the platform dialog tool (osascript on macOS,
//! zenity on Linux). The Windows backend uses a PowerShell call. The cfg split
//! keeps each target compiling only its own dependency set, mirroring the other
//! controllers.

use kiri_core::dialog::{DialogKind, DialogResult, DialogRunner};
use kiri_core::error::{Error, Result};

fn run_dialog(kind: DialogKind, title: &str) -> Result<DialogResult> {
    let (bin, args) = notifier_for(&kind, title);
    let status = std::process::Command::new(bin)
        .args(args)
        .status()
        .map_err(|e| Error::command_error(format!("kiri.dialog: spawn dialog failed: {e}")))?;
    // A dismissed/cancelled dialog is not fatal; report not-confirmed.
    Ok(DialogResult {
        kind: format!("{kind:?}").to_lowercase(),
        confirmed: status.success(),
        paths: vec![],
    })
}

#[cfg(target_os = "macos")]
fn notifier_for(kind: &DialogKind, title: &str) -> (&'static str, Vec<String>) {
    let script = match kind {
        &DialogKind::Confirm => format!(
            "display dialog \"{}\" buttons {{\"OK\", \"Cancel\"}}",
            title.replace('"', "\\\"")
        ),
        _ => format!("display dialog \"{}\"", title.replace('"', "\\\"")),
    };
    ("osascript", vec!["-e".to_string(), script])
}

#[cfg(all(not(target_os = "windows"), target_os = "linux"))]
fn notifier_for(kind: &DialogKind, title: &str) -> (&'static str, Vec<String>) {
    match kind {
        DialogKind::Confirm => {
            ("zenity", vec!["--question".to_string(), "--text".to_string(), title.to_string()])
        }
        _ => ("zenity", vec!["--info".to_string(), "--text".to_string(), title.to_string()]),
    }
}

#[cfg(target_os = "windows")]
fn notifier_for(kind: &DialogKind, title: &str) -> (&'static str, Vec<String>) {
    let script = match kind {
        &DialogKind::Confirm => format!("Add-Type -AssemblyName System.Windows.Forms; $r = [System.Windows.Forms.MessageBox]::Show('{0}','Kiri', 'OKCancel'); exit $(if ($r -eq 'OK') {{0}} else {{1}})", title.replace('\'', "''")),
        _ => format!("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{0}','Kiri')", title.replace('\'', "''")),
    };
    ("powershell", vec!["-NoProfile".to_string(), "-Command".to_string(), script])
}

#[cfg(not(target_os = "windows"))]
pub mod cross_dialog {
    use super::*;

    pub struct CrossDialogRunner;

    impl Default for CrossDialogRunner {
        fn default() -> Self {
            Self
        }
    }

    impl CrossDialogRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl DialogRunner for CrossDialogRunner {
        fn show(&self, kind: DialogKind, title: &str) -> Result<DialogResult> {
            run_dialog(kind, title)
        }
    }

    pub use CrossDialogRunner as PlatformDialogRunner;
}

#[cfg(target_os = "windows")]
pub mod win_dialog {
    use super::*;

    pub struct WinDialogRunner;

    impl Default for WinDialogRunner {
        fn default() -> Self {
            Self
        }
    }

    impl WinDialogRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl DialogRunner for WinDialogRunner {
        fn show(&self, kind: DialogKind, title: &str) -> Result<DialogResult> {
            run_dialog(kind, title)
        }
    }

    pub use WinDialogRunner as PlatformDialogRunner;
}

#[cfg(not(target_os = "windows"))]
pub use cross_dialog::CrossDialogRunner;
#[cfg(target_os = "windows")]
pub use win_dialog::WinDialogRunner;
