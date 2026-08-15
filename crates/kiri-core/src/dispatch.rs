//! Control-plane dispatch (T003): orchestrates validation, authorization,
//! decode, execution, and tracing for one request, then builds the wire
//! response. This is the layer the platform transports call after they have
//! identified the native caller.
//!
//! The dispatch order follows specs/IPC.md exactly: outer type -> version ->
//! command id -> payload length -> authorize. Application command code runs
//! only after validation and authorization succeed. Trace events are emitted
//! for the mandated stages so the latency benchmark can attribute time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::caller::CallerId;
use crate::capabilities::CapabilityBits;
use crate::error::{Error, Result};
use crate::header::ControlHeader;
use crate::limits::Limits;
use crate::resources::ResourceTable;
use crate::trace::{Stage, TraceEvent, TraceSink};
use crate::validate;
use crate::wire::{WireRequest, WireResponse};

/// Command IDs reserved by the runtime control plane.
pub mod command_id {
    /// Echo/pong command used for liveness and latency probing.
    pub const PING: u32 = 1;
    /// Diagnostics snapshot command (T010 developer panel).
    pub const DIAGNOSTICS: u32 = 2;
    /// Open a resource owned by the caller (T011 developer panel honesty).
    pub const RESOURCES_OPEN: u32 = 3;
    /// Close a previously opened resource (T011 developer panel honesty).
    pub const RESOURCES_CLOSE: u32 = 4;
    /// Report the host platform/OS (R-3 JS surface parity with Tauri).
    pub const PLATFORM_OS: u32 = 5;
    /// Report the host CPU architecture (R-3).
    pub const PLATFORM_ARCH: u32 = 6;
    /// Report the runtime/app version (R-3).
    pub const APP_VERSION: u32 = 7;
    /// Emit a named event to listeners (R-3 event bus).
    pub const EVENT_EMIT: u32 = 8;
    /// Register a listener for a named event (R-3 event bus).
    pub const EVENT_LISTEN: u32 = 9;
    /// Read a scoped file (R-5: kiri.fs.read).
    pub const FS_READ: u32 = 10;
    /// Write a scoped file (R-5: kiri.fs.write).
    pub const FS_WRITE: u32 = 11;
    /// Check a scoped path exists (R-5: kiri.fs.exists).
    pub const FS_EXISTS: u32 = 12;
    /// Remove a scoped file (R-5: kiri.fs.remove).
    pub const FS_REMOVE: u32 = 13;

    /// Get the window title (G-5: kiri.window.title.get).
    pub const WINDOW_TITLE_GET: u32 = 14;
    /// Set the window title (G-5: kiri.window.title.set).
    pub const WINDOW_TITLE_SET: u32 = 15;
    /// Show the window (G-5: kiri.window.show).
    pub const WINDOW_SHOW: u32 = 16;
    /// Hide the window (G-5: kiri.window.hide).
    pub const WINDOW_HIDE: u32 = 17;
    /// Minimize the window (G-5: kiri.window.minimize).
    pub const WINDOW_MINIMIZE: u32 = 18;
    /// Maximize the window (G-5: kiri.window.maximize).
    pub const WINDOW_MAXIMIZE: u32 = 19;
    /// Restore the window from minimized/maximized (G-5: kiri.window.restore).
    pub const WINDOW_RESTORE: u32 = 20;
    /// Request the window to close (G-5: kiri.window.close).
    pub const WINDOW_CLOSE: u32 = 21;
    /// Focus the window (G-5: kiri.window.focus).
    pub const WINDOW_FOCUS: u32 = 22;

    /// Read the system clipboard as text (G-6: kiri.clipboard.read).
    pub const CLIPBOARD_READ: u32 = 23;
    /// Write text to the system clipboard (G-6: kiri.clipboard.write).
    pub const CLIPBOARD_WRITE: u32 = 24;

