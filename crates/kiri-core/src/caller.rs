//! Native caller identity (specs/SECURITY.md).
//!
//! The native runtime assigns identity to each WebView or window. JavaScript
//! never supplies the authoritative caller identity. The only way to obtain a
//! `CallerId` is through the registry, which platform transports call when a
//! native caller is created.

use std::collections::HashMap;

/// Stable identity of one native caller (a WebView or window).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallerId(pub u64);

/// Assigns and tracks native caller identities.
#[derive(Debug, Default)]
pub struct CallerRegistry {
    next_id: u64,
    live: HashMap<CallerId, usize>,
}

impl CallerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a newly created native caller and return its authoritative
    /// identity. Never called with data derived from JavaScript.
    pub fn register(&mut self) -> CallerId {
        let id = CallerId(self.next_id);
        self.next_id += 1;
        self.live.insert(id, 0);
        id
    }

    /// Remove a caller. Its resources must be revoked separately through the
    /// resource table's `revoke_all_for`.
    pub fn unregister(&mut self, id: CallerId) {
        self.live.remove(&id);
    }

    pub fn is_live(&self, id: CallerId) -> bool {
        self.live.contains_key(&id)
    }

    pub fn live_count(&self) -> usize {
        self.live.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_assigns_distinct_ids() {
        let mut reg = CallerRegistry::new();
        let a = reg.register();
        let b = reg.register();
        assert_ne!(a, b);
        assert_eq!(reg.live_count(), 2);
    }

    #[test]
    fn unregister_removes_identity() {
        let mut reg = CallerRegistry::new();
        let a = reg.register();
        reg.unregister(a);
        assert!(!reg.is_live(a));
        assert_eq!(reg.live_count(), 0);
    }
}
