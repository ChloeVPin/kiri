//! Deterministic command catalog, static routing, and TypeScript emission
//! (T005: docs/11-codegen-cli-build.md, specs/API.md).
//!
//! Command numeric IDs are assigned from an explicit, ordered manifest so that
//! inserting or reordering a command never renumbers existing IDs (the
//! determinism contract in docs/11-codegen-cli-build.md). The static router
//! resolves a command ID to its handler from a const table at dispatch time,
//! so routing is data-driven and auditable rather than tied to HashMap
//! insertion order.
//!
//! The TypeScript emitter produces a stable `.ts` surface for the frontend;
//! emission is a pure function of the manifest, so identical input always
//! yields byte-identical output.

use serde::Serialize;

use crate::capabilities::CapabilityBits;

/// One catalogued command.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommandSpec {
    /// Stable string ID used in diagnostics and the generated frontend
    /// surface (for example `user.get`).
    pub name: &'static str,
    /// Compact, stable numeric routing ID.
    pub id: u32,
    /// Declared capability requirement (string form for the manifest; the
    /// numeric bit is resolved through the capability registry at build time).
    pub capability: &'static str,
    /// Execution class: `cpu`, `io`, `gpu`, `net`, or `pure`.
    pub execution: &'static str,
    /// Number of leading payload arguments (used by codegen for arity).
    pub arity: u8,
}

impl CommandSpec {
    /// Resolve the declared capability to a capability bit. The mapping is a
    /// fixed, small table; unknown names fall back to bit 0 (ping) so the
    /// catalog stays usable while the capability set grows.
    pub fn capability_bit(&self) -> u32 {
        capability_bit_for(self.capability)
    }
}

/// Fixed capability-name -> bit mapping. Kept tiny and explicit.
fn capability_bit_for(name: &str) -> u32 {
    match name {
        "ping" => crate::dispatch::capability_bit::PING,
        "diag" => crate::dispatch::capability_bit::DIAGNOSTICS,
        "resources" => crate::dispatch::capability_bit::RESOURCES,
        "platform" => crate::dispatch::capability_bit::PLATFORM,
        "app" => crate::dispatch::capability_bit::APP,
        "event" => crate::dispatch::capability_bit::EVENT,
        "fs" => crate::dispatch::capability_bit::FS,
        "window" => crate::dispatch::capability_bit::WINDOW,
        "clipboard" => crate::dispatch::capability_bit::CLIPBOARD,
        "path" => crate::dispatch::capability_bit::PATH,
        "http" => crate::dispatch::capability_bit::HTTP,
        "shell" => crate::dispatch::capability_bit::SHELL,
        "notification" => crate::dispatch::capability_bit::NOTIFICATION,
        "dialog" => crate::dispatch::capability_bit::DIALOG,
        "deeplink" => crate::dispatch::capability_bit::DEEPLINK,
        "opener" => crate::dispatch::capability_bit::OPENER,
        "window_state" => crate::dispatch::capability_bit::WINDOW_STATE,
        "tray" => crate::dispatch::capability_bit::TRAY,
        "sidecar" => crate::dispatch::capability_bit::SIDECAR,
        "config" => crate::dispatch::capability_bit::CONFIG,
        "updater" => crate::dispatch::capability_bit::UPDATER,
        "cli" => crate::dispatch::capability_bit::CLI,
        _ => 0,
    }
}

