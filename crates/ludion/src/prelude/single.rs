//! The singleton lookup: the one entity that carries a component (the run state, the player, the
//! camera).

use std::any::TypeId;

use ludion_core::ids::EntityId;
use ludion_ecs::{Component, World};

/// Singleton lookups on a [`World`].
///
/// Every method answers with the **first** carrier in entity order (lowest slot index) and does not
/// check for duplicates, so a transient second carrier is ignored rather than a panic.
pub trait SingleExt {
    /// The live sim entity with the lowest slot index carrying `T`; `None` when there is none.
    ///
    /// O(1).
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
    /// if let Some(score) = world.single_get_mut::<Score>() {
    ///     score.0 += 1;
    /// }
    /// assert_eq!(world.single_get::<Score>().map(|s| s.0), Some(8));
    /// ```
    fn single<T: Component>(&self) -> Option<EntityId>;

    /// The `T` on the entity [`single`](SingleExt::single) finds.
    fn single_get<T: Component>(&self) -> Option<&T>;

    /// The `T` on the entity [`single`](SingleExt::single) finds, for writing. In a presentation
    /// system `single` finds a *sim* entity, so this records an `L0028` and returns `None`: a
    /// presentation system reads sim state and writes its own.
    fn single_get_mut<T: Component>(&mut self) -> Option<&mut T>;
}

impl SingleExt for World {
    fn single<T: Component>(&self) -> Option<EntityId> {
        let index = self.entity_indices_with(TypeId::of::<T>()).next()?;
        // Outside the Sim phase the column also lists presentation rows; `live_id_at` answers `None` for them.
        self.live_id_at(index)
    }

    fn single_get<T: Component>(&self) -> Option<&T> {
        self.single::<T>().and_then(|e| self.component::<T>(e))
    }

    fn single_get_mut<T: Component>(&mut self) -> Option<&mut T> {
        let e = self.single::<T>()?;
        self.component_mut::<T>(e)
    }
}
