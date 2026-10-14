//! `SingleExt` finds exactly what the hand-rolled `entities().find(|&e| has::<T>(e))` did.

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
struct Tag(u32);

fn oracle(world: &World) -> Option<EntityId> {
    world.entities().find(|&e| world.has::<Tag>(e))
}

/// `single` and `single_get` against the old expression; returns what they found.
fn check(world: &World) -> Option<EntityId> {
    let found = world.single::<Tag>();
    assert_eq!(found, oracle(world));
    let expected = found.and_then(|e| world.get::<Tag>(e).ok().flatten());
    assert_eq!(world.single_get::<Tag>(), expected);
    found
}

#[test]
fn no_carrier_ever_stored() {
    let mut world = World::new();
    world.spawn();
    assert_eq!(check(&world), None);
}

#[test]
fn one_carrier() {
    let mut world = World::new();
    world.spawn();
    let b = world.spawn();
    world.insert(b, Tag(1)).unwrap();
    assert_eq!(check(&world), Some(b));
}

#[test]
fn two_carriers_the_lower_slot_wins() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    world.insert(b, Tag(2)).unwrap();
    world.insert(a, Tag(1)).unwrap();
    assert_eq!(check(&world), Some(a));
    assert_eq!(world.single_get::<Tag>(), Some(&Tag(1)));
}

#[test]
fn index_order_not_spawn_order() {
    let mut world = World::new();
    let a = world.spawn();
    let _b = world.spawn();
    let c = world.spawn();
    world.despawn(a).unwrap();
    let d = world.spawn();
    assert_eq!(d.index(), a.index(), "the freed slot is reused");
    world.insert(c, Tag(3)).unwrap();
    world.insert(d, Tag(4)).unwrap();
    assert_eq!(check(&world), Some(d));
}

#[test]
fn presentation_entities_are_not_singletons() {
    let mut world = World::new();
    let p = world.spawn_presentation();
    world.insert(p, Tag(5)).unwrap();
    assert_eq!(check(&world), None);
}

#[test]
fn inserted_then_removed() {
    let mut world = World::new();
    let a = world.spawn();
    world.insert(a, Tag(6)).unwrap();
    world.remove::<Tag>(a).unwrap();
    assert_eq!(check(&world), None);
}
