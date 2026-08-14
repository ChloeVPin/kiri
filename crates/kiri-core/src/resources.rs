//! Generational resource table (specs/RESOURCES.md).
//!
//! A 64-bit resource ID packs a 32-bit generation and a 32-bit slot:
//!
//! ```text
//! 63                         32 31                         0
//! +----------------------------+----------------------------+
//! | generation: u32            | slot: u32                  |
//! +----------------------------+----------------------------+
//! ```
//!
//! A valid numeric resource ID is not sufficient authority. Access also
//! checks the current caller identity. Generations increment when a slot is
//! reused; a slot is retired (never reused again) if its generation would
//! wrap, so an ambiguous stale ID can never resolve to a new object.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::caller::CallerId;
use crate::error::Error;

/// Packed generational resource handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(u64);

impl ResourceId {
    pub const fn from_parts(generation: u32, slot: u32) -> Self {
        ResourceId(((generation as u64) << 32) | (slot as u64))
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn slot(self) -> u32 {
        self.0 as u32
    }

    pub const fn into_raw(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> Self {
        ResourceId(raw)
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "g{}s{}", self.generation(), self.slot())
    }
}

#[derive(Debug)]
struct Entry<T> {
    generation: u32,
    owner: CallerId,
    value: Option<T>,
}

/// A generational resource table for resources of type `T`.
///
/// Each table instance holds resources of exactly one `T`; the resource type
/// check in the specification is therefore structural: a caller must request
/// a resource from the table for its declared type, and `get` returns the
/// concrete value type rather than a raw pointer.
#[derive(Debug)]
pub struct ResourceTable<T> {
    slots: Vec<Option<Entry<T>>>,
    free: Vec<u32>,
    /// Maps caller -> number of open resources (per-caller quota).
    per_caller: HashMap<CallerId, u32>,
    /// Retired slots never get reused after a generation wrap.
    retired: Vec<u32>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for ResourceTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ResourceTable<T> {
    pub fn new() -> Self {
        ResourceTable {
            slots: Vec::new(),
            free: Vec::new(),
            per_caller: HashMap::new(),
            retired: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Insert a resource owned by `caller`. Enforces the per-caller and total
    /// open-resource limits.
    pub fn insert(
        &mut self,
        caller: CallerId,
        value: T,
        max_open: u32,
    ) -> Result<ResourceId, Error> {
        let open = *self.per_caller.get(&caller).unwrap_or(&0);
        if open >= max_open {
            return Err(Error::limit_exceeded(format!(
                "caller {caller:?} open resource limit {max_open} reached"
            )));
        }

        let slot = match self.free.pop() {
            Some(slot) => {
                let entry = self
                    .slots
                    .get_mut(slot as usize)
                    .expect("free list references an existing slot");
                let next = entry
                    .as_ref()
                    .expect("free list references a tombstone slot")
                    .generation
                    .wrapping_add(1);
                if next == 0 {
                    // Generation wrap: retire the slot rather than produce an
                    // ambiguous stale ID (specs/RESOURCES.md lifecycle).
                    self.free.push(slot);
                    self.retired.push(slot);
                    return Err(Error::internal_error(
                        "resource slot generation wrapped; slot retired",
                    ));
                }
                entry.as_mut().unwrap().generation = next;
                entry.as_mut().unwrap().owner = caller;
                entry.as_mut().unwrap().value = Some(value);
                slot
            }
            None => {
                let slot = self.slots.len() as u32;
                self.slots.push(Some(Entry { generation: 1, owner: caller, value: Some(value) }));
                slot
            }
        };

        *self.per_caller.entry(caller).or_insert(0) += 1;
        Ok(ResourceId::from_parts(self.slots[slot as usize].as_ref().unwrap().generation, slot))
    }

    /// Fetch a resource with generation, owner, and slot validation.
    pub fn get(&self, caller: CallerId, id: ResourceId) -> Result<&T, Error> {
        let entry = self.lookup(caller, id)?;
        Ok(entry.value.as_ref().expect("validated slot has a value"))
    }

    /// Fetch a resource mutably with the same validation.
    pub fn get_mut(&mut self, caller: CallerId, id: ResourceId) -> Result<&mut T, Error> {
        let entry = self.lookup_mut(caller, id)?;
        Ok(entry.value.as_mut().expect("validated slot has a value"))
    }

    /// Remove and drop a resource. The slot becomes a tombstone that keeps
    /// its generation counter for the next allocation.
    pub fn remove(&mut self, caller: CallerId, id: ResourceId) -> Result<T, Error> {
        self.lookup(caller, id)?;
        let slot = id.slot() as usize;
        let entry = self.slots[slot].as_mut().expect("validated entry");
        let value = entry.value.take().expect("validated slot has a value");
        self.free.push(id.slot());
        self.decrement(caller);
        Ok(value)
    }

    /// Revoke every resource owned by `caller`. Returns the number revoked.
    pub fn revoke_all_for(&mut self, caller: CallerId) -> usize {
        let mut revoked = 0usize;
        for slot in &mut self.slots {
            if let Some(entry) = slot.as_mut() {
                if entry.owner == caller && entry.value.is_some() {
                    entry.value = None;
                    revoked += 1;
                }
            }
        }
        self.free = (0..self.slots.len())
            .filter(|i| {
                self.slots[*i].as_ref().is_some_and(|e| e.value.is_none())
                    && !self.retired.contains(&(*i as u32))
            })
            .map(|i| i as u32)
            .collect();
        self.per_caller.remove(&caller);
        revoked
    }

    /// Number of currently open resources.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.as_ref().is_some_and(|e| e.value.is_some())).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of resources owned by `caller`.
    pub fn count_for(&self, caller: CallerId) -> u32 {
        self.per_caller.get(&caller).copied().unwrap_or(0)
    }
    fn lookup(&self, caller: CallerId, id: ResourceId) -> Result<&Entry<T>, Error> {
        let slot = id.slot() as usize;
        let entry = match self.slots.get(slot) {
            Some(Some(entry)) if entry.generation == id.generation() => entry,
            Some(Some(_)) => {
                return Err(Error::resource_stale(format!("resource {id} generation mismatch")))
            }
            Some(None) | None => {
                return Err(Error::resource_not_found(format!("resource {id} slot empty")))
            }
        };
        if entry.value.is_none() {
            // Tombstone: the resource was removed; the handle is stale.
            return Err(Error::resource_not_found(format!("resource {id} was removed")));
        }
        if entry.owner != caller {
            return Err(Error::unauthorized(format!("resource {id} owned by different caller")));
        }
        Ok(entry)
    }

    fn lookup_mut(&mut self, caller: CallerId, id: ResourceId) -> Result<&mut Entry<T>, Error> {
        let slot = id.slot() as usize;
        let entry = match self.slots.get_mut(slot) {
            Some(Some(entry)) if entry.generation == id.generation() => entry,
            Some(Some(_)) => {
                return Err(Error::resource_stale(format!("resource {id} generation mismatch")))
            }
            Some(None) | None => {
                return Err(Error::resource_not_found(format!("resource {id} slot empty")))
            }
        };
        if entry.value.is_none() {
            // Tombstone: the resource was removed; the handle is stale.
            return Err(Error::resource_not_found(format!("resource {id} was removed")));
        }
        if entry.owner != caller {
            return Err(Error::unauthorized(format!("resource {id} owned by different caller")));
        }
        Ok(entry)
    }

    fn decrement(&mut self, caller: CallerId) {
        if let Some(count) = self.per_caller.get_mut(&caller) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_caller.remove(&caller);
            }
        }
    }
}

/// Drop resource table entries on table drop.
impl<T> Drop for ResourceTable<T> {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn caller_registry_pair() -> (crate::caller::CallerRegistry, CallerId) {
        let mut reg = crate::caller::CallerRegistry::new();
        let id = reg.register();
        (reg, id)
    }

