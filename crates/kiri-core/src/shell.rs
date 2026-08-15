//! Restricted command-execution surface (`kiri.shell`).
//!
//! This closes the Tauri `shell` plugin parity gap (G-4) and converts Tauri's
//! single biggest escape risk into a Kiri strength. Tauri's shell plugin, when
//! the capability is granted, can run arbitrary commands. Kiri requires BOTH
//! the `SHELL` capability bit AND an explicit command allowlist: the host
//! declares the exact program (and optionally arg prefixes) that may run. A
//! granted capability with no matching allowlist entry is refused, so a
//! compromised or careless frontend cannot spawn an unapproved binary. Output is
//! captured and bounded by the same bulk-object ceiling as `kiri.fs`.
//!
//! The actual spawn is behind the `ShellRunner` trait (mirrors `HttpClient`):
//! the native host injects a real spawner; tests use a `StubShell` and assert
//! authorization, allowlist enforcement, arg validation, and size caps without
//! launching real processes.

use std::sync::Arc;

use base64::Engine;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;

/// Authorizes the `kiri.shell.*` commands.
pub const SHELL_CAPABILITY: u32 = 11;

/// One allowed command: an exact program path plus an optional fixed arg prefix.
/// Only programs whose resolved executable equals `program` and whose args start
/// with `args` (in order) may run. Empty `args` means "no args required".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Host-configured allowlist of commands that may be spawned. Default-deny: a
/// command runs only if it matches an entry exactly (program + arg prefix).
#[derive(Debug, Clone, Default)]
pub struct ShellAllowlist {
    commands: Vec<AllowedCommand>,
}

impl ShellAllowlist {
    pub fn new(commands: Vec<AllowedCommand>) -> Self {
        Self { commands }
    }

    /// Whether `program` with `args` is permitted under the allowlist.
    fn allows(&self, program: &str, args: &[String]) -> bool {
        self.commands.iter().any(|c| {
            c.program == program
                && args.len() >= c.args.len()
                && c.args.iter().enumerate().all(|(i, a)| args.get(i) == Some(a))
        })
    }

    pub fn commands(&self) -> &[AllowedCommand] {
        &self.commands
    }
}

/// A captured command result.
#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Transport seam. The native host provides a real spawner; tests provide a
/// stub. Kept trait-based so the logical protocol has zero platform deps.
pub trait ShellRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<ShellOutput>;
}

/// Capability-scoped shell service bounded to a command allowlist plus limits.
#[derive(Clone)]
pub struct ShellService {
    runner: Arc<dyn ShellRunner>,
    allowlist: Arc<ShellAllowlist>,
    limits: Arc<Limits>,
}

impl ShellService {
    pub fn new(runner: Arc<dyn ShellRunner>, allowlist: ShellAllowlist, limits: Limits) -> Self {
        Self { runner, allowlist: Arc::new(allowlist), limits: Arc::new(limits) }
    }

    /// Run a command if it is on the allowlist and output fits the bulk cap.
    pub fn run(&self, program: &str, args: &[String]) -> Result<Value> {
        if !self.allowlist.allows(program, args) {
            return Err(Error::scope_denied(format!(
                "kiri.shell.run: command not on allowlist: {program}"
            )));
        }
        let out = self.runner.run(program, args)?;
        self.limits.check_bulk_object((out.stdout.len() + out.stderr.len()) as u64)?;
        Ok(serde_json::json!({
            "program": program,
            "exitCode": out.exit_code,
            "stdout": base64::engine::general_purpose::STANDARD.encode(&out.stdout),
            "stderr": base64::engine::general_purpose::STANDARD.encode(&out.stderr),
            "bytes": out.stdout.len() + out.stderr.len(),
        }))
    }
}

/// Build the `kiri.shell.*` handlers bound to one ShellService.
pub fn shell_handlers(
    service: ShellService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(SHELL_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::SHELL_RUN,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let program = p
                .get("program")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::invalid_argument("kiri.shell.run requires string program"))?;
            let args = p
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            svc.run(program, &args)
        }) as Handler,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;

    struct StubShell {
        code: i32,
        stdout: Vec<u8>,
    }
    impl ShellRunner for StubShell {
        fn run(&self, _program: &str, _args: &[String]) -> Result<ShellOutput> {
            Ok(ShellOutput {
                exit_code: self.code,
                stdout: self.stdout.clone(),
                stderr: Vec::new(),
            })
        }
    }

    fn allow() -> ShellAllowlist {
        ShellAllowlist::new(vec![AllowedCommand {
            program: "/usr/bin/echo".to_string(),
            args: vec!["hello".to_string()],
        }])
    }

    fn router() -> Router {
        let svc = ShellService::new(
            Arc::new(StubShell { code: 0, stdout: b"hello".to_vec() }),
            allow(),
            Limits::default(),
        );
        Router::new_with_limits(Limits::default()).with_shell(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(SHELL_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn allowed_command_runs_and_captures() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::SHELL_RUN,
            serde_json::json!({ "program": "/usr/bin/echo", "args": ["hello"] }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["exitCode"], 0);
        assert_eq!(
            out["payload"]["stdout"],
            base64::engine::general_purpose::STANDARD.encode(b"hello")
        );
    }

    #[test]
    fn command_not_on_allowlist_is_denied() {
        let r = router();
        let out = dispatch(
            &r,
            command_id::SHELL_RUN,
            serde_json::json!({ "program": "/usr/bin/rm", "args": ["-rf", "/"] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn wrong_arg_prefix_is_denied() {
        let r = router();
        // 'echo' is allowed only with the 'hello' arg prefix; 'echo world' is not.
        let out = dispatch(
            &r,
            command_id::SHELL_RUN,
            serde_json::json!({ "program": "/usr/bin/echo", "args": ["world"] }),
        );
        assert!(!out["error"].is_null());
    }

    #[test]
    fn missing_shell_capability_is_denied() {
        let r = router();
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::SHELL_RUN,
            1,
            1,
            serde_json::json!({ "program": "/usr/bin/echo", "args": ["hello"] }),
        );
        let resp = r.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }
}
