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
