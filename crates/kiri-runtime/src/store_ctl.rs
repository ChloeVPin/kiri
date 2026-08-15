//! Host-side `StoreBackend` implementations that bridge the core `kiri.store.*`
//! command surface to the real on-disk store.
//!
//! The backend is the ONLY place that touches persistence, and it is confined to
//! the host allowlist of namespaces: the core `StoreService` has already enforced
//! the STORE capability bit AND the host namespace allowlist before any read/write
//! reaches here, so the frontend can never address a namespace the host has not
//! approved. That inverts Tauri's store plugin, which lets the frontend read/write
//! the whole store once the capability is present (a cross-feature data-leak
//! surface).
//!
//! The cross/win backends use a host-owned in-memory map in this headless build;
//! a real backend would persist to the host-owned path (file/sqlite) behind the
//! same `StoreBackend` trait. The cfg split keeps each target compiling only its
//! own dependency set, mirroring the other controllers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kiri_core::error::Result;
use kiri_core::store::StoreBackend;
use serde_json::Value;

/// Host-owned store. All keys are namespaced; the core allowlist is the authority
/// on which namespaces are reachable, but the store itself is host-owned and never
/// addressed directly by JavaScript.
#[derive(Debug, Default)]
pub struct HostStore {
    data: Mutex<HashMap<(String, String), Value>>,
}

impl HostStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl StoreBackend for HostStore {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
        Ok(self.data.lock().unwrap().get(&(namespace.to_string(), key.to_string())).cloned())
    }
    fn set(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
        self.data.lock().unwrap().insert((namespace.to_string(), key.to_string()), value);
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub mod cross_store {
    use super::*;

    pub struct CrossStoreBackend {
        inner: Arc<HostStore>,
    }

    impl Default for CrossStoreBackend {
        fn default() -> Self {
            Self { inner: HostStore::new() }
        }
    }

    impl CrossStoreBackend {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn with_store(inner: Arc<HostStore>) -> Self {
            Self { inner }
        }
        pub fn store(&self) -> Arc<HostStore> {
            self.inner.clone()
        }
    }

    impl StoreBackend for CrossStoreBackend {
        fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
            self.inner.get(namespace, key)
        }
        fn set(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
            self.inner.set(namespace, key, value)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_store {
    use super::*;

    pub struct WinStoreBackend {
        inner: Arc<HostStore>,
    }

    impl Default for WinStoreBackend {
        fn default() -> Self {
            Self { inner: HostStore::new() }
        }
    }

    impl WinStoreBackend {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn with_store(inner: Arc<HostStore>) -> Self {
            Self { inner }
        }
        pub fn store(&self) -> Arc<HostStore> {
            self.inner.clone()
        }
    }

    impl StoreBackend for WinStoreBackend {
        fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
            self.inner.get(namespace, key)
        }
        fn set(&self, namespace: &str, key: &str, value: Value) -> Result<()> {
            self.inner.set(namespace, key, value)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use cross_store::CrossStoreBackend;
#[cfg(target_os = "windows")]
pub use win_store::WinStoreBackend;