    /// Directory portion of a path (audit item 2: kiri.path.dirname).
    pub const PATH_DIRNAME: u32 = 25;
    /// Final path component (kiri.path.basename).
    pub const PATH_BASENAME: u32 = 26;
    /// Extension of the final component (kiri.path.extname).
    pub const PATH_EXTNAME: u32 = 27;
    /// Final component without extension (kiri.path.stem).
    pub const PATH_STEM: u32 = 28;
    /// Join base + segments (kiri.path.join).
    pub const PATH_JOIN: u32 = 29;
    /// Whether a path is absolute (kiri.path.isAbsolute).
    pub const PATH_IS_ABSOLUTE: u32 = 30;
    /// Host home directory (kiri.os.homedir).
    pub const OS_HOME_DIR: u32 = 31;
    /// Host temp directory (kiri.os.tempdir).
    pub const OS_TEMP_DIR: u32 = 32;
    /// App config directory (kiri.os.appConfigDir).
    pub const OS_APP_CONFIG_DIR: u32 = 33;
    /// App data directory (kiri.os.appDataDir).
    pub const OS_APP_DATA_DIR: u32 = 34;
    /// App cache directory (kiri.os.appCacheDir).
    pub const OS_APP_CACHE_DIR: u32 = 35;
    /// User documents directory (kiri.os.documentDir).
    pub const OS_DOCUMENT_DIR: u32 = 36;
    /// Application directory (kiri.os.appDir).
    pub const OS_APP_DIR: u32 = 37;
    /// Capability-scoped HTTP GET (kiri.http.get, audit item 3). Allows a
    /// host-allowlisted fetch only; exceeds Tauri's unrestricted http plugin.
    pub const HTTP_GET: u32 = 38;
    /// Capability-scoped, host-allowlisted command execution
    /// (kiri.shell.run, audit item 4, G-4 JS surface parity with Tauri's shell
    /// plugin). Exceeds Tauri's shell plugin on the security axis: a granted
    /// capability still cannot spawn an unapproved program; the host allowlist
    /// is the second gate.
    pub const SHELL_RUN: u32 = 39;
    /// Capability-scoped, host-template-allowlisted notification
    /// (kiri.notification.show, audit item 5). Exceeds Tauri's notification
    /// plugin on the security axis: the frontend may only trigger pre-approved
    /// templates with bounded args; it cannot render free-form title/body.
    pub const NOTIFY: u32 = 40;
    /// Capability-scoped, host-allowlisted native dialog (kiri.dialog.open, audit
    /// item 7). Exceeds Tauri's dialog plugin on the security axis: the frontend
    /// may only open pre-approved dialog kinds with a host-owned title; it cannot
    /// fabricate free-form native prompts.
    pub const DIALOG_OPEN: u32 = 41;
}

/// Capability bits used by built-in control commands.
pub mod capability_bit {
    /// Authorizes the `ping` liveness probe. Bit 0.
    pub const PING: u32 = 0;
    /// Authorizes reading the runtime diagnostics snapshot. Bit 1.
    pub const DIAGNOSTICS: u32 = 1;
    /// Authorizes opening/closing caller-owned resources. Bit 2.
    pub const RESOURCES: u32 = 2;
    /// Authorizes reading host platform/OS facts. Bit 3 (R-3).
    pub const PLATFORM: u32 = 3;
    /// Authorizes reading the runtime/app version. Bit 4 (R-3).
    pub const APP: u32 = 4;
    /// Authorizes emitting/listening to named events. Bit 5 (R-3).
    pub const EVENT: u32 = 5;

    /// Authorizes scoped filesystem access (R-5: kiri.fs.*). Bit 6.
    pub const FS: u32 = 6;

    /// Authorizes window control (kiri.window.*). Bit 7 (G-5 JS surface).
    pub const WINDOW: u32 = 7;

    /// Authorizes clipboard read/write (kiri.clipboard.*). Bit 8 (G-6 JS
    /// surface; exceeds Tauri's unrestricted clipboard plugin on the security
    /// axis: capability authority + audit instead of a blanket grant).
    pub const CLIPBOARD: u32 = 8;

    /// Authorizes path/os helper queries (kiri.path.*, kiri.os.*). Bit 9 (audit
    /// item 2; exceeds Tauri's path/os plugins on the security axis: each access
    /// is capability-gated instead of granted by default).
    pub const PATH: u32 = 9;
    /// Authorizes capability-scoped, host-allowlisted HTTP fetches
    /// (kiri.http.get). Bit 10 (audit item 3; exceeds Tauri's http plugin on the
    /// security axis: arbitrary fetch is denied unless the host is on the
    /// allowlist).
    pub const HTTP: u32 = 10;
    /// Authorizes restricted, host-allowlisted command execution
    /// (kiri.shell.run). Bit 11 (audit item 4, G-4 JS surface). Exceeds Tauri's
    /// shell plugin on the security axis: a granted capability still cannot spawn
    /// an unapproved program; the host allowlist is the second gate, so a
    /// compromised or careless frontend cannot run an arbitrary binary.
    pub const SHELL: u32 = 11;
    /// Authorizes restricted, host-template-allowlisted notifications
    /// (kiri.notification.show). Bit 12 (audit item 5). Exceeds Tauri's
    /// notification plugin on the security axis: a granted capability still
    /// cannot render arbitrary title/body; only host-declared templates with
    /// bounded args may show, so a malicious frontend cannot spoof a system
    /// notification.
    pub const NOTIFICATION: u32 = 12;
    /// Authorizes restricted, host-allowlisted native dialogs (kiri.dialog.open).
    /// Bit 13 (audit item 7). Exceeds Tauri's dialog plugin on the security axis:
    /// a granted capability still cannot open an arbitrary native prompt; only
    /// host-approved dialog kinds with a host-owned title may show, so a malicious
    /// frontend cannot spoof a system dialog.
    pub const DIALOG: u32 = 13;

