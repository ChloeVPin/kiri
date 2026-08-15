//! Host-owned external-plugin inventory (kiri.plugin.list, G-2 capstone).
//!
//! The only plugin-discovery surface the frontend gets is this inventory: a
//! serializable list of loaded external plugins and the exact commands each is
//! allowed to expose. The descriptors themselves (raw `KiriPluginV1` pointers)
//! never cross the bridge, so a malicious or careless frontend cannot enumerate
//! or reach an unvetted plugin command. This exceeds Tauri's plugin model on
//! the security axis: Tauri exposes plugin commands through the same invoke
//! surface as built-ins, while Kiri's plugin surface is host-owned and
//! discoverable only through an allowlist-shaped inventory.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::capabilities::CapabilityBits;
use crate::dispatch::{capability_bit, command_id, Handler};
use crate::error::Error;

/// One externally-loaded plugin as the host is willing to disclose it: its name
/// and the exact command names it may serve. No descriptor pointer, no library
/// path, no capability internals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub commands: Vec<String>,
}

/// The host-owned inventory returned by `kiri.plugin.list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PluginInventory {
    pub plugins: Vec<PluginInfo>,
}

impl PluginInventory {
    pub fn empty() -> Self {
        PluginInventory { plugins: Vec::new() }
    }

    /// Build the inventory from a host-owned manifest summary: each plugin name
    /// paired with the commands the host has allowlisted for it. This is the
    /// exact projection the runtime discloses; it carries no capability to widen
    /// the surface.
    pub fn from_allowed(entries: &[(String, Vec<String>)]) -> Self {
        let plugins = entries
            .iter()
            .map(|(name, commands)| PluginInfo { name: name.clone(), commands: commands.clone() })
            .collect();
        PluginInventory { plugins }
    }
}

/// Handler factory for `kiri.plugin.list`. The returned handler is
/// capability-gated (PLUGIN) and returns the host-owned inventory verbatim.
pub fn plugin_list_handlers(inventory: PluginInventory) -> Vec<(u32, CapabilityBits, Handler)> {
    let mut required = CapabilityBits::empty();
    required.set(capability_bit::PLUGIN);
    let inv = inventory.clone();
    vec![(
        command_id::PLUGIN_LIST,
        required,
        Arc::new(move |_caller, _request_id, _payload: &serde_json::Value| {
            serde_json::to_value(inv.clone())
                .map_err(|e| Error::internal_error(format!("plugin inventory serialize: {e}")))
        }) as Handler,
    )]
}

#[cfg(test)]
mod tests {
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{capability_bit, command_id, Router};
    use crate::plugin_inventory::PluginInventory;
    use crate::trace::RingTraceSink;
    use crate::wire::WireRequest;
    use serde_json::json;

    #[test]
    fn plugin_list_denied_without_capability() {
        let inventory =
            PluginInventory::from_allowed(&[("foo".to_string(), vec!["kiri.foo.bar".to_string()])]);
        let router = Router::new().with_plugin_inventory(inventory);
        let caller = CallerId(1);
        let req = WireRequest::new(command_id::PLUGIN_LIST, 1, 1, json!(null));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(caller, &CapabilityBits::empty(), &req, &mut sink);
        assert!(resp.error.is_some(), "kiri.plugin.list MUST be denied without PLUGIN capability");
        assert_eq!(resp.error.as_ref().unwrap().code, crate::error::ErrorCode::Unauthorized,);
    }

    #[test]
    fn plugin_list_returns_inventory_with_capability() {
        let inventory = PluginInventory::from_allowed(&[
            ("foo".to_string(), vec!["kiri.foo.bar".to_string()]),
            ("baz".to_string(), vec!["kiri.baz.qux".to_string()]),
        ]);
        let router = Router::new().with_plugin_inventory(inventory);
        let mut caps = CapabilityBits::empty();
        caps.set(capability_bit::PLUGIN);
        let caller = CallerId(1);
        let req = WireRequest::new(command_id::PLUGIN_LIST, 1, 1, json!(null));
        let mut sink = RingTraceSink::new(16);
        let resp = router.dispatch(caller, &caps, &req, &mut sink);
        assert!(
            resp.error.is_none(),
            "kiri.plugin.list MUST succeed with PLUGIN capability: {:?}",
            resp.error
        );
        let value = resp.payload.as_ref().unwrap();
        let plugins = value.get("plugins").unwrap().as_array().unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0]["name"], json!("foo"));
        assert_eq!(plugins[0]["commands"], json!(["kiri.foo.bar"]));
        assert_eq!(plugins[1]["name"], json!("baz"));
    }
}
