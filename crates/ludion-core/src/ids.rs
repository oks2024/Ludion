//! Entity identity.
//!
//! [`EntityId`] is a plain generational index: a slot [`index`](EntityId::index) plus a
//! [`generation`](EntityId::generation) counter that is bumped every time the slot is recycled.
//! This lets the ECS detect stale handles (a reference to a despawned entity whose slot has since
//! been reused) instead of silently hitting a different entity.
//!
//! This module is deliberately allocator-free: it defines the *value type* only. The free-list and
//! generation-bumping logic live in `ludion-ecs` (Phase 1). Keeping the id a dumb, `Copy` data type
//! means it can travel through the wire protocol, scenes, and snapshots without dragging ECS
//! internals along.

use serde::{Deserialize, Serialize};

/// A generational entity handle: a slot index plus the generation that owned it.
///
/// Two ids with the same `index` but different `generation` refer to different entities — the
/// second reused the slot after the first was despawned. Ordering is by `index` then `generation`,
/// which gives the deterministic, defined ordering the sim relies on.
///
/// # Example
///
/// ```
/// use ludion_core::ids::EntityId;
///
/// let a = EntityId::new(7, 0);
/// let b = EntityId::new(7, 1); // same slot, later generation
/// assert_ne!(a, b);
/// assert!(a < b);
/// assert_eq!(b.to_string(), "e7v1");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct EntityId {
    /// Storage slot the entity occupies.
    pub index: u32,
    /// How many times this slot has been (re)allocated; distinguishes recycled slots.
    pub generation: u32,
}

impl EntityId {
    /// Constructs an id from an explicit slot index and generation.
    ///
    /// ```
    /// use ludion_core::ids::EntityId;
    /// let id = EntityId::new(3, 0);
    /// assert_eq!(id.index(), 3);
    /// assert_eq!(id.generation(), 0);
    /// ```
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The storage slot this id occupies.
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation counter distinguishing this id from earlier users of the same slot.
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

impl std::fmt::Display for EntityId {
    /// Renders as `e{index}v{generation}`, e.g. `e42v3` — compact and greppable in logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "e{}v{}", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_readable() {
        assert_eq!(EntityId::new(42, 3).to_string(), "e42v3");
        assert_eq!(EntityId::new(0, 0).to_string(), "e0v0");
    }

    #[test]
    fn ordering_is_index_then_generation() {
        assert!(EntityId::new(1, 5) < EntityId::new(2, 0));
        assert!(EntityId::new(2, 0) < EntityId::new(2, 1));
    }

    #[test]
    fn serde_round_trips() {
        let id = EntityId::new(7, 2);
        let json = serde_json::to_string(&id).unwrap();
        let back: EntityId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
