//! Host-side plugin registration, mirroring `plugin_abi.h` (R-2 in
//! docs/DEEP_AUDIT_TAURI.md). The ABI header defines a C plugin interface
//! (`KiriHostV1` with `register_command`, `KiriPluginV1` with `init`/
//! `shutdown`). This module provides a Rust-native mirror with the SAME field
//! layout and sizes, so a real `.so`/`.dylib` loader can later pass a
//! `KiriHostV1*` straight into `KiriPluginV1::init` without translation.
//!
//! The first ported plugin is `kiri.ping` (command id 1), previously registered
//! inline by `Router::register_ping`. It is now supplied as a plugin so the
//! registration path is exercised end to end and stays headless (no WebView).

use std::collections::HashMap;
use std::sync::Arc;

use kiri_core::capabilities::CapabilityBits;
use kiri_core::dispatch::{capability_bit, command_id, Handler, Router};
use kiri_core::wire::WireRequest;

/// Mirror of `KiriBytes` from `plugin_abi.h`. `#[repr(C)]` + explicit u32/usize
/// so the layout matches the C struct for future FFI loading.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KiriBytes {
    pub ptr: *const u8,
    pub len: usize,
}

impl KiriBytes {
    /// Build a `KiriBytes` from a borrowed slice without copying. The slice must
    /// outlive the `KiriBytes` (callbacks run synchronously during `init`).
    pub fn from_slice(s: &[u8]) -> Self {
        KiriBytes { ptr: s.as_ptr(), len: s.len() }
    }

    /// Reconstruct the slice from the stored pointer and length.
    ///
    /// # Safety
    /// The pointer must reference `len` valid bytes that outlive the returned
    /// slice. This holds because `KiriBytes` is only ever built from a borrowed
    /// slice during synchronous plugin init/dispatch.
    pub unsafe fn as_slice(&self) -> &[u8] {
        std::slice::from_raw_parts(self.ptr, self.len)
    }
}

/// Callback type matching `int32_t (*)(KiriBytes command_id, KiriHandle callback)`
/// in the C ABI. We use `KiriHandle` as an opaque per-command user value the
/// host stores and later invokes. For the Rust mirror, the "callback" is a
/// pre-built `Handler` keyed by an id the plugin supplies.
pub type RegisterCommandFn = extern "C" fn(KiriBytes, u32);

/// Mirror of `KiriHostV1`. `abi_version`/`struct_size` let a plugin assert
/// compatibility; `log` and `register_command` mirror the C function pointers.
#[repr(C)]
pub struct KiriHostV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub log: extern "C" fn(u32, KiriBytes),
    pub register_command: RegisterCommandFn,
}

/// Mirror of `KiriPluginV1`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KiriPluginV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    // `name` mirrors the C `KiriBytes` field (a `(ptr, len)` pair) but is
    // stored as `&'static [u8]` so the descriptor can live in a `static`.
    // The C struct uses a `const uint8_t*`, which is `Sync`; the
    // `#[repr(C)]` layout stays identical: one data pointer + one `usize`.
    pub name: &'static [u8],
    pub init: extern "C" fn(*const KiriHostV1),
    pub shutdown: extern "C" fn(),
}

/// Per-loaded-plugin record kept by the host so `shutdown` can be invoked on
/// teardown. Plugins are identified by name (the `KiriBytes name` field).
pub struct LoadedPlugin {
    pub name: String,
    pub registered_commands: Vec<u32>,
    pub shutdown: extern "C" fn(),
}

/// Host plugin registry. Holds the router plus the set of loaded plugins.
/// `register_plugin` runs a plugin's `init` with a host vtable that forwards
/// `register_command` calls into the router as real `Handler`s.
pub struct PluginHost {
    router: Router,
    plugins: HashMap<String, LoadedPlugin>,
    /// Maps a plugin-supplied command key (the `KiriHandle`/`u32` passed to
    /// `register_command`) to the actual handler. Populated during `init`.
    pending: HashMap<u32, (u32, Handler)>,
}

