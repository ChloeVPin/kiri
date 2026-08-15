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
    /// Host-owned allowlist governing which external plugins (and which of their
    /// commands) may load. Empty by default: nothing external is permitted until
    /// the host sets a policy.
    allowlist: PluginAllowlist,
    /// When an external plugin is loading, the allowlisted command names for the
    /// current plugin. `None` for built-in registration (no command-level gate).
    current_allowed_commands: Option<std::collections::HashSet<String>>,
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
            allowlist: PluginAllowlist::empty(),
            current_allowed_commands: None,
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

    /// Build a router with all built-in plugins loaded (ping, diag, resources)
    /// and any EXTERNAL plugins named in `manifest` (default-deny: an empty
    /// manifest loads none). Replaces the inline `Router::new()` so every
    /// built-in command arrives via the plugin registration path (R-2), and
    /// third-party plugins arrive only through the host-owned allowlist, which
    /// exceeds Tauri's trust-any-plugin-on-path model. Stateful plugins
    /// (`diag`, `open`, `close`) bind the runtime's shared
    /// `diagnostics`/`resource_table`/`caller` through the ABI context, exactly
    /// as an external plugin would.
    pub fn build_router_with_plugins(
        diagnostics: &kiri_core::diagnostics::Diagnostics,
        resource_table: &Arc<Mutex<kiri_core::resources::ResourceTable<()>>>,
        caller: kiri_core::caller::CallerId,
        manifest: &PluginManifest,
        registry: &PluginRegistry,
    ) -> Router {
        let mut host = PluginHost::new();
        // diag + resources bind the shared context; ping needs none.
        let ctx = KiriHostContextV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriHostContextV1>() as u32,
            diagnostics: diagnostics as *const kiri_core::diagnostics::Diagnostics,
            resource_table: std::sync::Arc::as_ptr(resource_table),
            caller_id: caller.0,
        };
        host.register_builtin_with_context(&PING_PLUGIN, std::ptr::null())
            .expect("built-in ping plugin must load");
        host.register_builtin_with_context(&DIAG_PLUGIN, &ctx as *const _)
            .expect("built-in diag plugin must load");
        host.register_builtin_with_context(&RESOURCES_PLUGIN, &ctx as *const _)
            .expect("built-in resources plugin must load");
        // External plugins: only those named in the host-owned manifest, resolved
        // through the host-owned registry, and gated by the per-plugin command
        // allowlist. Unresolved or disallowed entries are skipped (fail-closed).
        host.load_external_from_manifest(manifest, registry);
        // Disclose only the host-owned inventory (plugin names + allowlisted
        // command names) through kiri.plugin.list. The descriptors themselves
        // never cross the bridge, so the frontend cannot reach an unvetted
        // plugin command. Exceeds Tauri's plugin discovery on the security axis.
        let inventory =
            kiri_core::plugin_inventory::PluginInventory::from_allowed(&manifest.allowed_entries());
        host.router.with_plugin_inventory(inventory)
    }

    /// Resolve and load every external plugin named in `manifest`. Unknown plugin
    /// names are dropped (the registry has no descriptor) and unlisted commands
    /// are dropped by `register_external`'s allowlist gate. Fail-closed: a
    /// missing descriptor or a denied command never widens the surface.
    fn load_external_from_manifest(
        &mut self,
        manifest: &PluginManifest,
        registry: &PluginRegistry,
    ) {
        self.allowlist = manifest.to_allowlist();
        for entry in &manifest.entries {
            match registry.resolve(&entry.name) {
                Some(descriptor) => {
                    if let Err(e) = self.register_external(descriptor) {
                        eprintln!("[kiri-plugin] external load failed for {}: {:?}", entry.name, e);
                    }
                }
                None => {
                    eprintln!("[kiri-plugin] no descriptor for {} (skipped)", entry.name);
                }
            }
        }
    }

    /// Replace the host-owned plugin allowlist (G-2 external-plugin policy).
    pub fn set_allowlist(&mut self, allowlist: PluginAllowlist) {
        self.allowlist = allowlist;
    }

    /// Load an EXTERNAL plugin (shipped as a separate library) through the
    /// host-owned allowlist. The plugin name must be on `self.allowlist`; if not,
    /// loading is refused before `init` runs. Every command the plugin registers
    /// is then checked against that plugin's allowlisted command set and dropped
    /// if absent (fail-closed). This is the G-2 on-ramp: third-party code can
    /// extend Kiri only through a surface the host explicitly approved, which
    /// exceeds Tauri's plugin model (any plugin on the configured path is
    /// trusted). A real loader resolves the `KiriPluginV1` via a `dlopen`/entry
    /// point and hands the resulting descriptor here; the allowlist gate is
    /// identical either way.
    pub fn register_external(&mut self, plugin: &KiriPluginV1) -> Result<(), PluginError> {
        let name = String::from_utf8_lossy(plugin.name).to_string();
        if !self.allowlist.is_plugin_allowed(&name) {
            return Err(PluginError::NotAllowed(name));
        }
        self.current_allowed_commands = self.allowlist.commands_for(&name).cloned();
        let result = self.register_plugin(plugin);
        self.current_allowed_commands = None;
        result
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
    /// The plugin name is not present on the host-owned allowlist.
    NotAllowed(String),
    /// Loading or resolving the plugin library failed (dlopen/entry point).
    LoadError(String),
}

