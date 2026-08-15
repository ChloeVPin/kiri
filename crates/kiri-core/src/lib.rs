//! Kiri Core: the platform-neutral logical protocol and security authority.
//!
//! This crate implements the logical control-plane contracts defined in the
//! Kiri corpus (`specs/ERRORS.md`, `specs/IPC.md`, `specs/RESOURCES.md`,
//! `specs/SECURITY.md`, `specs/TRACE.md`) as pure, testable Rust.
//!
//! It deliberately contains no platform transport code. Windows, macOS, and
//! Linux backends map these contracts onto their physical WebView transports.

pub mod autostart;
pub mod caller;
pub mod capabilities;
pub mod clipboard;
pub mod commands;
pub mod diagnostics;
pub mod dialog;
pub mod dispatch;
pub mod error;
pub mod fs;
pub mod header;
pub mod http;
pub mod latency;
pub mod limits;
pub mod notification;
pub mod path;
pub mod platform;
pub mod resources;
pub mod security;
pub mod shell;
pub mod shortcut;
pub mod store;
pub mod trace;
pub mod update;
pub mod validate;
pub mod window;
pub mod wire;

pub use autostart::{AutostartAllowlist, AutostartService};
pub use caller::{CallerId, CallerRegistry};
pub use capabilities::{CapabilityBits, CapabilityId, GlobScope, PathScope, Scope};
pub use clipboard::{ClipboardController, ClipboardState};
pub use commands::{
    command_name, emit_typescript, required_capabilities, resolve_command, CommandSpec, COMMANDS,
};
pub use dialog::{DialogAllowlist, DialogKind, DialogService, DialogTemplate};
pub use dispatch::{capability_bit, command_id, is_pong, ping_request, Router, StaticRouter};
pub use error::{Error, ErrorCode, Result};
pub use header::{ControlFlags, ControlHeader, MAGIC, PROTOCOL_VERSION};
pub use http::{HostAllowlist, HttpService};
pub use latency::{LatencyDistribution, LatencySummary};
pub use limits::Limits;
pub use notification::{NotificationAllowlist, NotificationService, NotificationTemplate};
pub use path::{PathService, PathState};
pub use platform::EventBus;
pub use resources::{ResourceId, ResourceTable};
pub use security::{
    is_app_origin, is_navigation_allowed, trusted_frontend_capabilities, CROSS_APP_ORIGIN,
    WINDOWS_APP_ORIGIN,
};
pub use shell::{ShellAllowlist, ShellService};
pub use shortcut::{ShortcutAllowlist, ShortcutBinding, ShortcutService};
pub use store::{StoreAllowlist, StoreNamespace, StoreService};
pub use trace::{MonotonicClock, Stage, TraceEvent, TraceSink};
pub use update::{Ed25519Verifier, PlatformAsset, UpdateManifest, VerifiedAsset, Version};
pub use wire::{WireRequest, WireResponse};

/// Protocol constants shared by all Kiri transports.
pub mod constants {
    /// Default hard ceiling for a single control-plane payload (specs/IPC.md).
    pub const DEFAULT_MAX_CONTROL_PAYLOAD_BYTES: u32 = 1024 * 1024;
    /// Default in-flight requests per WebView.
    pub const DEFAULT_MAX_INFLIGHT_REQUESTS: u32 = 256;
    /// Default outstanding bulk bytes per WebView.
    pub const DEFAULT_MAX_OUTSTANDING_BULK_BYTES: u64 = 256 * 1024 * 1024;
    /// Default single bulk object ceiling.
    pub const DEFAULT_MAX_SINGLE_BULK_BYTES: u64 = 128 * 1024 * 1024;
    /// Default open resource handles per WebView.
    pub const DEFAULT_MAX_OPEN_RESOURCES: u32 = 4096;
}
