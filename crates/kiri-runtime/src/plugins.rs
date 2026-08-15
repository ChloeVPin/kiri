//! Host-side plugin registration, mirroring `plugin_abi.h` (R-2 in
//! docs/DEEP_AUDIT_TAURI.md). The ABI header defines a C plugin interface
//! (`KiriHostV1` + `KiriHostContextV1` passed to `KiriPluginV1::init`, plus
//! `register_command`). This module provides a Rust-native mirror with the SAME
//! field layout and sizes, so a real `.so`/`.dylib` loader can later pass a
//! `KiriHostV1*`/`KiriHostContextV1*` straight into `init` without translation.
//!
//! Built-in control-plane commands are supplied as plugins so the registration
//! path is proven end to end and stays headless (no WebView). Stateful plugins
//! (`kiri.diag`, `kiri.open`/`kiri.close`) bind the runtime's shared
//! `Diagnostics`/`ResourceTable`/`CallerId` through `KiriHostContextV1`, which
//! is exactly how an external plugin would reach host services.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
/// in the C ABI. For the Rust mirror the `callback` slot carries the `Handler`
/// encoded as a `u32` (a real loader would carry a function pointer instead).
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

/// Mirror of `KiriHostContextV1`. Host services a plugin may bind during `init`.
/// Pointers are opaque from the C side; the Rust mirror names the concrete types.
#[repr(C)]
pub struct KiriHostContextV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub diagnostics: *const kiri_core::diagnostics::Diagnostics,
    pub resource_table: *const Mutex<kiri_core::resources::ResourceTable<()>>,
    pub caller_id: u64,
}

/// Mirror of `KiriPluginV1` (C signature: `init(host, ctx)`).
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
    pub init: extern "C" fn(*const KiriHostV1, *const KiriHostContextV1),
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
/// `register_plugin` runs a plugin's `init` with a host vtable + context that
/// forwards `register_command` calls into the router as real `Handler`s.
pub struct PluginHost {
    router: Router,
    plugins: HashMap<String, LoadedPlugin>,
    /// Maps a plugin-supplied command key (the `KiriHandle`/`u32` passed to
    /// `register_command`) to the actual handler. Populated during `init`.
    pending: HashMap<u32, (u32, Handler)>,
    /// Side table of real `Handler`s indexed by the opaque `u32` handle a plugin
    /// passes to `register_command`. Lets the `u32` slot stay a handle (matching
    /// the C `KiriHandle` semantics) while the handler itself is a fat pointer.
    handler_store: HashMap<u32, Handler>,
    next_handle: u32,
}

impl PluginHost {
    /// Create a host seeded with an empty router so built-in commands come
    /// exclusively from loaded plugins (R-2), proving the registration path
    /// instead of relying on inline defaults.
    pub fn new() -> Self {
        PluginHost {
            router: Router::new_empty(),
            plugins: HashMap::new(),
            pending: HashMap::new(),
            handler_store: HashMap::new(),
            next_handle: 1,
        }
    }