/// Host-owned plugin allowlist (G-2 external-plugin security model). An external
/// plugin is only loadable when its name is present, and only the command names
/// listed for it are registered; everything else is dropped. This exceeds
/// Tauri's plugin loading, which trusts any plugin on the configured path, by
/// requiring an explicit, named, command-level allowlist.
#[derive(Debug, Clone, Default)]
pub struct PluginAllowlist {
    plugins: std::collections::HashMap<String, std::collections::HashSet<String>>,
}

impl PluginAllowlist {
    pub fn empty() -> Self {
        PluginAllowlist { plugins: std::collections::HashMap::new() }
    }
    /// Allow `plugin` to load, restricted to the exact `commands` it may expose.
    pub fn allow<P: Into<String>, C: AsRef<str>>(mut self, plugin: P, commands: &[C]) -> Self {
        let set: std::collections::HashSet<String> =
            commands.iter().map(|c| c.as_ref().to_string()).collect();
        self.plugins.insert(plugin.into(), set);
        self
    }
    pub fn is_plugin_allowed(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }
    pub fn commands_for(&self, name: &str) -> Option<&std::collections::HashSet<String>> {
        self.plugins.get(name)
    }
}

/// A host-owned, default-deny policy for external plugins. Parsed from a
/// manifest the host supplies at startup (e.g. an embedded `&[u8]` or a file the
/// host reads). An empty manifest loads ZERO external plugins; the host must
/// explicitly name each plugin and the exact commands it may expose. This is the
/// concrete form of `PluginAllowlist` that the runtime consumes at boot, and it
/// is what exceeds Tauri (which loads any plugin found on the configured path).
#[derive(Debug, Clone, Default)]
pub struct PluginManifest {
    /// Entries are resolved by name against a host-provided descriptor registry.
    entries: Vec<PluginManifestEntry>,
}

#[derive(Debug, Clone)]
struct PluginManifestEntry {
    name: String,
    commands: Vec<String>,
    /// Opaque locator for the host to resolve the descriptor (path, in a real
    /// loader). Kept as a string so the manifest is serializable and host-owned.
    /// Reserved for the real dlopen-based loader; not yet read on the headless
    /// path where descriptors come from the in-process `PluginRegistry`.
    #[allow(dead_code)]
    library: String,
}

impl PluginManifest {
    pub fn empty() -> Self {
        PluginManifest { entries: Vec::new() }
    }

    /// Parse a default-deny manifest from JSON bytes supplied by the host.
    /// Format: `{ "plugins": [ { "name": str, "commands": [str], "library": str } ] }`.
    /// Any malformed entry is skipped (fail-closed): a bad manifest never
    /// broadens the plugin surface.
    pub fn from_json(bytes: &[u8]) -> Self {
        let mut manifest = PluginManifest { entries: Vec::new() };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return manifest;
        };
        let Some(list) = value.get("plugins").and_then(|v| v.as_array()) else {
            return manifest;
        };
        for item in list {
            let (Some(name), Some(cmds), Some(lib)) = (
                item.get("name").and_then(|v| v.as_str()),
                item.get("commands").and_then(|v| v.as_array()),
                item.get("library").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let commands: Vec<String> =
                cmds.iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect();
            manifest.entries.push(PluginManifestEntry {
                name: name.to_string(),
                commands,
                library: lib.to_string(),
            });
        }
        manifest
    }

    /// Host-owned disclosure of the manifest: the (name, commands) pairs the host
    /// is willing to expose via `kiri.plugin.list`. No descriptor pointers, no
    /// library paths. This is exactly what the frontend may discover.
    pub fn allowed_entries(&self) -> Vec<(String, Vec<String>)> {
        self.entries.iter().map(|e| (e.name.clone(), e.commands.clone())).collect()
    }
    /// Build the `PluginAllowlist` this manifest implies. Pure projection: every
    /// manifest entry becomes a per-plugin, command-level allowlist entry.
    pub fn to_allowlist(&self) -> PluginAllowlist {
        let mut allow = PluginAllowlist::empty();
        for e in &self.entries {
            let cmds: Vec<&str> = e.commands.iter().map(|s| s.as_str()).collect();
            allow = allow.allow(e.name.clone(), &cmds);
        }
        allow
    }
}

/// Host-owned registry mapping a plugin name + library locator to its
/// `KiriPluginV1` descriptor. In a real loader this is where `dlopen`/entry-point
/// resolution would happen; for the headless, cross-platform path the host
/// supplies descriptors (built-ins-extended or test fakes). The allowlist gate
/// applied by `register_external` is identical regardless of how the descriptor
/// is obtained, so the security property that exceeds Tauri holds.
pub struct PluginRegistry {
    descriptors: std::collections::HashMap<String, *const KiriPluginV1>,
}