    #[test]
    fn insert_get_roundtrip() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let id = table.insert(caller, String::from("hello"), 4096).unwrap();
        assert_eq!(table.get(caller, id).unwrap(), "hello");
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn stale_handle_rejected() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let first = table.insert(caller, 1u64, 4096).unwrap();
        table.remove(caller, first).unwrap();
        let second = table.insert(caller, 2u64, 4096).unwrap();
        // Same slot, new generation: the old handle must not resolve.
        assert_eq!(first.slot(), second.slot());
        assert_ne!(first.generation(), second.generation());
        let err = table.get(caller, first).unwrap_err();
        assert_eq!(err.code, ErrorCode::ResourceStale);
        assert_eq!(*table.get(caller, second).unwrap(), 2u64);
    }

    #[test]
    fn wrong_owner_rejected() {
        let mut reg = crate::caller::CallerRegistry::new();
        let alice = reg.register();
        let bob = reg.register();
        let mut table = ResourceTable::new();
        let id = table.insert(alice, String::from("alice's file"), 4096).unwrap();
        let err = table.get(bob, id).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(table.get(alice, id).unwrap(), "alice's file");
    }

    #[test]
    fn remove_returns_value_and_releases_slot() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let id = table.insert(caller, 42u64, 4096).unwrap();
        assert_eq!(table.remove(caller, id).unwrap(), 42u64);
        assert!(table.is_empty());
        assert_eq!(table.count_for(caller), 0);
        let err = table.get(caller, id).unwrap_err();
        assert_eq!(err.code, ErrorCode::ResourceNotFound);
    }

