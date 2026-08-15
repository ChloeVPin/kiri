//! Host-side `WindowStateBackend` implementations that bridge the core
//! `kiri.window.state.*` command surface to the real OS-persisted geometry.
//!
//! The backend is the ONLY place that persists window geometry, and it is confined
//! to a host-owned store (the same `HostStore` used by `kiri.store.*`), so the core
//! has already enforced the `WINDOW_STATE` capability bit before any save/load
//! reaches here. The frontend can never choose the namespace, the key, or another
//! window's state, and it can never read the raw persisted blob. That inverts
//! Tauri's window-state plugin, which persists to a frontend-readable/writable JSON
//! without a second capability gate.
//!
//! The cross/win backends record geometry in a host-owned map in this headless build;
//! a real backend would persist to the host-owned path behind the same
//! `WindowStateBackend` trait. The cfg split keeps each target compiling only its own
//! dependency set, mirroring the other controllers.

use std::sync::{Arc, Mutex};

use kiri_core::error::Result;
use kiri_core::window_state::{Geometry, WindowStateBackend};

/// Host-owned geometry store. The core namespace is the authority on where geometry
/// lives, but the store itself is host-owned and never addressed directly by JS.
#[derive(Debug, Default)]
pub struct HostWindowState {
    geometry: Mutex<Option<Geometry>>,
}

impl HostWindowState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl WindowStateBackend for HostWindowState {
    fn save(&self, geometry: &Geometry) -> Result<()> {
        *self.geometry.lock().unwrap() = Some(geometry.clone());
        Ok(())
    }

    fn load(&self) -> Result<Option<Geometry>> {
        Ok(self.geometry.lock().unwrap().clone())
    }
}

#[cfg(not(target_os = "windows"))]
pub mod cross_window_state {
    use super::*;

    pub struct CrossWindowStateBackend {
        inner: Arc<HostWindowState>,
    }

    impl Default for CrossWindowStateBackend {
        fn default() -> Self {
            Self { inner: HostWindowState::new() }
        }
    }

    impl CrossWindowStateBackend {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostWindowState>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostWindowState> {
            self.inner.clone()
        }
    }

    impl WindowStateBackend for CrossWindowStateBackend {
        fn save(&self, geometry: &Geometry) -> Result<()> {
            self.inner.save(geometry)
        }

        fn load(&self) -> Result<Option<Geometry>> {
            self.inner.load()
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_window_state {
    use super::*;

    pub struct WinWindowStateBackend {
        inner: Arc<HostWindowState>,
    }

    impl Default for WinWindowStateBackend {
        fn default() -> Self {
            Self { inner: HostWindowState::new() }
        }
    }

    impl WinWindowStateBackend {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostWindowState>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostWindowState> {
            self.inner.clone()
        }
    }

    impl WindowStateBackend for WinWindowStateBackend {
        fn save(&self, geometry: &Geometry) -> Result<()> {
            self.inner.save(geometry)
        }

        fn load(&self) -> Result<Option<Geometry>> {
            self.inner.load()
        }
    }
}