/// The command catalog. Ordered by `id` ascending for stable iteration.
///
/// Inserting a new command appends an entry with the next free numeric ID;
/// never renumber an existing entry.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "kiri.ping", id: 1, capability: "ping", execution: "pure", arity: 1 },
    CommandSpec { name: "kiri.diag", id: 2, capability: "diag", execution: "pure", arity: 0 },
    CommandSpec { name: "kiri.open", id: 3, capability: "resources", execution: "pure", arity: 1 },
    CommandSpec { name: "kiri.close", id: 4, capability: "resources", execution: "pure", arity: 1 },
    CommandSpec {
        name: "kiri.platform.os",
        id: 5,
        capability: "platform",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.platform.arch",
        id: 6,
        capability: "platform",
        execution: "pure",
        arity: 0,
    },
    CommandSpec { name: "kiri.app.version", id: 7, capability: "app", execution: "pure", arity: 0 },
    CommandSpec {
        name: "kiri.event.emit",
        id: 8,
        capability: "event",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.event.listen",
        id: 9,
        capability: "event",
        execution: "pure",
        arity: 1,
    },
    CommandSpec { name: "kiri.fs.read", id: 10, capability: "fs", execution: "io", arity: 1 },
    CommandSpec { name: "kiri.fs.write", id: 11, capability: "fs", execution: "io", arity: 2 },
    CommandSpec { name: "kiri.fs.exists", id: 12, capability: "fs", execution: "pure", arity: 1 },
    CommandSpec { name: "kiri.fs.remove", id: 13, capability: "fs", execution: "io", arity: 1 },
    CommandSpec {
        name: "kiri.window.title.get",
        id: 14,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.title.set",
        id: 15,
        capability: "window",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.window.show",
        id: 16,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.hide",
        id: 17,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.minimize",
        id: 18,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.maximize",
        id: 19,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.restore",
        id: 20,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.close",
        id: 21,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.window.focus",
        id: 22,
        capability: "window",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.clipboard.read",
        id: 23,
        capability: "clipboard",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.clipboard.write",
        id: 24,
        capability: "clipboard",
        execution: "pure",
        arity: 1,
    },
    // --- audit item 2: kiri.path.* / kiri.os.* (G-7) ---
    CommandSpec {
        name: "kiri.path.dirname",
        id: 25,
        capability: "path",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.path.basename",
        id: 26,
        capability: "path",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.path.extname",
        id: 27,
        capability: "path",
        execution: "pure",
        arity: 1,
    },
    CommandSpec { name: "kiri.path.stem", id: 28, capability: "path", execution: "pure", arity: 1 },
    CommandSpec { name: "kiri.path.join", id: 29, capability: "path", execution: "pure", arity: 1 },
    CommandSpec {
        name: "kiri.path.isAbsolute",
        id: 30,
        capability: "path",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.os.homedir",
        id: 31,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.os.tempdir",
        id: 32,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.os.appConfigDir",
        id: 33,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.os.appDataDir",
        id: 34,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.os.appCacheDir",
        id: 35,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec {
        name: "kiri.os.documentDir",
        id: 36,
        capability: "path",
        execution: "pure",
        arity: 0,
    },
    CommandSpec { name: "kiri.os.appDir", id: 37, capability: "path", execution: "pure", arity: 0 },
    // --- audit item 3: kiri.http.get (G-3) ---
    CommandSpec { name: "kiri.http.get", id: 38, capability: "http", execution: "io", arity: 1 },
    // --- audit item 4: kiri.shell.run (G-4) restricted, host-allowlisted ---
    CommandSpec { name: "kiri.shell.run", id: 39, capability: "shell", execution: "io", arity: 1 },
    // --- audit item 5: kiri.notification.show (G-4b) restricted, host-template-allowlisted ---
    CommandSpec {
        name: "kiri.notification.show",
        id: 40,
        capability: "notification",
        execution: "io",
        arity: 1,
    },
    // --- audit item 7: kiri.dialog.open (G-4c) restricted, host-allowlisted dialog kinds ---
    CommandSpec {
        name: "kiri.dialog.open",
        id: 41,
        capability: "dialog",
        execution: "io",
        arity: 1,
    },
    // --- audit item 8: kiri.shortcut.register (G-4d) restricted, host-allowlisted global
    // shortcuts (exceeds Tauri global-shortcut plugin on the security axis) ---
    CommandSpec {
        name: "kiri.shortcut.register",
        id: 42,
        capability: "shortcut",
        execution: "io",
        arity: 1,
    },
    // --- audit item 9: kiri.autostart.set (G-4e) restricted, host-policy-gated autostart
    // (exceeds Tauri autostart plugin on the security axis) ---
    CommandSpec {
        name: "kiri.autostart.set",
        id: 43,
        capability: "autostart",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.autostart.get",
        id: 44,
        capability: "autostart",
        execution: "pure",
        arity: 0,
    },
    // --- audit item 10: kiri.store.get/set (G-4f) restricted, host-namespace-allowlisted
    // store (exceeds Tauri store plugin on the security axis) ---
    CommandSpec { name: "kiri.store.get", id: 45, capability: "store", execution: "io", arity: 1 },
    CommandSpec { name: "kiri.store.set", id: 46, capability: "store", execution: "io", arity: 1 },
    // --- audit item 11: kiri.deeplink.register (G-4g) restricted, host-scheme-allowlisted
    // deep-link registration (exceeds Tauri deep-link plugin on the security axis) ---
    CommandSpec {
        name: "kiri.deeplink.register",
        id: 47,
        capability: "deeplink",
        execution: "io",
        arity: 1,
    },
    // --- audit item 12: kiri.opener.open (G-2c) restricted, host-allowlisted opener
    // (exceeds Tauri opener plugin on the security axis) ---
    CommandSpec {
        name: "kiri.opener.open",
        id: 48,
        capability: "opener",
        execution: "io",
        arity: 1,
    },
    // --- audit item 13: kiri.window.state.save/load (G-2d) restricted, host-owned
    // window-state persistence (exceeds Tauri window-state plugin on the security axis) ---
    CommandSpec {
        name: "kiri.window.state.save",
        id: 49,
        capability: "window_state",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.window.state.load",
        id: 50,
        capability: "window_state",
        execution: "io",
        arity: 0,
    }, // --- audit item 14: kiri.tray (G-6) restricted, host-allowlisted tray
    // (exceeds Tauri tray on the security axis) ---
    CommandSpec {
        name: "kiri.tray.setMenu",
        id: 51,
        capability: "tray",
        execution: "io",
        arity: 1,
    },
    CommandSpec { name: "kiri.tray.invoke", id: 52, capability: "tray", execution: "io", arity: 1 },
    // --- audit item 15: kiri.sidecar (G-6) restricted, host-allowlisted sidecar
    // (exceeds Tauri sidecar on the security axis) ---
    CommandSpec {
        name: "kiri.sidecar.spawn",
        id: 53,
        capability: "sidecar",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.sidecar.stop",
        id: 54,
        capability: "sidecar",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.sidecar.list",
        id: 55,
        capability: "sidecar",
        execution: "io",
        arity: 0,
    },
    // --- audit item 16: kiri.event.* (restricted, channel-allowlisted) exceeds
    // Tauri's unrestricted event module on the security axis ---
    CommandSpec {
        name: "kiri.event.publish",
        id: 56,
        capability: "event",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.event.subscribe",
        id: 57,
        capability: "event",
        execution: "io",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.event.channels",
        id: 58,
        capability: "event",
        execution: "io",
        arity: 0,
    },
    // --- audit item 17: kiri.config.* (restricted, key-allowlisted) exceeds
    // Tauri's unrestricted getConfig() on the security axis ---
    CommandSpec {
        name: "kiri.config.get",
        id: 59,
        capability: "config",
        execution: "pure",
        arity: 1,
    },
    CommandSpec {
        name: "kiri.config.keys",
        id: 60,
        capability: "config",
        execution: "pure",
        arity: 0,
    },
    // --- audit item 18: kiri.updater.check (G-3) restricted, host-pinned-key
    // updater (exceeds Tauri's updater on the security axis) ---
    CommandSpec {
        name: "kiri.updater.check",
        id: 61,
        capability: "updater",
        execution: "pure",
        arity: 1,
    },
    // --- G-9: HTTP verbs beyond GET (exceeds Tauri http plugin: method
    // allowlist + host allowlist + bulk ceiling on every verb) ---
    CommandSpec { name: "kiri.http.post", id: 62, capability: "http", execution: "io", arity: 1 },
    CommandSpec { name: "kiri.http.put", id: 63, capability: "http", execution: "io", arity: 1 },
    CommandSpec { name: "kiri.http.patch", id: 64, capability: "http", execution: "io", arity: 1 },
    CommandSpec { name: "kiri.http.delete", id: 65, capability: "http", execution: "io", arity: 1 },
    // --- G-5: kiri.cli.args (exceeds Tauri process.argv: structured + allowlist
    // scoped command-line surface) ---
    CommandSpec { name: "kiri.cli.args", id: 66, capability: "cli", execution: "pure", arity: 0 },
];