    /// Load and initialize a plugin. `init` is called with the host vtable and
    /// the shared context; `init` calls `register_command` one or more times,
    /// each stashing a handler in `pending` keyed by the supplied command id.
    /// After `init` returns, the pending handlers are merged into the router and
    /// the plugin is recorded as loaded.
    pub fn register_plugin(&mut self, plugin: &KiriPluginV1) -> Result<(), PluginError> {
        if plugin.abi_version != 1 {
            return Err(PluginError::UnsupportedAbi(plugin.abi_version));
        }
        let name = String::from_utf8_lossy(plugin.name).to_string();

        let host = KiriHostV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostV1>() as u32,
            log: host_log,
            register_command: host_register_command,
        };
        let ctx = KiriHostContextV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostContextV1>() as u32,
            diagnostics: std::ptr::null(),
            resource_table: std::ptr::null(),
            caller_id: 0,
        };
        let self_ptr = self as *mut PluginHost;
        with_host_ptr(self_ptr, || {
            (plugin.init)(&host as *const KiriHostV1, &ctx as *const KiriHostContextV1);
        });

        // Merge pending handlers into the router. Capability bit is derived from
        // the command id via `capability_for`, so each plugin command enforces
        // exactly the same authority as the previous inline registration.
        let pending = std::mem::take(&mut self.pending);
        let mut ids = Vec::new();
        for (_key, (id, handler)) in pending {
            let mut required = CapabilityBits::empty();
            required.set(capability_bit::for_command(id));
            self.router.register(id, required, handler);
            ids.push(id);
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

    /// Register a built-in plugin, passing a specific context pointer to its
    /// `init`. Used by `build_router_with_plugins` so stateful built-ins bind
    /// the runtime's shared services. Separate from the public `register_plugin`
    /// (which uses a null context) so the external loader contract stays clean.
    fn register_builtin_with_context(
        &mut self,
        plugin: &KiriPluginV1,
        ctx: *const KiriHostContextV1,
    ) -> Result<(), PluginError> {
        if plugin.abi_version != 1 {
            return Err(PluginError::UnsupportedAbi(plugin.abi_version));
        }
        let name = String::from_utf8_lossy(plugin.name).to_string();
        let host = KiriHostV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostV1>() as u32,
            log: host_log,
            register_command: host_register_command,
        };
        let self_ptr = self as *mut PluginHost;
        with_host_ptr(self_ptr, || {
            (plugin.init)(&host as *const KiriHostV1, ctx);
        });
        let pending = std::mem::take(&mut self.pending);
        let mut ids = Vec::new();
        for (_key, (id, handler)) in pending {
            let mut required = CapabilityBits::empty();
            required.set(capability_bit::for_command(id));
            self.router.register(id, required, handler);
            ids.push(id);
        }
        self.plugins.insert(
            name.clone(),
            LoadedPlugin { name, registered_commands: ids, shutdown: plugin.shutdown },
        );
        Ok(())
    }

    /// Build a router with all built-in plugins loaded (ping, diag, resources).
    /// Replaces the inline `Router::new()` so every built-in command arrives via
    /// the plugin registration path (R-2). Stateful plugins (`diag`, `open`,
    /// `close`) bind the runtime's shared `diagnostics`/`resource_table`/`caller`
    /// through the ABI context, exactly as an external plugin would.
    pub fn build_router_with_plugins(
        diagnostics: &kiri_core::diagnostics::Diagnostics,
        resource_table: &Arc<Mutex<kiri_core::resources::ResourceTable<()>>>,
        caller: kiri_core::caller::CallerId,
    ) -> Router {
        let mut host = PluginHost::new();
        // Share the runtime's services with the plugins via the ABI context.
        let ctx = KiriHostContextV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostContextV1>() as u32,
            diagnostics: diagnostics as *const kiri_core::diagnostics::Diagnostics,
            resource_table: std::sync::Arc::as_ptr(resource_table),
            caller_id: caller.0,
        };
        // diag + resources bind the shared context; ping needs none.
        host.register_builtin_with_context(&PING_PLUGIN, std::ptr::null())
            .expect("built-in ping plugin must load");
        host.register_builtin_with_context(&DIAG_PLUGIN, &ctx as *const _)
            .expect("built-in diag plugin must load");
        host.register_builtin_with_context(&RESOURCES_PLUGIN, &ctx as *const _)
            .expect("built-in resources plugin must load");
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

/// Host `register_command` implementation (C signature). Maps the plugin's
/// ASCII command name to a runtime command id and pairs it with the handler the
/// plugin supplies via `callback` (for the Rust mirror, `callback` is the
/// `Handler` encoded as a `u32`). A real FFI loader would instead resolve
/// `callback` to a `extern "C" fn` function pointer; the host contract is
/// identical: name -> (id, handler).
extern "C" fn host_register_command(command_id_bytes: KiriBytes, callback: u32) {
    let name = unsafe { String::from_utf8_lossy(command_id_bytes.as_slice()).to_string() };
    let id = match name.as_str() {
        "kiri.ping" => command_id::PING,
        "kiri.diag" => command_id::DIAGNOSTICS,
        "kiri.open" => command_id::RESOURCES_OPEN,
        "kiri.close" => command_id::RESOURCES_CLOSE,
        other => {
            eprintln!("[kiri-plugin] unknown command id string: {other}");
            return;
        }
    };
    with_host_mut(|host| {
        // callback is an opaque handle into handler_store.
        if let Some(handler) = host.handler_store.remove(&callback) {
            host.pending.insert(callback, (id, handler));
        }
    });
}