impl PluginRegistry {
    pub fn empty() -> Self {
        PluginRegistry { descriptors: std::collections::HashMap::new() }
    }
    /// Register a descriptor under `name`. The pointer must outlive the loader
    /// call (in practice: a `static`).
    pub fn register(&mut self, name: String, descriptor: &'static KiriPluginV1) {
        self.descriptors.insert(name, descriptor as *const KiriPluginV1);
    }
    fn resolve(&self, name: &str) -> Option<&'static KiriPluginV1> {
        self.descriptors.get(name).map(|p| unsafe { &**p })
    }
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
    // Fail-closed: when an external plugin is loading, drop any command whose
    // name is not on that plugin's host-allowlisted set.
    if let Some(allowed) = with_host_allowed_commands() {
        if !allowed.contains(&name) {
            eprintln!("[kiri-plugin] command {name} not on allowlist; dropped");
            return;
        }
    }
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

fn with_host_allowed_commands() -> Option<std::collections::HashSet<String>> {
    HOST_PTR.with(|c| {
        let ptr = c.get();
        if ptr.is_null() {
            return None;
        }
        let host = unsafe { &*ptr };
        host.current_allowed_commands.clone()
    })
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

    #[test]
    fn external_loader_rejects_unknown_plugin() {
        let mut host = PluginHost::new();
        host.set_allowlist(PluginAllowlist::empty());
        let bad = KiriPluginV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
            name: b"evil.plugin" as &'static [u8; 11] as &'static [u8],
            init: ping_plugin_init,
            shutdown: ping_plugin_shutdown,
        };
        let err = host.register_external(&bad);
        assert_eq!(err, Err(PluginError::NotAllowed("evil.plugin".to_string())));
        assert!(!host.is_known(command_id::PING), "no commands leaked");
    }

    #[test]
    fn external_loader_allows_plugin_but_drops_unlisted_command() {
        let mut host = PluginHost::new();
        // Allow the plugin but with an EMPTY command set: fail-closed means the
        // command it registers is dropped, so the router never learns it.
        host.set_allowlist(PluginAllowlist::empty().allow("kiri.ping", &[] as &[&str]));
        let ping = KiriPluginV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
            name: b"kiri.ping" as &'static [u8; 9] as &'static [u8],
            init: ping_plugin_init,
            shutdown: ping_plugin_shutdown,
        };
        host.register_external(&ping).expect("plugin name allowed");
        assert!(!host.is_known(command_id::PING), "unlisted command dropped");
    }

    #[test]
    fn external_loader_allows_plugin_and_command() {
        let mut host = PluginHost::new();
        host.set_allowlist(PluginAllowlist::empty().allow("kiri.ping", &["kiri.ping"]));
        let ping = KiriPluginV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<KiriPluginV1>() as u32,
            name: b"kiri.ping" as &'static [u8; 9] as &'static [u8],
            init: ping_plugin_init,
            shutdown: ping_plugin_shutdown,
        };
        host.register_external(&ping).expect("plugin + command allowed");
        assert!(host.is_known(command_id::PING), "allowlisted command registered");
    }

    #[test]
    fn manifest_empty_loads_no_external() {
        // Default-deny: an empty manifest resolves to an empty allowlist and a
        // registry with no descriptors, so no external command is registered.
        let manifest = PluginManifest::empty();
        let registry = PluginRegistry::empty();
        let mut host = PluginHost::new();
        host.load_external_from_manifest(&manifest, &registry);
        assert!(!host.is_known(command_id::PING), "no external command from empty manifest");
        assert_eq!(host.loaded_plugin_names().len(), 0);
    }

    #[test]
    fn manifest_from_json_gates_external_by_name_and_command() {
        // Manifest names the plugin and an allowlisted command set. Without a
        // registry descriptor the load is skipped (fail-closed); with one, the
        // allowlisted command registers and nothing else does.
        let json = br#"{"plugins":[{"name":"kiri.ping","commands":["kiri.ping"],"library":"libkiri_ping.dylib"}]}"#;
        let manifest = PluginManifest::from_json(json);
        let mut registry = PluginRegistry::empty();
        registry.register("kiri.ping".to_string(), &PING_PLUGIN);
        let mut host = PluginHost::new();
        host.load_external_from_manifest(&manifest, &registry);
        assert!(host.is_known(command_id::PING), "manifest-allowed command registered");
        assert_eq!(host.loaded_plugin_names(), vec!["kiri.ping"]);
    }

    #[test]
    fn manifest_from_json_with_unknown_plugin_skips() {
        let json = br#"{"plugins":[{"name":"ghost","commands":["x"],"library":"libghost.dylib"}]}"#;
        let manifest = PluginManifest::from_json(json);
        let registry = PluginRegistry::empty();
        let mut host = PluginHost::new();
        host.load_external_from_manifest(&manifest, &registry);
        assert_eq!(host.loaded_plugin_names().len(), 0, "unknown plugin skipped");
    }
}
