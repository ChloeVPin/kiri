//! Host-side `AutostartRunner` implementations that bridge the core
//! `kiri.autostart.*` command surface to the real OS launch-at-login mechanism.
//!
//! The runner is the ONLY place that actually registers a login item, and it only
//! ever registers the host's own binary (the target path is host-owned, passed at
//! construction). The core `AutostartService` has already enforced the AUTOSTART
//! capability bit AND the host policy (default-deny) before this runs, so the
//! frontend can never choose which executable persists. That inverts Tauri's
//! autostart plugin, which lets the frontend enable launch-at-login freely once
//! the capability is present (a persistence surface).
//!
//! The cross backend records the binding in a host-owned store; a real backend
//! would call the platform login-item API (launchd on macOS, systemd --user on
//! Linux, the registry Run key on Windows). The cfg split keeps each target
//! compiling only its own dependency set, mirroring the other controllers.

use std::sync::{Arc, Mutex};

use kiri_core::autostart::AutostartRunner;
use kiri_core::error::Result;

/// Shared, host-owned record of the current autostart state. The frontend can
/// only toggle `enabled`; the target binary is the host's own path and is never
/// exposed to or settable by JavaScript.
#[derive(Debug, Clone, Default)]
pub struct AutostartState {
    pub enabled: bool,
    pub target: String,
}

#[derive(Debug, Default)]
pub struct AutostartStore {
    state: Mutex<AutostartState>,
}

impl AutostartStore {
    pub fn new(target: String) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(AutostartState { enabled: false, target }) })
    }

    pub fn snapshot(&self) -> AutostartState {
        self.state.lock().unwrap().clone()
    }
}

fn apply(store: &Arc<AutostartStore>, enabled: bool) -> Result<()> {
    // A real backend would register/remove the host-owned `target` as a login
    // item here. The security contract is already satisfied by the core policy
    // gate; this store is the host-owned side of the binding and is safe to
    // exercise headless.
    store.state.lock().unwrap().enabled = enabled;
    Ok(())
}

fn read(store: &Arc<AutostartStore>) -> Result<bool> {
    Ok(store.state.lock().unwrap().enabled)
}

#[cfg(not(target_os = "windows"))]
pub mod cross_autostart {
    use super::*;

    pub struct CrossAutostartRunner {
        store: Arc<AutostartStore>,
    }

    impl Default for CrossAutostartRunner {
        fn default() -> Self {
            Self { store: AutostartStore::new(String::new()) }
        }
    }

    impl CrossAutostartRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_store(store: Arc<AutostartStore>) -> Self {
            Self { store }
        }

        pub fn store(&self) -> Arc<AutostartStore> {
            self.store.clone()
        }
    }

    impl AutostartRunner for CrossAutostartRunner {
        fn set_enabled(&self, enabled: bool) -> Result<()> {
            apply(&self.store, enabled)
        }
        fn is_enabled(&self) -> Result<bool> {
            read(&self.store)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_autostart {
    use super::*;

    pub struct WinAutostartRunner {
        store: Arc<AutostartStore>,
    }

    impl Default for WinAutostartRunner {
        fn default() -> Self {
            Self { store: AutostartStore::new(String::new()) }
        }
    }

    impl WinAutostartRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_store(store: Arc<AutostartStore>) -> Self {
            Self { store }
        }

        pub fn store(&self) -> Arc<AutostartStore> {
            self.store.clone()
        }
    }

    impl AutostartRunner for WinAutostartRunner {
        fn set_enabled(&self, enabled: bool) -> Result<()> {
            apply(&self.store, enabled)
        }
        fn is_enabled(&self) -> Result<bool> {
            read(&self.store)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use cross_autostart::CrossAutostartRunner;
#[cfg(target_os = "windows")]
pub use win_autostart::WinAutostartRunner;
