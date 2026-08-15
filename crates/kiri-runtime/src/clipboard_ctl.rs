//! Host-side ClipboardController implementations that bridge the core
//! kiri.clipboard.* command surface to the real OS clipboard.
//!
//! The controller is the ONLY place that touches the OS clipboard API, so
//! JavaScript can never reach it directly: every change flows through the
//! capability-gated core handler -> this controller -> OS API. State is
//! mirrored in core's ClipboardState (updated here) so the control plane stays
//! authoritative without re-querying the OS.
//!
//! Every backend uses arboard, which gives one cross-platform clipboard API
//! for macOS, Linux, and Windows. The cfg split is kept for symmetry with the
//! other controllers and so each target compiles only its own dependency set.

#[cfg(not(target_os = "windows"))]
mod arboard_clip {
    use std::sync::Arc;

    use kiri_core::clipboard::{ClipboardController, ClipboardState};
    use kiri_core::error::{Error, Result};

    /// Bridges kiri.clipboard.* to arboard (macOS/Linux cross backend).
    pub struct CrossClipboardController {
        cell: Arc<std::sync::Mutex<arboard::Clipboard>>,
    }

    impl CrossClipboardController {
        pub fn new() -> Result<Self> {
            let cb = arboard::Clipboard::new()
                .map_err(|_e| Error::command_error("clipboard init failed"))?;
            Ok(Self { cell: Arc::new(std::sync::Mutex::new(cb)) })
        }
    }

    impl ClipboardController for CrossClipboardController {
        fn read(&self, _state: &mut ClipboardState) -> Result<String> {
            self.cell
                .lock()
                .unwrap()
                .get_text()
                .map_err(|_e| Error::command_error("clipboard read failed"))
        }
        fn write(&self, state: &mut ClipboardState, text: &str) {
            if self.cell.lock().unwrap().set_text(text.to_string()).is_ok() {
                state.last_written = text.to_string();
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use arboard_clip::CrossClipboardController;

#[cfg(target_os = "windows")]
mod win_clip {
    use std::sync::Arc;

    use kiri_core::clipboard::{ClipboardController, ClipboardState};
    use kiri_core::error::{Error, Result};

    /// Bridges kiri.clipboard.* to arboard on Windows (direct host backend).
    pub struct WinClipboardController {
        cell: Arc<std::sync::Mutex<arboard::Clipboard>>,
    }

    impl WinClipboardController {
        pub fn new() -> Result<Self> {
            let cb = arboard::Clipboard::new()
                .map_err(|_e| Error::command_error("clipboard init failed"))?;
            Ok(Self { cell: Arc::new(std::sync::Mutex::new(cb)) })
        }
    }

    impl ClipboardController for WinClipboardController {
        fn read(&self, _state: &mut ClipboardState) -> Result<String> {
            self.cell
                .lock()
                .unwrap()
                .get_text()
                .map_err(|_e| Error::command_error("clipboard read failed"))
        }
        fn write(&self, state: &mut ClipboardState, text: &str) {
            if self.cell.lock().unwrap().set_text(text.to_string()).is_ok() {
                state.last_written = text.to_string();
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_clip::WinClipboardController;
