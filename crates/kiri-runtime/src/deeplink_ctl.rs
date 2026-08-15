//! Host-side `DeeplinkRunner` implementations that bridge the core
//! `kiri.deeplink.*` command surface to the real OS deep-link registrar.
//!
//! The registrar is the ONLY place that touches the OS scheme registry, and it is
//! confined to the host allowlist of schemes: the core `DeeplinkService` has already
//! enforced the DEEPLINK capability bit AND the host scheme allowlist before any
//! registration reaches here, so the frontend can never bind a scheme the host has
//! not approved. That inverts Tauri's deep-link plugin, which lets the frontend
//! register an arbitrary URI scheme once the capability is present (a
//! scheme-squatting / handler-hijack surface).
//!
//! The cross/win backends record the registered scheme in a host-owned list in this
//! headless build; a real backend would register against the OS registrar (protocol
//! handler on Windows, URL scheme on macOS/Linux) behind the same `DeeplinkRunner`
//! trait. The cfg split keeps each target compiling only its own dependency set,
//! mirroring the other controllers.

use std::sync::{Arc, Mutex};

use kiri_core::deeplink::DeeplinkRunner;
use kiri_core::error::Result;

/// Host-owned record of registered schemes. The core allowlist is the authority on
/// which schemes are reachable, but the registrar itself is host-owned and never
/// addressed directly by JavaScript.
#[derive(Debug, Default)]
pub struct HostDeeplink {
    schemes: Mutex<Vec<String>>,
}

impl HostDeeplink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl DeeplinkRunner for HostDeeplink {
    fn register(&self, scheme: &str) -> Result<()> {
        self.schemes.lock().unwrap().push(scheme.to_string());
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub mod cross_deeplink {
    use super::*;

    pub struct CrossDeeplinkRunner {
        inner: Arc<HostDeeplink>,
    }

    impl Default for CrossDeeplinkRunner {
        fn default() -> Self {
            Self { inner: HostDeeplink::new() }
        }
    }

    impl CrossDeeplinkRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostDeeplink>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostDeeplink> {
            self.inner.clone()
        }
    }

    impl DeeplinkRunner for CrossDeeplinkRunner {
        fn register(&self, scheme: &str) -> Result<()> {
            self.inner.register(scheme)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_deeplink {
    use super::*;

    pub struct WinDeeplinkRunner {
        inner: Arc<HostDeeplink>,
    }

    impl Default for WinDeeplinkRunner {
        fn default() -> Self {
            Self { inner: HostDeeplink::new() }
        }
    }

    impl WinDeeplinkRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_inner(inner: Arc<HostDeeplink>) -> Self {
            Self { inner }
        }

        pub fn inner(&self) -> Arc<HostDeeplink> {
            self.inner.clone()
        }
    }

    impl DeeplinkRunner for WinDeeplinkRunner {
        fn register(&self, scheme: &str) -> Result<()> {
            self.inner.register(scheme)
        }
    }
}