    /// Map a command id to the capability bit it requires. Keeps plugin command
    /// registration in lockstep with the inline `Router::with_*` definitions so
    /// a command can only be registered with the authority it is supposed to
    /// enforce. Unknown ids map to `PING` (harmless liveness-only authority).
    pub fn for_command(id: u32) -> u32 {
        match id {
            crate::dispatch::command_id::PING => PING,
            crate::dispatch::command_id::DIAGNOSTICS => DIAGNOSTICS,
            crate::dispatch::command_id::RESOURCES_OPEN
            | crate::dispatch::command_id::RESOURCES_CLOSE => RESOURCES,
            crate::dispatch::command_id::PLATFORM_OS
            | crate::dispatch::command_id::PLATFORM_ARCH => PLATFORM,
            crate::dispatch::command_id::APP_VERSION => APP,
            crate::dispatch::command_id::EVENT_EMIT | crate::dispatch::command_id::EVENT_LISTEN => {
                EVENT
            }
            crate::dispatch::command_id::FS_READ
            | crate::dispatch::command_id::FS_WRITE
            | crate::dispatch::command_id::FS_EXISTS
            | crate::dispatch::command_id::FS_REMOVE => FS,
            crate::dispatch::command_id::WINDOW_TITLE_GET
            | crate::dispatch::command_id::WINDOW_TITLE_SET
            | crate::dispatch::command_id::WINDOW_SHOW
            | crate::dispatch::command_id::WINDOW_HIDE
            | crate::dispatch::command_id::WINDOW_MINIMIZE
            | crate::dispatch::command_id::WINDOW_MAXIMIZE
            | crate::dispatch::command_id::WINDOW_RESTORE
            | crate::dispatch::command_id::WINDOW_CLOSE
            | crate::dispatch::command_id::WINDOW_FOCUS => WINDOW,
            crate::dispatch::command_id::CLIPBOARD_READ
            | crate::dispatch::command_id::CLIPBOARD_WRITE => CLIPBOARD,
            crate::dispatch::command_id::PATH_DIRNAME
            | crate::dispatch::command_id::PATH_BASENAME
            | crate::dispatch::command_id::PATH_EXTNAME
            | crate::dispatch::command_id::PATH_STEM
            | crate::dispatch::command_id::PATH_JOIN
            | crate::dispatch::command_id::PATH_IS_ABSOLUTE
            | crate::dispatch::command_id::OS_HOME_DIR
            | crate::dispatch::command_id::OS_TEMP_DIR
            | crate::dispatch::command_id::OS_APP_CONFIG_DIR
            | crate::dispatch::command_id::OS_APP_DATA_DIR
            | crate::dispatch::command_id::OS_APP_CACHE_DIR
            | crate::dispatch::command_id::OS_DOCUMENT_DIR
            | crate::dispatch::command_id::OS_APP_DIR => PATH,
            crate::dispatch::command_id::HTTP_GET => HTTP,
            crate::dispatch::command_id::SHELL_RUN => SHELL,
            crate::dispatch::command_id::NOTIFY => NOTIFICATION,
            crate::dispatch::command_id::DIALOG_OPEN => DIALOG,
            _ => PING,
        }
    }
}

/// A command handler. Receives the authoritative caller, the request id, and
/// the already-decoded JSON payload. Returns the response payload or an error.
pub type Handler = Arc<dyn Fn(CallerId, u64, &Value) -> Result<Value> + Send + Sync>;

/// Registered command: its required capability and handler.
#[derive(Clone)]
struct Command {
    required: CapabilityBits,
    handler: Handler,
}

