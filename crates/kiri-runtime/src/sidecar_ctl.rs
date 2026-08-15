//! Host-side `SidecarRunner` implementations that bridge the core
//! `kiri.sidecar.*` command surface to real OS process spawning.
//!
//! The runner is the ONLY place that actually spawns a sidecar. JavaScript can
//! never reach `std::process::Command` directly: every spawn flows through the
//! capability-gated core handler -> this runner -> OS API, and the core has
//! already enforced the `SIDECAR` capability bit AND the host allowlist (exact
//! binary name, fixed argv prefix). That inverts Tauri's sidecar API, which
//! launches an arbitrary companion executable the frontend names once the
//! capability is present; Kiri refuses every sidecar that is not an explicit
//! allowlist entry, so a compromised or careless frontend cannot fork an
//! unapproved binary or smuggle argv.
//!
//! The cross/win backends spawn through `std::process::Command` (identical on
//! macOS/Linux/Windows); the cfg split is kept for symmetry with the other
//! controllers and so each target compiles only its own dependency set.

use std::process::Command;

use kiri_core::error::{Error, Result};
use kiri_core::sidecar::{SidecarOutput, SidecarRunner};

/// Spawn a host-resolved sidecar by name with the host-declared argv prefix,
/// capturing stdout/stderr and the exit code.
fn run_captured(name: &str, args: &[String]) -> Result<SidecarOutput> {
    let output = Command::new(name)
        .args(args)
        .output()
        .map_err(|e| Error::command_error(format!("kiri.sidecar: failed to spawn {name}: {e}")))?;
    Ok(SidecarOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(not(target_os = "windows"))]
pub mod cross_sidecar {
    use super::*;

    /// Real sidecar spawner for the macOS/Linux wry/tao backend (audit item 15,
    /// G-6). The core has already enforced the SIDECAR capability and the host
    /// allowlist before this runs.
    pub struct CrossSidecarRunner;

    impl Default for CrossSidecarRunner {
        fn default() -> Self {
            Self
        }
    }

    impl CrossSidecarRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl SidecarRunner for CrossSidecarRunner {
        fn spawn(&self, name: &str, _path: &str, args: &[String]) -> Result<SidecarOutput> {
            run_captured(name, args)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_sidecar {
    use super::*;

    /// Real sidecar spawner for the Windows direct Win32 + WebView2 backend
    /// (audit item 15, G-6). Same enforcement contract as the cross backend.
    pub struct WinSidecarRunner;

    impl Default for WinSidecarRunner {
        fn default() -> Self {
            Self
        }
    }

    impl WinSidecarRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl SidecarRunner for WinSidecarRunner {
        fn spawn(&self, name: &str, _path: &str, args: &[String]) -> Result<SidecarOutput> {
            run_captured(name, args)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use cross_sidecar::CrossSidecarRunner;
#[cfg(target_os = "windows")]
pub use win_sidecar::WinSidecarRunner;