    #[test]
    fn resource_count_returns_to_zero_after_close_all() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let ids: Vec<_> = (0..10).map(|i| table.insert(caller, i, 4096).unwrap()).collect();
        assert_eq!(table.len(), 10);
        for id in ids {
            table.remove(caller, id).unwrap();
        }
        assert_eq!(table.len(), 0);
        assert_eq!(table.count_for(caller), 0);
    }

    #[test]
    fn revoke_all_for_caller() {
        let mut reg = crate::caller::CallerRegistry::new();
        let alice = reg.register();
        let bob = reg.register();
        let mut table = ResourceTable::new();
        let a1 = table.insert(alice, 1u64, 4096).unwrap();
        let _b1 = table.insert(bob, 2u64, 4096).unwrap();
        assert_eq!(table.revoke_all_for(alice), 1);
        assert!(!table.is_empty());
        assert!(table.get(alice, a1).is_err());
        assert_eq!(table.count_for(alice), 0);
        assert_eq!(table.count_for(bob), 1);
    }

    #[test]
    fn per_caller_quota_enforced() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let _a = table.insert(caller, 1u64, 2).unwrap();
        let _b = table.insert(caller, 2u64, 2).unwrap();
        let err = table.insert(caller, 3u64, 2).unwrap_err();
        assert_eq!(err.code, ErrorCode::LimitExceeded);
    }

    #[test]
    fn slot_reuse_uses_fresh_generation() {
        let (_reg, caller) = caller_registry_pair();
        let mut table = ResourceTable::new();
        let a = table.insert(caller, 1u64, 4096).unwrap();
        table.remove(caller, a).unwrap();
        let b = table.insert(caller, 2u64, 4096).unwrap();
        assert_eq!(a.slot(), b.slot());
        assert_eq!(b.generation(), a.generation() + 1);
    }

    #[test]
    fn id_packing_roundtrip() {
        let id = ResourceId::from_parts(0xDEAD_BEEF, 0x1234_5678);
        assert_eq!(id.generation(), 0xDEAD_BEEF);
        assert_eq!(id.slot(), 0x1234_5678);
        assert_eq!(ResourceId::from_raw(id.into_raw()), id);
    }
}