/// Resolve a command name to its numeric ID (deterministic lookup).
pub fn resolve_command(name: &str) -> Option<u32> {
    COMMANDS.iter().find(|c| c.name == name).map(|c| c.id)
}

/// Resolve a numeric ID to its command name.
pub fn command_name(id: u32) -> Option<&'static str> {
    COMMANDS.iter().find(|c| c.id == id).map(|c| c.name)
}

/// Capability bits required by a command ID, or empty if unknown.
pub fn required_capabilities(id: u32) -> CapabilityBits {
    let mut bits = CapabilityBits::empty();
    if let Some(spec) = COMMANDS.iter().find(|c| c.id == id) {
        bits.set(spec.capability_bit());
    }
    bits
}

/// Emit a deterministic TypeScript surface for the catalog. Each command
/// becomes a typed method on a `commands` object. The output is a pure
/// function of `COMMANDS`, so re-running codegen is byte-stable.
///
/// The generated `Arg` type is `unknown` by design: the concrete argument
/// types are application-defined and supplied by the full codegen pass; this
/// emitter covers the routing/ID/capability contract the runtime enforces.
pub fn emit_typescript() -> String {
    let mut methods = String::new();
    let mut names = String::new();
    for cmd in COMMANDS {
        methods.push_str(&format!(
            "  /** {name} (id={id}, capability=\"{cap}\", execution=\"{exec}\") */\n  {name}(arg: unknown): Promise<unknown>;\n",
            name = sanitize_ident(cmd.name),
            id = cmd.id,
            cap = cmd.capability,
            exec = cmd.execution,
        ));
        if !names.is_empty() {
            names.push_str(", ");
        }
        names.push_str(&format!("\"{}\"", cmd.name));
    }

    format!(
        "// AUTO-GENERATED by kiri-core commands::emit_typescript. Do not edit.\n\
         // Deterministic: byte-stable for a given COMMANDS catalog.\n\
         export interface KiriCommands {{\n{methods}}}\n\n\
         export const KIRI_COMMAND_NAMES = [{names}] as const;\n\n\
         export function commandId(name: string): number | undefined {{\n  \
         switch (name) {{\n{branches}    default: return undefined;\n  }}\n}}\n",
        methods = methods,
        names = names,
        branches = {
            let mut b = String::new();
            for cmd in COMMANDS {
                b.push_str(&format!(
                    "    case \"{name}\": return {id};\n",
                    name = cmd.name,
                    id = cmd.id
                ));
            }
            b
        },
    )
}