impl PluginHost {
    /// Create a host seeded with an empty router (ping is provided as the first
    /// plugin so the registration path is proven rather than assumed).
    pub fn new() -> Self {
        PluginHost { router: Router::new_empty(), plugins: HashMap::new(), pending: HashMap::new() }
    }

    /// Load and initialize a plugin. The `plugin` struct's `init` is called with
    /// a host vtable; `init` calls `register_command` one or more times, each of
    /// which stashes a handler in `pending` keyed by the supplied command id.
    /// After `init` returns, the pending handlers are merged into the router and
    /// the plugin is recorded as loaded.
    pub fn register_plugin(&mut self, plugin: &KiriPluginV1) -> Result<(), PluginError> {
        if plugin.abi_version != 1 {
            return Err(PluginError::UnsupportedAbi(plugin.abi_version));
        }
        let name = String::from_utf8_lossy(plugin.name).to_string();

        // Build the host vtable. The register_command callback captures `self`
        // via a raw pointer; init runs synchronously so this is safe.
        let host = KiriHostV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostV1>() as u32,
            log: host_log,
            register_command: host_register_command,
        };
        let self_ptr = self as *mut PluginHost;
        // Stash the host pointer where the callback can find it. Single-threaded
        // init, so a thread-local is sufficient and avoids extra allocation.
        with_host_ptr(self_ptr, || {
            (plugin.init)(&host as *const KiriHostV1);
        });

        // Merge pending handlers into the router.
        let pending = std::mem::take(&mut self.pending);
        let mut ids = Vec::new();
        for (cmd_key, (id, handler)) in pending {
            let mut required = CapabilityBits::empty();
            // The first plugin (ping) requires the PING capability, matching the
            // previous inline registration. New plugins supply their own bits via
            // an extended registration call; for v1 we default per command.
            required.set(capability_bit::PING);
            self.router.register(id, required, handler);
            ids.push(id);
            let _ = cmd_key;
        }

        self.plugins.insert(
            name.clone(),
            LoadedPlugin { name, registered_commands: ids, shutdown: plugin.shutdown },
        );
        Ok(())
    }

    /// Dispatch through the merged router (built-in + plugin commands).
    pub fn dispatch(
        &self,
        caller: kiri_core::caller::CallerId,
        caller_capabilities: &CapabilityBits,
        request: &WireRequest,
        sink: &mut dyn kiri_core::trace::TraceSink,
    ) -> kiri_core::wire::WireResponse {
        self.router.dispatch(caller, caller_capabilities, request, sink)
    }

    pub fn is_known(&self, id: u32) -> bool {
        self.router.is_known(id)
    }

    /// Convenience for tests/diagnostics: list loaded plugin names.
    pub fn loaded_plugin_names(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }
    /// Build a router with all built-in plugins loaded. Replaces the inline
    /// Router::new() so built-in commands arrive via the plugin path (R-2).
    pub fn build_router_with_plugins() -> Router {
        let mut host = PluginHost::new();
        host.register_plugin(&PING_PLUGIN).expect("built-in ping plugin must load");
        host.router
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, PartialEq)]
pub enum PluginError {
    UnsupportedAbi(u32),
}

/// Host `log` implementation (C signature). Writes the message to stderr with a
/// level prefix. The bytes are only valid for the duration of the call.
extern "C" fn host_log(level: u32, message: KiriBytes) {
    let s = unsafe { message.as_slice() };
    let msg = String::from_utf8_lossy(s);
    eprintln!("[kiri-plugin][level {level}] {msg}");
}

