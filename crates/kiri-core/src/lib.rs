//! Kiri Core: the platform-neutral logical protocol and security authority.
//!
//! This crate implements the logical control-plane contracts defined in the
//! Kiri corpus (`specs/ERRORS.md`, `specs/IPC.md`, `specs/RESOURCES.md`,
//! `specs/SECURITY.md`, `specs/TRACE.md`) as pure, testable Rust.
//!
//! It deliberately contains no platform transport code. Windows, macOS, and
//! Linux backends map these contracts onto their physical WebView transports.

pub mod caller;
pub mod capabilities;
pub mod commands;
pub mod diagnostics;
pub mod dispatch;
pub mod error;
pub mod header;
pub mod latency;
pub mod limits;
pub mod resources;
pub mod security;
pub mod trace;
pub mod validate;
pub mod wire;

pub use caller::{CallerId, CallerRegistry};
pub use capabilities::{CapabilityBits, CapabilityId, PathScope, Scope};
pub use commands::{
    command_name, emit_typescript, required_capabilities, resolve_command, CommandSpec, COMMANDS,
};
pub use dispatch::{is_pong, ping_request, Router, StaticRouter};
pub use error::{Error, ErrorCode, Result};
pub use header::{ControlFlags, ControlHeader, MAGIC, PROTOCOL_VERSION};
pub use latency::{LatencyDistribution, LatencySummary};
pub use limits::Limits;
pub use resources::{ResourceId, ResourceTable};
pub use security::{
    is_app_origin, is_navigation_allowed, trusted_frontend_capabilities, CROSS_APP_ORIGIN,
    WINDOWS_APP_ORIGIN,
};
pub use trace::{MonotonicClock, Stage, TraceEvent, TraceSink};
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