/// The control-plane router: maps command IDs to commands and runs the
/// mandated validation + trace pipeline for each request.
///
/// `Router` is cheaply cloneable (handlers are shared via `Arc`); the runtime
/// clones it into each per-connection dispatch context.
#[derive(Clone)]
pub struct Router {
    commands: HashMap<u32, Command>,
    limits: Limits,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        let mut router = Router { commands: HashMap::new(), limits: Limits::default() };
        router.register_ping();
        router
    }

    /// Create a router with no commands registered. Used by the plugin host so
    /// built-in commands come exclusively from loaded plugins (R-2), proving the
    /// registration path instead of relying on inline defaults.
    pub fn new_empty() -> Self {
        Router { commands: HashMap::new(), limits: Limits::default() }
    }

    /// Build an empty router with an explicit limit set (tests + tuning).
    pub fn new_with_limits(limits: Limits) -> Self {
        Router { commands: HashMap::new(), limits }
    }

    /// Attach a shared diagnostics sink and register the `kiri.diag` command.
    /// The command returns the privacy-safe snapshot; it requires the
    /// `DIAGNOSTICS` capability, enforced by the validation pipeline.
    pub fn with_diagnostics(mut self, diagnostics: crate::diagnostics::Diagnostics) -> Self {
        let diag = diagnostics.clone();
        let mut required = CapabilityBits::empty();
        required.set(capability_bit::DIAGNOSTICS);
        self.register(
            command_id::DIAGNOSTICS,
            required,
            Arc::new(move |_caller, _request_id, _payload| {
                let snap = diag.snapshot(
                    env!("CARGO_PKG_VERSION"),
                    if cfg!(target_os = "windows") { "windows" } else { "cross" },
                );
                serde_json::to_value(&snap)
                    .map_err(|e| Error::internal_error(format!("diagnostics snapshot encode: {e}")))
            }),
        );
        self
    }

    /// Attach a shared generational resource table and register `kiri.open`
    /// (id 3) / `kiri.close` (id 4). Both require the `RESOURCES` capability.
    /// `kiri.open` inserts one caller-owned resource and returns its packed
    /// numeric id; `kiri.close` removes it (owner + generation validated by
    /// the table). The live open count is written into `diagnostics` after
    /// every mutation so the developer panel shows an honest, dynamic number
    /// (T011: replaces the previously hardcoded baseline of 1).
    pub fn with_resources(
        mut self,
        diagnostics: crate::diagnostics::Diagnostics,
        caller: CallerId,
    ) -> Self {
        let table: Arc<Mutex<ResourceTable<()>>> = Arc::new(Mutex::new(ResourceTable::new()));
        let open_table = table.clone();
        let open_diag = diagnostics.clone();
        let open_caller = caller;
        let mut open_required = CapabilityBits::empty();
        open_required.set(capability_bit::RESOURCES);
        self.register(
            command_id::RESOURCES_OPEN,
            open_required,
            Arc::new(move |_c, _rid, _payload| {
                let mut t = open_table.lock().unwrap();
                let id = t
                    .insert(open_caller, (), 4096)
                    .map_err(|e| Error::limit_exceeded(e.to_string()))?;
                open_diag.set_open_resources(t.len() as u32);
                Ok(serde_json::json!({ "resource_id": id.into_raw() }))
            }),
        );

        let close_table = table.clone();
        let close_diag = diagnostics.clone();
        let close_caller = caller;
        let mut close_required = CapabilityBits::empty();
        close_required.set(capability_bit::RESOURCES);
        self.register(
            command_id::RESOURCES_CLOSE,
            close_required,
            Arc::new(move |_c, _rid, payload| {
                let raw = payload.get("resource_id").and_then(|v| v.as_u64()).ok_or_else(|| {
                    Error::protocol_error("kiri.close requires numeric resource_id")
                })?;
                let id = crate::resources::ResourceId::from_raw(raw);
                let mut t = close_table.lock().unwrap();
                t.remove(close_caller, id)?;
                close_diag.set_open_resources(t.len() as u32);
                Ok(serde_json::json!({ "closed": true }))
            }),
        );
        self
    }

    /// Attach the R-3 JS-surface command set: `kiri.platform.os`,
    /// `kiri.platform.arch`, `kiri.app.version`, `kiri.event.emit`, and
    /// `kiri.event.listen`. Each is capability-gated; the handlers read real
    /// host facts and never touch the filesystem or network. The event bus is
    /// an in-process broadcast so the trusted frontend and native tooling share
    /// one channel (parity with Tauri's `event` module, R-3).
    pub fn with_platform(mut self, events: crate::platform::EventBus) -> Self {
        let mut os_required = CapabilityBits::empty();
        os_required.set(capability_bit::PLATFORM);
        self.register(
            command_id::PLATFORM_OS,
            os_required,
            Arc::new(|_c, _rid, _payload| {
                Ok(serde_json::json!({ "os": crate::platform::host_os() }))
            }),
        );

        let mut arch_required = CapabilityBits::empty();
        arch_required.set(capability_bit::PLATFORM);
        self.register(
            command_id::PLATFORM_ARCH,
            arch_required,
            Arc::new(|_c, _rid, _payload| {
                Ok(serde_json::json!({ "arch": crate::platform::host_arch() }))
            }),
        );

        let mut app_required = CapabilityBits::empty();
        app_required.set(capability_bit::APP);
        self.register(
            command_id::APP_VERSION,
            app_required,
            Arc::new(|_c, _rid, _payload| {
                Ok(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
            }),
        );

        let emit_bus = events.clone();
        let mut emit_required = CapabilityBits::empty();
        emit_required.set(capability_bit::EVENT);
        self.register(
            command_id::EVENT_EMIT,
            emit_required,
            Arc::new(move |_c, _rid, payload| {
                let name = payload.get("event").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::protocol_error("kiri.event.emit requires string event")
                })?;
                let data = payload.get("payload").cloned().unwrap_or(serde_json::Value::Null);
                emit_bus.publish(name, data);
                Ok(serde_json::json!({ "emitted": true }))
            }),
        );

        let listen_bus = events.clone();
        let mut listen_required = CapabilityBits::empty();
        listen_required.set(capability_bit::EVENT);
        self.register(
            command_id::EVENT_LISTEN,
            listen_required,
            Arc::new(move |_c, _rid, payload| {
                let name = payload.get("event").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::protocol_error("kiri.event.listen requires string event")
                })?;
                let id = listen_bus.subscribe(name);
                Ok(serde_json::json!({ "listener_id": id }))
            }),
        );
        self
    }

    /// Override the default limits (used by tests and tuning).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Register the four `kiri.fs.*` commands bound to a scoped `FsService`.
    /// The scope and limits come from the host; JavaScript cannot expand them.
    pub fn with_fs(self, scope: crate::capabilities::PathScope, limits: Limits) -> Self {
        self.with_fs_service(crate::fs::FsService::new(scope, limits))
    }

    /// Register `kiri.fs.*` from an already-constructed `FsService`. Used by
    /// the plugin path so the same authority applies whether the surface is
    /// wired inline or through the plugin host.
    pub fn with_fs_service(mut self, service: crate::fs::FsService) -> Self {
        for (id, required, handler) in crate::fs::fs_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the `kiri.window.*` command set (G-5 JS surface parity with
    /// Tauri's `window` module). Every handler is capability-gated (bit
    /// `WINDOW`) and operates on a host-owned `WindowController` + shared
    /// `WindowState` mirror, so the native window handle is never reachable
    /// from JavaScript. The host supplies the real controller; tests use a
    /// stub and assert routing/authorization/state without a WebView.
    pub fn with_window(
        mut self,
        controller: std::sync::Arc<dyn crate::window::WindowController>,
        state: std::sync::Arc<std::sync::Mutex<crate::window::WindowState>>,
    ) -> Self {
        for (id, required, handler) in crate::window::window_handlers(controller, state) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register a command with its required capability and handler.
    pub fn register(&mut self, id: u32, required: CapabilityBits, handler: Handler) {
        self.commands.insert(id, Command { required, handler });
    }
    /// Register the kiri.clipboard.* command set (G-6 JS surface parity with
    /// Tauri's clipboard plugin). Every handler is capability-gated (bit
    /// CLIPBOARD) and operates on a host-owned ClipboardController + shared
    /// ClipboardState mirror, so the OS clipboard is never reachable from
    /// JavaScript. The host supplies the real controller; tests use a stub and
    /// assert routing/authorization/state without the system clipboard.
    pub fn with_clipboard(
        mut self,
        controller: std::sync::Arc<dyn crate::clipboard::ClipboardController>,
        state: std::sync::Arc<std::sync::Mutex<crate::clipboard::ClipboardState>>,
    ) -> Self {
        for (id, required, handler) in crate::clipboard::clipboard_handlers(controller, state) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the kiri.path.* and kiri.os.* command set (audit item 2, G-7
    /// JS surface parity with Tauri's path/os plugins). Every handler is
    /// capability-gated (bit PATH) and resolves OS directory facts through
    /// PathState, so JavaScript cannot reach env vars or filesystem-root
    /// queries except via the explicitly granted helpers. The surface is pure
    /// path math plus read-only directory discovery, fully exercisable
    /// headlessly with no WebView and no real filesystem mutation.
    pub fn with_path(mut self, service: crate::path::PathService) -> Self {
        for (id, required, handler) in crate::path::path_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the kiri.http.* command set (audit item 3, G-3 JS surface parity
    /// with Tauri's http plugin). Every request is capability-gated (bit HTTP) and
    /// constrained to a host allowlist supplied by the native host, so a granted
    /// capability still cannot fetch an unapproved origin. Responses are bounded by
    /// the shared bulk-object ceiling, matching kiri.fs backpressure.
    pub fn with_http(mut self, service: crate::http::HttpService) -> Self {
        for (id, required, handler) in crate::http::http_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the kiri.shell.* command set (audit item 4, G-4 JS surface
    /// parity with Tauri's shell plugin). Every spawn is capability-gated (bit
    /// SHELL) AND constrained to the command allowlist supplied by the native
    /// host, so a granted capability still cannot spawn an unapproved program.
    /// Output is bounded by the shared bulk-object ceiling, matching kiri.fs
    /// backpressure.
    pub fn with_shell(mut self, service: crate::shell::ShellService) -> Self {
        for (id, required, handler) in crate::shell::shell_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the kiri.notification.* command set (audit item 5). Every
    /// notification is capability-gated (bit NOTIFICATION) AND constrained to a
    /// host template allowlist supplied by the native host, so a granted
    /// capability still cannot render arbitrary title/body; only pre-approved
    /// template ids with bounded args may show.
    pub fn with_notification(mut self, service: crate::notification::NotificationService) -> Self {
        for (id, required, handler) in crate::notification::notification_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    /// Register the kiri.dialog.* command set (audit item 7). Every dialog is
    /// capability-gated (bit DIALOG) AND constrained to a host allowlist of dialog
    /// kinds (with a host-owned title), so a granted capability still cannot render
    /// an arbitrary native prompt; only pre-approved dialog kinds may open.
    pub fn with_dialog(mut self, service: crate::dialog::DialogService) -> Self {
        for (id, required, handler) in crate::dialog::dialog_handlers(service) {
            self.register(id, required, handler);
        }
        self
    }

    fn register_ping(&mut self) {
        let mut required = CapabilityBits::empty();
        required.set(capability_bit::PING);
        self.register(
            command_id::PING,
            required,
            Arc::new(|_caller, _request_id, payload| {
                // Echo the payload back so request correlation is observable
                // end to end; the benchmark asserts request_id maps to pong.
                Ok(serde_json::json!({ "pong": true, "echo": payload }))
            }),
        );
    }

    /// Returns true when the command id is registered.
    pub fn is_known(&self, id: u32) -> bool {
        self.commands.contains_key(&id)
    }

    /// Dispatch one parsed wire request from an already-identified caller.
    ///
    /// Emits trace events for receive/authorize/decode/execute/encode/send/
    /// complete and returns the wire response. Malformed input is rejected
    /// with a stable error before any handler runs.
    pub fn dispatch(
        &self,
        caller: CallerId,
        caller_capabilities: &CapabilityBits,
        request: &WireRequest,
        sink: &mut dyn TraceSink,
    ) -> WireResponse {
        let request_id = request.request_id;
        sink.emit(&TraceEvent::new(Stage::Receive).with_request_id(request_id));

        // Reconstruct the logical header for the validation pipeline. The wire
        // envelope carries the same fields the logical protocol requires.
        let header = ControlHeader {
            magic: request.magic,
            version: request.version,
            flags: request.flags,
            command_id: request.command_id,
            request_id,
            payload_len: request.payload_len,
            codec: request.codec,
            reserved: 0,
            resource_count: 0,
        };

        let actual_len = serde_json::to_vec(&request.payload).unwrap_or_default().len() as u32;
        let validated = match validate::validate_request(
            caller,
            &header,
            actual_len,
            caller_capabilities,
            self.command_required(caller, request.command_id),
            &self.limits,
            |id| self.is_known(id),
        ) {
            Ok(v) => v,
            Err(e) => {
                let e = e.with_request_id(request_id);
                sink.emit(
                    &TraceEvent::new(Stage::Complete)
                        .with_request_id(request_id)
                        .with_result_code(e.code.as_str()),
                );
                return WireResponse::err(request_id, e);
            }
        };

        sink.emit(
            &TraceEvent::new(Stage::Authorize)
                .with_request_id(request_id)
                .with_command_id(validated.command_id),
        );
        sink.emit(
            &TraceEvent::new(Stage::Decode)
                .with_request_id(request_id)
                .with_payload_bytes(actual_len as u64),
        );

        let start = crate::trace::MonotonicClock::now_ns();
        let result = match self.commands.get(&validated.command_id) {
            Some(cmd) => (cmd.handler)(caller, request_id, &request.payload),
            None => {
                Err(Error::protocol_error(format!("unknown command id {}", validated.command_id)))
            }
        };
        let elapsed = crate::trace::MonotonicClock::now_ns().saturating_sub(start);

        sink.emit(
            &TraceEvent::new(Stage::Execute)
                .with_request_id(request_id)
                .with_command_id(validated.command_id)
                .with_duration_ns(elapsed),
        );

        let response = match result {
            Ok(payload) => {
                sink.emit(
                    &TraceEvent::new(Stage::Encode).with_request_id(request_id).with_payload_bytes(
                        serde_json::to_vec(&payload).unwrap_or_default().len() as u64,
                    ),
                );
                WireResponse::ok(request_id, payload)
            }
            Err(e) => {
                let e = e.with_request_id(request_id);
                sink.emit(
                    &TraceEvent::new(Stage::Complete)
                        .with_request_id(request_id)
                        .with_result_code(e.code.as_str()),
                );
                WireResponse::err(request_id, e)
            }
        };
        sink.emit(&TraceEvent::new(Stage::Send).with_request_id(request_id));
        sink.emit(&TraceEvent::new(Stage::Complete).with_request_id(request_id));
        response
    }

    fn command_required(&self, _caller: CallerId, id: u32) -> CapabilityBits {
        self.commands.get(&id).map(|c| c.required).unwrap_or_else(CapabilityBits::empty)
    }
}

/// Build a `WireRequest` for the built-in ping command (helper for tests and
/// the runtime bridge).
pub fn ping_request(request_id: u64, payload: Value) -> WireRequest {
    WireRequest::new(command_id::PING, request_id, 1, payload)
}

/// True when a wire response is a successful pong for the given request id.
pub fn is_pong(response: &WireResponse, request_id: u64) -> bool {
    response.request_id == request_id
        && response.error.is_none()
        && matches!(&response.payload, Some(Value::Object(map)) if map.get("pong") == Some(&Value::Bool(true)))
}

/// A static, data-driven router built from the command catalog
/// (`crate::commands::COMMANDS`). Unlike [`Router`], which builds a `HashMap`
/// at runtime, `StaticRouter` resolves a command ID directly from the const
/// catalog, so routing order is deterministic and auditable (T005). It
/// reuses the same validation + trace pipeline as [`Router`].
pub struct StaticRouter {
    limits: Limits,
}

impl Default for StaticRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticRouter {
    pub fn new() -> Self {
        StaticRouter { limits: Limits::default() }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// True when the catalog knows the command id.
    pub fn is_known(&self, id: u32) -> bool {
        crate::commands::command_name(id).is_some()
    }

    /// Dispatch a parsed request using the catalog-defined handler and the
    /// capability required by the command id. The actual execution handler is
    /// the built-in ping for now; T005's codegen path replaces this with
    /// per-command glue, but the routing decision stays catalog-driven.
    pub fn dispatch(
        &self,
        caller: CallerId,
        caller_capabilities: &CapabilityBits,
        request: &WireRequest,
        sink: &mut dyn TraceSink,
    ) -> WireResponse {
        // The catalog is authoritative for routing. If the request carries an
        // id the catalog does not know, reject it before any handler runs
        // (T005 acceptance: unknown IDs rejected). The capability requirement
        // is resolved by `Router::dispatch` through the catalog so the caller
        // is authorized against the real required bit, never self-granted.
        if crate::commands::command_name(request.command_id).is_none() {
            let e = Error::protocol_error(format!("unknown command id {}", request.command_id));
            return WireResponse::err(request.request_id, e);
        }
        // Delegate to the shared pipeline. The caller's granted capabilities
        // are passed through unchanged: the runtime assigns them natively and
        // JavaScript can never widen them. The catalog `required` cap is the
        // authorization requirement checked against the caller by
        // `validate_request` (specs/SECURITY.md step 3) -- it is NOT granted
        // to the caller here.
        Router::new().with_limits(self.limits.clone()).dispatch(
            caller,
            caller_capabilities,
            request,
            sink,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::caller::CallerRegistry;
    use crate::capabilities::CapabilityBits;
    use crate::diagnostics::Diagnostics;
    use crate::error::ErrorCode;
    use crate::latency::LatencyDistribution;
    use crate::trace::RingTraceSink;
    use serde_json::json;

    fn caller_caps() -> CapabilityBits {
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        caps
    }

    #[test]
    fn ping_returns_pong_with_echoed_payload() {
        let router = Router::new();
        let req = ping_request(7, json!({ "hello": "world" }));
        let mut sink = RingTraceSink::new(64);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(is_pong(&resp, 7));
        let payload = resp.payload.unwrap();
        assert_eq!(payload["echo"], json!({ "hello": "world" }));
    }

    #[test]
    fn request_ids_correlate_across_many_requests() {
        let router = Router::new();
        let mut sink = RingTraceSink::new(1024);
        for id in 1u64..=500 {
            let req = ping_request(id, json!({ "n": id }));
            let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
            assert!(is_pong(&resp, id), "request {id} did not correlate");
            assert_eq!(resp.payload.as_ref().unwrap()["echo"]["n"], id);
        }
    }

    #[test]
    fn ten_thousand_ping_loop_completes_with_latency_distribution() {
        let router = Router::new();
        let mut sink = RingTraceSink::new(2048);
        let mut dist = LatencyDistribution::new();
        for id in 1u64..=10_000 {
            let start = crate::trace::MonotonicClock::now_ns();
            let req = ping_request(id, json!({ "i": id }));
            let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
            let elapsed = crate::trace::MonotonicClock::now_ns().saturating_sub(start);
            assert!(is_pong(&resp, id));
            dist.record(elapsed);
        }
        assert_eq!(dist.count(), 10_000);
        let summary = dist.summary();
        assert!(summary.min_ns <= summary.p50_ns);
        assert!(summary.p50_ns <= summary.p99_ns);
        assert!(summary.p99_ns <= summary.max_ns);
        assert!(summary.max_ns > 0, "latency distribution must be emitted");
    }

    #[test]
    fn malformed_magic_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.magic = *b"NOPE";
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn malformed_version_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.version = 999;
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn malformed_payload_length_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.payload_len = req.payload_len + 1; // declared != actual
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn unknown_command_id_rejected() {
        let router = Router::new();
        let mut req = ping_request(1, json!(null));
        req.command_id = 4242;
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::ProtocolError);
    }

    #[test]
    fn missing_capability_denied() {
        let router = Router::new();
        // caller with no capabilities at all
        let empty = CapabilityBits::empty();
        let req = ping_request(1, json!(null));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &empty, &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn invalid_json_payload_for_ping_still_roundtrips() {
        // ping echoes arbitrary JSON; a non-object payload must not panic.
        let router = Router::new();
        let req = ping_request(3, json!("a-string"));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(CallerId(1), &caller_caps(), &req, &mut sink);
        assert!(is_pong(&resp, 3));
        assert_eq!(resp.payload.as_ref().unwrap()["echo"], json!("a-string"));
    }

    #[test]
    fn open_close_mutates_live_resource_count() {
        let mut reg = CallerRegistry::new();
        let caller = reg.register();
        let diag = Diagnostics::new();
        let router =
            Router::new().with_diagnostics(diag.clone()).with_resources(diag.clone(), caller);

        // No resources open yet.
        assert_eq!(diag.snapshot("0.1.0", "cross").open_resources, 0);

        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::RESOURCES);
        let open_payload = json!({});
        let open_req = WireRequest {
            magic: *b"KRI1",
            version: 1,
            flags: 1,
            command_id: command_id::RESOURCES_OPEN,
            request_id: 1,
            payload_len: serde_json::to_vec(&open_payload).unwrap().len() as u32,
            codec: 1,
            payload: open_payload,
        };
        let mut sink = diag.clone();
        let resp = router.dispatch(caller, &caps, &open_req, &mut sink);
        assert!(resp.error.is_none(), "open should succeed: {:?}", resp.error);
        assert_eq!(diag.snapshot("0.1.0", "cross").open_resources, 1);

        let rid =
            resp.payload.as_ref().unwrap().get("resource_id").and_then(|v| v.as_u64()).unwrap();
        let close_payload = json!({ "resource_id": rid });
        let close_req = WireRequest {
            magic: *b"KRI1",
            version: 1,
            flags: 1,
            command_id: command_id::RESOURCES_CLOSE,
            request_id: 2,
            payload_len: serde_json::to_vec(&close_payload).unwrap().len() as u32,
            codec: 1,
            payload: close_payload,
        };
        let resp2 = router.dispatch(caller, &caps, &close_req, &mut sink);
        assert!(resp2.error.is_none(), "close should succeed: {:?}", resp2.error);
        assert_eq!(diag.snapshot("0.1.0", "cross").open_resources, 0);
    }

    #[test]
    fn open_rejected_without_resources_capability() {
        let mut reg = CallerRegistry::new();
        let caller = reg.register();
        let diag = Diagnostics::new();
        let router =
            Router::new().with_diagnostics(diag.clone()).with_resources(diag.clone(), caller);
        // Only PING capability: open must be denied by the validate pipeline.
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        let deny_payload = json!({});
        let req = WireRequest {
            magic: *b"KRI1",
            version: 1,
            flags: 1,
            command_id: command_id::RESOURCES_OPEN,
            request_id: 1,
            payload_len: serde_json::to_vec(&deny_payload).unwrap().len() as u32,
            codec: 1,
            payload: deny_payload,
        };
        let mut sink = diag.clone();
        let resp = router.dispatch(caller, &caps, &req, &mut sink);
        assert!(resp.error.is_some(), "open without capability must be denied");
        assert_eq!(resp.error.as_ref().unwrap().code, ErrorCode::Unauthorized);
    }
    // --- R-3 JS-surface commands (kiri.platform.*, kiri.app.*, kiri.event.*) ---

    fn platform_router() -> (Router, crate::platform::EventBus) {
        let bus = crate::platform::EventBus::new();
        let router = Router::new().with_platform(bus.clone());
        (router, bus)
    }

    fn full_caps() -> CapabilityBits {
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        caps.set(capability_bit::PLATFORM);
        caps.set(capability_bit::APP);
        caps.set(capability_bit::EVENT);
        caps
    }

    #[test]
    fn platform_os_arch_return_real_facts() {
        let (router, _bus) = platform_router();
        let mut sink = RingTraceSink::new(16);

        let os_req = WireRequest::new(command_id::PLATFORM_OS, 1, 1, json!(null));
        let os_resp = router.dispatch(CallerId(1), &full_caps(), &os_req, &mut sink);
        assert!(os_resp.error.is_none(), "os: {:?}", os_resp.error);
        let os = os_resp.payload.as_ref().unwrap()["os"].as_str().unwrap();
        assert!(matches!(os, "macos" | "windows" | "linux" | "unknown"));

        let arch_req = WireRequest::new(command_id::PLATFORM_ARCH, 2, 1, json!(null));
        let arch_resp = router.dispatch(CallerId(1), &full_caps(), &arch_req, &mut sink);
        assert!(arch_resp.error.is_none(), "arch: {:?}", arch_resp.error);
        assert!(arch_resp.payload.as_ref().unwrap()["arch"].is_string());
    }

    #[test]
    fn app_version_reports_package_version() {
        let (router, _bus) = platform_router();
        let mut sink = RingTraceSink::new(16);
        let req = WireRequest::new(command_id::APP_VERSION, 1, 1, json!(null));
        let resp = router.dispatch(CallerId(1), &full_caps(), &req, &mut sink);
        assert!(resp.error.is_none(), "app.version: {:?}", resp.error);
        assert_eq!(
            resp.payload.as_ref().unwrap()["version"].as_str().unwrap(),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn platform_rejected_without_capability() {
        let (router, _bus) = platform_router();
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        let mut sink = RingTraceSink::new(16);
        let req = WireRequest::new(command_id::PLATFORM_OS, 1, 1, json!(null));
        let resp = router.dispatch(CallerId(1), &caps, &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, ErrorCode::Unauthorized);
    }

    #[test]
    fn event_emit_publishes_to_listener() {
        let (router, bus) = platform_router();
        let mut sink = RingTraceSink::new(16);
        let caps = full_caps();

        let listen_req =
            WireRequest::new(command_id::EVENT_LISTEN, 1, 1, json!({ "event": "greeting" }));
        let listen_resp = router.dispatch(CallerId(1), &caps, &listen_req, &mut sink);
        assert!(listen_resp.error.is_none());
        let listener_id = listen_resp.payload.as_ref().unwrap()["listener_id"].as_u64().unwrap();

        let emit_req = WireRequest::new(
            command_id::EVENT_EMIT,
            2,
            1,
            json!({ "event": "greeting", "payload": { "hi": 1 } }),
        );
        let emit_resp = router.dispatch(CallerId(1), &caps, &emit_req, &mut sink);
        assert!(emit_resp.error.is_none());
        assert_eq!(emit_resp.payload.as_ref().unwrap()["emitted"], json!(true));

        let drained = bus.drain(listener_id);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0]["hi"], json!(1));
    }

    #[test]
    fn event_emit_requires_string_event() {
        let (router, _bus) = platform_router();
        let mut sink = RingTraceSink::new(16);
        let req = WireRequest::new(command_id::EVENT_EMIT, 1, 1, json!({ "payload": { "x": 1 } }));
        let resp = router.dispatch(CallerId(1), &full_caps(), &req, &mut sink);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, ErrorCode::ProtocolError);
    }
}
