//! Entity hierarchy.
//!
//! The hierarchy is exactly one plain component: [`Parent`]. There are no relation graphs and no
//! stored child lists — a parent link is data like any other and is registered with the
//! [`Registry`](crate::Registry) like any other component. The inverse direction is derived on
//! demand: [`ChildMap`] is one pass over the `Parent` column, never stored. [`World::despawn`] and
//! [`World::despawn_many`] cascade through it (the named entity, then its descendants in ascending
//! slot index).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use ludion_core::ids::EntityId;
use serde::{Deserialize, Serialize};

use crate::World;
use crate::entity::is_presentation;

/// A link to this entity's parent in the hierarchy.
///
/// A newtype over [`EntityId`]; it serializes transparently as the parent's id (`{index,
/// generation}`).
///
/// # Example
/// ```
/// use ludion_ecs::{Parent, World};
///
/// let mut world = World::new();
/// let parent = world.spawn();
/// let child = world.spawn();
/// world.insert(child, Parent(parent)).unwrap();
/// assert_eq!(world.get::<Parent>(child).unwrap().unwrap().0, parent);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent(pub EntityId);

// `EntityId` lives in `ludion-core`, which has no `schemars` dependency, so `Parent` cannot derive
// its schema: it reuses the entity-reference schema.
impl schemars::JsonSchema for Parent {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Parent")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        crate::entity_ref::hierarchy_ref_schema(
            "A link to this entity's parent: the parent's generational EntityId.",
        )
    }
}

/// Parent → children, derived in one pass over the `Parent` column; never stored.
///
/// Parents match by full [`EntityId`] (index and generation): a `Parent` holding a stale handle
/// is not a child of the slot's new occupant. Children and descendants come out in ascending slot
/// index.
///
/// # Example
/// ```
/// use ludion_ecs::{ChildMap, Parent, World};
///
/// let mut world = World::new();
/// let root = world.spawn();
/// let child = world.spawn();
/// let grandchild = world.spawn();
/// world.insert(child, Parent(root)).unwrap();
/// world.insert(grandchild, Parent(child)).unwrap();
///
/// let map = ChildMap::build(&world);
/// assert_eq!(map.children(root), &[child]);
/// assert_eq!(map.descendants(root), vec![child, grandchild]);
/// ```
pub struct ChildMap {
    children: BTreeMap<EntityId, Vec<EntityId>>,
}

impl ChildMap {
    /// Builds the map over the rows the current phase may read: in `Phase::Sim` a presentation
    /// child is invisible, like it is to a query.
    pub fn build(world: &World) -> ChildMap {
        Self::from_links(world.parent_links(world.query_window(false)))
    }

    /// Builds the map over the whole column, whatever the phase — the cascade's view.
    pub fn build_full(world: &World) -> ChildMap {
        Self::from_links(world.parent_links(0..u32::MAX))
    }

    fn from_links(links: Vec<(EntityId, EntityId)>) -> ChildMap {
        let mut children: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
        for (child, parent) in links {
            children.entry(parent).or_default().push(child);
        }
        ChildMap { children }
    }

    /// The direct children of `parent`, ascending by slot index; empty if it has none.
    pub fn children(&self, parent: EntityId) -> &[EntityId] {
        self.children.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// Every descendant of `root`, excluding `root`, ascending by slot index. A cycle terminates
    /// and never lists `root`. Below a presentation entity, sim children (and their subtrees) are
    /// skipped: a presentation despawn must never move a sim id.
    pub fn descendants(&self, root: EntityId) -> Vec<EntityId> {
        let mut seen = BTreeSet::from([root]);
        let mut stack = vec![root];
        let mut found = Vec::new();
        while let Some(node) = stack.pop() {
            for &child in self.children(node) {
                if is_presentation(node) && !is_presentation(child) {
                    continue;
                }
                if seen.insert(child) {
                    found.push(child);
                    stack.push(child);
                }
            }
        }
        found.sort_by_key(|id| id.index);
        found
    }
}