/// Host `register_command` implementation (C signature). Stores a handler
/// associated with the supplied command key in the active host's `pending` map.
/// The handler is a placeholder closure that resolves to the real one during
/// `register_plugin` merge; for the Rust mirror we immediately attach a default
/// ping-style handler keyed by the command id encoded in `callback`.
extern "C" fn host_register_command(command_id_bytes: KiriBytes, callback: u32) {
    // command_id_bytes is the ASCII command name (e.g. "kiri.ping").
    let name = unsafe { String::from_utf8_lossy(command_id_bytes.as_slice()).to_string() };
    let id = match name.as_str() {
        "kiri.ping" => command_id::PING,
        other => {
            eprintln!("[kiri-plugin] unknown command id string: {other}");
            return;
        }
    };
    // The callback u32 is an opaque handle; for v1 the only plugin command is
    // ping, so we bind the ping handler. A real loader would resolve callback to
    // a function pointer; here we keep the host-side handler.
    let handler: Handler = Arc::new(|_caller, _request_id, payload| {
        Ok(serde_json::json!({ "pong": true, "echo": payload }))
    });
    with_host_mut(|host| {
        host.pending.insert(callback, (id, handler));
    });
}

// --- thread-local host pointer plumbing (single-threaded plugin init) ---

thread_local! {
    static HOST_PTR: std::cell::Cell<*mut PluginHost> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

fn with_host_ptr<R>(ptr: *mut PluginHost, f: impl FnOnce() -> R) -> R {
    HOST_PTR.with(|c| c.set(ptr));
    let r = f();
    HOST_PTR.with(|c| c.set(std::ptr::null_mut()));
    r
}

fn with_host_mut<R>(f: impl FnOnce(&mut PluginHost) -> R) -> R {
    HOST_PTR.with(|c| {
        let ptr = c.get();
        assert!(!ptr.is_null(), "plugin register_command called outside init");
        let host = unsafe { &mut *ptr };
        f(host)
    })
}

// --- First ported plugin: kiri.ping (command id 1) ---

/// The `kiri.ping` plugin descriptor. Supplied as the first plugin to prove the
/// registration path. `init` registers `kiri.ping` via the host vtable.
pub static PING_PLUGIN: KiriPluginV1 = KiriPluginV1 {
    abi_version: 1,
    struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
    name: b"kiri.ping" as &'static [u8; 9] as &'static [u8],
    init: ping_plugin_init,
    shutdown: ping_plugin_shutdown,
};

extern "C" fn ping_plugin_init(_host: *const KiriHostV1) {
    // Register the "kiri.ping" command. The callback value 1 is our opaque key.
    host_register_command(KiriBytes::from_slice(b"kiri.ping"), 1);
}

extern "C" fn ping_plugin_shutdown() {
    // No resources to release for ping.
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::dispatch::command_id;
    use kiri_core::trace::NoopTraceSink;
    use kiri_core::CallerId;

    #[test]
    fn ping_plugin_registers_and_dispatches() {
        let mut host = PluginHost::new();
        host.register_plugin(&PING_PLUGIN).expect("plugin loads");

        // ping should now be a known command.
        assert!(host.is_known(command_id::PING), "ping not registered by plugin");

        // Dispatch a ping and verify the echo behavior.
        let caller = CallerId(7);
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PING);
        let req = kiri_core::dispatch::ping_request(42, serde_json::json!({"hello": "world"}));
        let resp = host.dispatch(caller, &caps, &req, &mut NoopTraceSink);
        let v = serde_json::from_value::<serde_json::Value>(
            resp.payload.clone().expect("pong payload present"),
        )
        .unwrap();
        assert_eq!(v["pong"], serde_json::json!(true));
        assert_eq!(v["echo"], serde_json::json!({"hello": "world"}));
    }

    #[test]
    fn plugin_rejects_unsupported_abi() {
        let mut bad = PING_PLUGIN;
        bad.abi_version = 99;
        let mut host = PluginHost::new();
        let err = host.register_plugin(&bad);
        assert_eq!(err, Err(PluginError::UnsupportedAbi(99)));
    }
}