/// Register one command by name with its handler (used by built-in plugins).
/// Allocates an opaque `u32` handle for the handler and forwards it to the host
/// `register_command` callback, exactly as an external plugin would pass a
/// `KiriHandle`.
fn register_one(name: &[u8], handler: Handler) {
    with_host_mut(|host| {
        let handle = host.next_handle;
        host.next_handle += 1;
        host.handler_store.insert(handle, handler);
        host_register_command(KiriBytes::from_slice(name), handle);
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

// --- Ported built-in plugins (R-2): each control-plane command arrives via
// the plugin registration path, proving the ABI-mirroring host end to end. The
// handlers are the SAME implementations used by `Router::with_*` so behavior is
// unchanged; only the registration mechanism differs.

/// `kiri.ping` (id 1). Liveness probe; pure echo.
pub static PING_PLUGIN: KiriPluginV1 = KiriPluginV1 {
    abi_version: 1,
    struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
    name: b"kiri.ping" as &'static [u8; 9] as &'static [u8],
    init: ping_plugin_init,
    shutdown: ping_plugin_shutdown,
};

extern "C" fn ping_plugin_init(_host: *const KiriHostV1, _ctx: *const KiriHostContextV1) {
    register_one(
        b"kiri.ping",
        Arc::new(|_caller, _request_id, payload| {
            Ok(serde_json::json!({ "pong": true, "echo": payload }))
        }),
    );
}

extern "C" fn ping_plugin_shutdown() {}

/// `kiri.diag` (id 2). Privacy-safe runtime snapshot bound to the shared host
/// diagnostics sink passed via context.
pub static DIAG_PLUGIN: KiriPluginV1 = KiriPluginV1 {
    abi_version: 1,
    struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
    name: b"kiri.diag" as &'static [u8; 9] as &'static [u8],
    init: diag_plugin_init,
    shutdown: diag_plugin_shutdown,
};

extern "C" fn diag_plugin_init(_host: *const KiriHostV1, ctx: *const KiriHostContextV1) {
    let diag = unsafe { &*(*ctx).diagnostics };
    register_one(
        b"kiri.diag",
        Arc::new(move |_caller, _request_id, _payload| {
            let snap = diag.snapshot(
                env!("CARGO_PKG_VERSION"),
                if cfg!(target_os = "windows") { "windows" } else { "cross" },
            );
            serde_json::to_value(&snap).map_err(|e| {
                kiri_core::error::Error::internal_error(format!("diagnostics snapshot encode: {e}"))
            })
        }),
    );
}

extern "C" fn diag_plugin_shutdown() {}

/// `kiri.open` (id 3) + `kiri.close` (id 4). Stateful: share the runtime's
/// generational resource table and diagnostics sink via context (T011 honest
/// open-resource count).
pub static RESOURCES_PLUGIN: KiriPluginV1 = KiriPluginV1 {
    abi_version: 1,
    struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
    name: b"kiri.resources" as &'static [u8; 14] as &'static [u8],
    init: resources_plugin_init,
    shutdown: resources_plugin_shutdown,
};

extern "C" fn resources_plugin_init(_host: *const KiriHostV1, ctx: *const KiriHostContextV1) {
    let table = unsafe { &*(*ctx).resource_table };
    let diag = unsafe { &*(*ctx).diagnostics };

    let open_table = table;
    let open_diag = diag;
    register_one(
        b"kiri.open",
        Arc::new(move |c, _request_id, _payload| {
            let mut t = open_table.lock().unwrap();
            let id = t
                .insert(c, (), 4096)
                .map_err(|e| kiri_core::error::Error::limit_exceeded(e.to_string()))?;
            open_diag.set_open_resources(t.len() as u32);
            Ok(serde_json::json!({ "resource_id": id.into_raw() }))
        }),
    );

    let close_table = table;
    let close_diag = diag;
    register_one(
        b"kiri.close",
        Arc::new(move |c, _request_id, payload| {
            let raw = payload.get("resource_id").and_then(|v| v.as_u64()).ok_or_else(|| {
                kiri_core::error::Error::protocol_error("kiri.close requires numeric resource_id")
            })?;
            let id = kiri_core::resources::ResourceId::from_raw(raw);
            let mut t = close_table.lock().unwrap();
            t.remove(c, id)?;
            close_diag.set_open_resources(t.len() as u32);
            Ok(serde_json::json!({ "closed": true }))
        }),
    );
}

extern "C" fn resources_plugin_shutdown() {}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::dispatch::command_id;
    use kiri_core::trace::NoopTraceSink;
    use kiri_core::CallerId;
    use serde_json::json;

    fn load_all() -> PluginHost {
        let mut host = PluginHost::new();
        // Ping needs no context.
        host.register_plugin(&PING_PLUGIN).expect("ping");
        // Diag/resources bind real shared services through the ABI context, so we
        // drive them with a context like the runtime does (not the null-context
        // public loader). The pointed-to services are leaked so their pointers
        // stay valid for the plugin handlers (mirrors the production host, whose
        // `diagnostics`/`resources` live as long as the event loop runs).
        let diagnostics: &'static kiri_core::diagnostics::Diagnostics =
            Box::leak(Box::new(kiri_core::diagnostics::Diagnostics::new()));
        let resources: &'static std::sync::Mutex<kiri_core::resources::ResourceTable<()>> =
            Box::leak(Box::new(std::sync::Mutex::new(
                kiri_core::resources::ResourceTable::<()>::new(),
            )));
        let caller = kiri_core::caller::CallerId(7);
        let ctx = KiriHostContextV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostContextV1>() as u32,
            diagnostics: diagnostics as *const _,
            resource_table: resources as *const _,
            caller_id: caller.0,
        };
        host.register_builtin_with_context(&DIAG_PLUGIN, &ctx as *const _).expect("diag");
        host.register_builtin_with_context(&RESOURCES_PLUGIN, &ctx as *const _).expect("resources");
        host
    }

    #[test]
    fn all_builtins_register_via_plugin_path() {
        let host = load_all();
        assert!(host.is_known(command_id::PING));
        assert!(host.is_known(command_id::DIAGNOSTICS));
        assert!(host.is_known(command_id::RESOURCES_OPEN));
        assert!(host.is_known(command_id::RESOURCES_CLOSE));
    }

    #[test]
    fn ping_plugin_dispatches() {
        let host = load_all();
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
    fn diag_plugin_dispatches() {
        let host = load_all();
        let caller = CallerId(7);
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::DIAGNOSTICS);
        let req = kiri_core::wire::WireRequest::new(command_id::DIAGNOSTICS, 1, 1, json!(null));
        let resp = host.dispatch(caller, &caps, &req, &mut NoopTraceSink);
        assert!(resp.payload.is_some(), "diag returns a snapshot");
    }

    #[test]
    fn resources_open_close_via_plugin() {
        let host = load_all();
        let caller = CallerId(7);
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::RESOURCES);
        let open = kiri_core::wire::WireRequest::new(command_id::RESOURCES_OPEN, 1, 1, json!(null));
        let oresp = host.dispatch(caller, &caps, &open, &mut NoopTraceSink);
        let rid = oresp
            .payload
            .as_ref()
            .unwrap()
            .get("resource_id")
            .and_then(|v| v.as_u64())
            .expect("resource_id returned");
        let close = kiri_core::wire::WireRequest::new(
            command_id::RESOURCES_CLOSE,
            2,
            1,
            json!({ "resource_id": rid }),
        );
        let cresp = host.dispatch(caller, &caps, &close, &mut NoopTraceSink);
        assert_eq!(cresp.payload.as_ref().unwrap()["closed"], json!(true));
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