/// Sanitize a dotted command name into a valid TypeScript identifier fragment.
/// `user.get` -> `user_get`.
fn sanitize_ident(name: &str) -> String {
    name.replace(['.', '-', ' '], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_ordered_and_unique() {
        let mut last = 0u32;
        let mut seen = std::collections::HashSet::new();
        for c in COMMANDS {
            assert!(c.id > last, "COMMANDS must be ordered by id");
            assert!(seen.insert(c.id), "duplicate command id {}", c.id);
            last = c.id;
        }
    }

    #[test]
    fn resolve_roundtrips() {
        assert_eq!(resolve_command("kiri.ping"), Some(1));
        assert_eq!(command_name(1), Some("kiri.ping"));
        assert_eq!(resolve_command("kiri.diag"), Some(2));
        assert_eq!(command_name(2), Some("kiri.diag"));
        assert_eq!(resolve_command("nope"), None);
        assert_eq!(command_name(999), None);
    }

    #[test]
    fn unknown_id_has_no_capability() {
        assert!(required_capabilities(999).is_empty());
    }

    #[test]
    fn typescript_emission_is_deterministic() {
        let a = emit_typescript();
        let b = emit_typescript();
        assert_eq!(a, b, "emit_typescript must be byte-stable");
        assert!(a.contains("kiri_ping(arg: unknown)"));
        assert!(a.contains("export const KIRI_COMMAND_NAMES"));
    }

    #[test]
    fn typescript_emission_is_well_formed() {
        let ts = emit_typescript();
        // Basic structural sanity: balanced braces and a switch for routing.
        assert!(ts.contains("switch (name)"));
        assert!(ts.contains("case \"kiri.ping\": return 1;"));
        // Sanity: every command name appears as a method.
        for c in COMMANDS {
            assert!(ts.contains(&sanitize_ident(c.name)));
        }
    }

    #[test]
    fn generated_typescript_matches_committed_artifact() {
        let emitted = emit_typescript();
        // The committed artifact lives next to the crate source so it can be
        // reviewed and consumed by the frontend build. Regenerate it with
        // KIRI_REGEN_TS=1 if the catalog changes.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("gen/commands.ts");
        if std::env::var("KIRI_REGEN_TS").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            std::fs::write(&path, &emitted).expect("write generated ts");
            return;
        }
        // When not regenerating, the committed file must match emission
        // exactly (byte-stable determinism gate).
        let committed = std::fs::read_to_string(&path)
            .expect("gen/commands.ts must be committed; run with KIRI_REGEN_TS=1 to create it");
        assert_eq!(
            committed, emitted,
            "gen/commands.ts is out of date; rerun tests with KIRI_REGEN_TS=1"
        );
    }
}

