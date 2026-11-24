//! [`Group`] and [`GroupMember`]: squads as plain components.

use ludion_core::ids::EntityId;
use ludion_ecs::{EntityRefCollection, EntityRefSchema, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A squad's shared entity. Carries no state of its own beyond a free-form `kind` label a game
/// can read (`"patrol"`, `"pack"`); members point at it with [`GroupMember`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    /// A game-defined label.
    #[serde(default)]
    pub kind: String,
}

/// An agent's membership of a [`Group`]. Despawning the member removes it from the group: it is
/// the component that is scanned, so there is no member list to go stale.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GroupMember {
    /// The group entity; `None` is a member of nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<EntityRefSchema>")]
    pub group: Option<EntityId>,
    /// Orders members within the group: lower first, ties by entity. A game's own meaning (leader
    /// `0`, flankers `1`…).
    #[serde(default)]
    pub role: u8,
}

/// Opt-in membership memory for a [`Group`]: with it, each tick's departures reach the remaining
/// members through [`Machine::member_left`](crate::Machine::member_left). A group without it reports nothing. All fields are
/// runtime state; author an empty `GroupRoster: {}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GroupRoster {
    /// The membership as of `seen`, in [`members`] order. A member despawned across a save or
    /// carry reads `null`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "EntityRefCollection<Vec<Option<EntityRefSchema>>>")]
    pub members: Vec<Option<EntityId>>,
    /// The departures found at `seen`, in the order of the previous `members`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "EntityRefCollection<Vec<Option<EntityRefSchema>>>")]
    pub left: Vec<Option<EntityId>>,
    /// The gameplay tick of the last refresh.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seen: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Brings every [`GroupRoster`] up to the tick: departures are the previous members no longer in the
/// group (a `None` entry counts as departed). The `seen` stamp makes a second machine's call in the
/// same tick reuse the result.
pub(crate) fn refresh_rosters(world: &mut World, now: u64) {
    let groups: Vec<EntityId> = world
        .query_ids::<(&Group, &GroupRoster)>()
        .filter(|(_, (_, r))| r.seen != now)
        .map(|(id, _)| id)
        .collect();
    for group in groups {
        let current = members(world, group);
        let Some(roster) = world.component_mut::<GroupRoster>(group) else {
            continue;
        };
        roster.left = roster
            .members
            .iter()
            .filter(|m| m.is_none_or(|id| !current.contains(&id)))
            .copied()
            .collect();
        roster.members = current.into_iter().map(Some).collect();
        roster.seen = now;
    }
}

/// The groups whose roster was refreshed at `now` and saw departures, with the live members and the
/// departures, in ascending group id.
pub(crate) fn departures(
    world: &mut World,
    now: u64,
) -> Vec<(Vec<EntityId>, Vec<Option<EntityId>>)> {
    world
        .query_ids::<(&Group, &GroupRoster)>()
        .filter(|(_, (_, r))| r.seen == now && !r.left.is_empty())
        .map(|(_, (_, r))| {
            (
                r.members.iter().flatten().copied().collect(),
                r.left.clone(),
            )
        })
        .collect()
}

/// The members of `group` that are alive, sorted by `(role, entity)`.
///
/// # Example
///
/// ```
/// use ludion_ai::{GroupMember, members};
/// use ludion_ecs::World;
///
/// let mut world = World::new();
/// let group = world.spawn();
/// let (a, b) = (world.spawn(), world.spawn());
/// world.insert(a, GroupMember { group: Some(group), role: 1 }).unwrap();
/// world.insert(b, GroupMember { group: Some(group), role: 0 }).unwrap();
/// assert_eq!(members(&mut world, group), vec![b, a]);
/// ```
pub fn members(world: &mut World, group: EntityId) -> Vec<EntityId> {
    let mut found: Vec<(u8, EntityId)> = world
        .query_ids::<(&GroupMember,)>()
        .filter(|(_, (m,))| m.group == Some(group))
        .map(|(id, (m,))| (m.role, id))
        .collect();
    found.sort();
    found.into_iter().map(|(_, id)| id).collect()
}
