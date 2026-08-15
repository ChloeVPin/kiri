//! Host-side `ShellRunner` implementations that bridge the core
//! `kiri.shell.run` command surface to the real OS process spawner.
//!
//! The runner is the ONLY place that actually spawns a process. JavaScript can
//! never reach `std::process::Command` directly: every spawn flows through the
//! capability-gated core handler -> this runner -> OS API, and the core has
//! already enforced the `SHELL` capability bit AND the host allowlist. That is
//! the inversion of Tauri's shell plugin: Tauri grants arbitrary execution once
//! the capability is present; Kiri refuses every command that is not an
//! explicit allowlist entry, so a compromised or careless frontend cannot run
//! an unapproved binary.
//!
//! Both backends use `std::process::Command` (identical on macOS/Linux/Windows);
//! the cfg split is kept for symmetry with the other controllers and so each
//! target compiles only its own dependency set.

use kiri_core::error::{Error, Result};
use kiri_core::shell::{ShellOutput, ShellRunner};

/// Spawn a program with args, capturing stdout/stderr and the exit code.
fn run_captured(program: &str, args: &[String]) -> Result<ShellOutput> {
    let output = std::process::Command::new(program).args(args).output().map_err(|e| {
        Error::command_error(format!("kiri.shell.run: failed to spawn {program}: {e}"))
    })?;
    Ok(ShellOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(not(target_os = "windows"))]
pub mod cross_shell {
    use super::*;

    /// Real process spawner for the macOS/Linux wry/tao backend (audit item 4,
    /// G-4). The core has already enforced the SHELL capability and the host
    /// allowlist before this runs.
    pub struct CrossShellRunner;

    impl Default for CrossShellRunner {
        fn default() -> Self {
            Self
        }
    }

    impl CrossShellRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl ShellRunner for CrossShellRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<ShellOutput> {
            run_captured(program, args)
        }
    }
}

#[cfg(target_os = "windows")]
pub mod win_shell {
    use super::*;

    /// Real process spawner for the Windows direct Win32 + WebView2 backend
    /// (audit item 4, G-4). Same enforcement contract as the cross backend.
    pub struct WinShellRunner;

    impl Default for WinShellRunner {
        fn default() -> Self {
            Self
        }
    }

    impl WinShellRunner {
        pub fn new() -> Self {
            Self
        }
    }

    impl ShellRunner for WinShellRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<ShellOutput> {
            run_captured(program, args)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use cross_shell::CrossShellRunner;
#[cfg(target_os = "windows")]
pub use win_shell::WinShellRunner;
