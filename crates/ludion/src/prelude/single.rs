//! The singleton lookup: the one entity that carries a component (the run state, the player, the
//! camera).

use std::any::TypeId;

use ludion_core::ids::EntityId;
use ludion_ecs::{Component, PRESENTATION_BASE, World};

/// Singleton lookups on a [`World`].
///
/// Both methods answer with the **first** carrier in entity order (lowest slot index) and do not
/// check for duplicates, so a transient second carrier is ignored rather than a panic.
pub trait SingleExt {
    /// The live sim entity with the lowest slot index carrying `T`; `None` when there is none.
    ///
    /// An absent `T` is O(1). A present `T` walks the entity slots up to the match: the ECS exposes
    /// no index-to-id lookup, so the cost is linear in the match's slot index.
    ///
    /// # Example
    /// ```
    /// use ludion::prelude::*;
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// assert_eq!(world.single::<Score>(), None);
    /// let _other = world.spawn();
    /// let holder = world.spawn();
    /// world.insert(holder, Score(7)).unwrap();
    /// assert_eq!(world.single::<Score>(), Some(holder));
    /// assert_eq!(world.single_get::<Score>().map(|s| s.0), Some(7));
    /// ```
    fn single<T: Component>(&self) -> Option<EntityId>;

    /// The `T` on the entity [`single`](SingleExt::single) finds.
    fn single_get<T: Component>(&self) -> Option<&T>;
}

impl SingleExt for World {
    fn single<T: Component>(&self) -> Option<EntityId> {
        let index = self.entity_indices_with(TypeId::of::<T>()).next()?;
        // Outside the Sim phase the column also lists presentation rows, which `entities()` never yields.
        if index >= PRESENTATION_BASE {
            return None;
        }
        self.entities().find(|e| e.index() == index)
    }

    fn single_get<T: Component>(&self) -> Option<&T> {
        self.single::<T>()
            .and_then(|e| self.get::<T>(e).ok().flatten())
    }
}
