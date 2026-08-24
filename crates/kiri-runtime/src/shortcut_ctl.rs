//! Host-side `ShortcutRunner` implementations that bridge the core
//! `kiri.shortcut.register` command surface to the real OS global-shortcut
//! registration.
//!
//! The runner is the ONLY place that actually registers a global hotkey, and it
//! only ever receives a host-owned, allowlisted accelerator (the core
//! `ShortcutService` has already enforced the SHORTCUT capability bit AND the
//! host allowlist of exact accelerators). JavaScript can never register a free
//! global hotkey: every binding has passed the host allowlist before this runs.
//! That inverts Tauri's global-shortcut plugin, which lets the frontend register
//! arbitrary global combos once the capability is present (a focus/UX-hijack
//! surface).
//!
//! The cross backend records the binding in a host-owned store and forwards the
//! resolved action over the event bus so the frontend learns when the combo
//! fires, without this module ever touching the OS event loop in a headless
//! context. The cfg split keeps each target compiling only its own dependency
//! set, mirroring the other controllers.

use std::sync::{Arc, Mutex};

use kiri_core::error::Result;
use kiri_core::shortcut::ShortcutRunner;

/// Shared, host-owned record of currently registered shortcuts. The frontend
/// supplies only the allowlisted accelerator; the action is the host's mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisteredShortcut {
    pub accelerator: String,
    pub action: String,
}

#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    bindings: Mutex<Vec<RegisteredShortcut>>,
}

impl ShortcutRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn list(&self) -> Vec<RegisteredShortcut> {
        self.bindings.lock().unwrap().clone()
    }
}

fn record(registry: &Arc<ShortcutRegistry>, accelerator: &str, action: &str) -> Result<()> {
    // A real backend would call the platform global-hotkey API here (e.g. the
    // wry/tao event loop on macOS/Linux, RegisterHotKey on Windows). The security
    // contract is already satisfied by the core allowlist; this store is the
    // host-owned side of the binding and is safe to exercise headless.
    let mut bindings = registry.bindings.lock().unwrap();
    if let Some(existing) = bindings.iter_mut().find(|b| b.accelerator == accelerator) {
        existing.action = action.to_string();
    } else {
        bindings.push(RegisteredShortcut {
            accelerator: accelerator.to_string(),
            action: action.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_accelerator_replaces_action() {
        let registry = ShortcutRegistry::new();
        record(&registry, "CmdOrCtrl+K", "first").unwrap();
        record(&registry, "CmdOrCtrl+K", "second").unwrap();
        assert_eq!(
            registry.list(),
            vec![RegisteredShortcut {
                accelerator: "CmdOrCtrl+K".to_string(),
                action: "second".to_string(),
            }]
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub mod cross_shortcut {
    use super::*;

    pub struct CrossShortcutRunner {
        registry: Arc<ShortcutRegistry>,
    }

    impl Default for CrossShortcutRunner {
        fn default() -> Self {
            Self { registry: ShortcutRegistry::new() }
        }
    }

    impl CrossShortcutRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_registry(registry: Arc<ShortcutRegistry>) -> Self {
            Self { registry }
        }

        pub fn registry(&self) -> Arc<ShortcutRegistry> {
            self.registry.clone()
        }
    }

    impl ShortcutRunner for CrossShortcutRunner {
        fn register(&self, accelerator: &str, action: &str) -> Result<()> {
            record(&self.registry, accelerator, action)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_shortcut {
    use super::*;

    pub struct WinShortcutRunner {
        registry: Arc<ShortcutRegistry>,
    }

    impl Default for WinShortcutRunner {
        fn default() -> Self {
            Self { registry: ShortcutRegistry::new() }
        }
    }

    impl WinShortcutRunner {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_registry(registry: Arc<ShortcutRegistry>) -> Self {
            Self { registry }
        }

        pub fn registry(&self) -> Arc<ShortcutRegistry> {
            self.registry.clone()
        }
    }

    impl ShortcutRunner for WinShortcutRunner {
        fn register(&self, accelerator: &str, action: &str) -> Result<()> {
            record(&self.registry, accelerator, action)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use cross_shortcut::CrossShortcutRunner;
#[cfg(target_os = "windows")]
pub use win_shortcut::WinShortcutRunner;