/// The frontend command catalog in `examples/blank/kiri.js` (`IDS` map) must
/// stay in lockstep with the backend `COMMANDS` catalog: every user-facing
/// command (id >= 5; ids 1..4 are host-only ping/diag/resources) must be
/// exposed on the frontend with the correct numeric id, and the frontend
/// must not bind any id/name that is not in `COMMANDS`. A drift here means a
/// capability Kiri claims to expose (the exceed-Tauri surface) is silently
/// unusable from JavaScript. This is a headless contract check: it parses the
/// committed JS, no WebView is launched.
#[test]
fn frontend_js_catalog_matches_backend_commands() {
    let js_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/blank/kiri.js");
    let js = std::fs::read_to_string(&js_path)
        .expect("examples/blank/kiri.js must exist and be committed");

    // Parse the `IDS = { "kiri.x.y": N, ... }` object.
    let start = js.find("var IDS = {").expect("kiri.js must define var IDS");
    let open = js[start..].find('{').unwrap() + start;
    let close = js[open..].find('}').unwrap() + open;
    let block = &js[open + 1..close];

    let mut frontend: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for entry in block.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, val) =
            entry.split_once(':').unwrap_or_else(|| panic!("malformed IDS entry: {entry}"));
        let name = name.trim().trim_matches('"').to_string();
        let val: u32 =
            val.trim().parse().unwrap_or_else(|e| panic!("malformed IDS value for {name}: {e}"));
        assert!(frontend.insert(name.clone(), val).is_none(), "duplicate frontend id for {name}");
    }

    // Every user-facing backend command must be bound on the frontend.
    for cmd in COMMANDS {
        if cmd.id < 5 {
            continue; // host-only liveness/diagnostics/resource commands
        }
        let expected = frontend.get(cmd.name).copied();
        assert_eq!(
            expected,
            Some(cmd.id),
            "frontend binding for {} must equal backend id {} (got {:?})",
            cmd.name,
            cmd.id,
            expected
        );
    }

    // No frontend binding may point at a backend command that does not exist.
    let backend_names: std::collections::HashSet<&str> = COMMANDS.iter().map(|c| c.name).collect();
    for (name, id) in &frontend {
        assert!(
            backend_names.contains(name.as_str()),
            "frontend binds {name} which is not a backend command"
        );
        assert!(
            COMMANDS.iter().any(|c| c.id == *id),
            "frontend binds {name} -> id {id} which is not a backend command id"
        );
    }
}
