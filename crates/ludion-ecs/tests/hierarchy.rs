//! The derived child map and the cascading despawn: order, free-list reuse, `despawn_many`
//! equivalence, hostile `Parent` data, and the presentation range rules.

use std::sync::Arc;

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{ChildMap, Parent, Schedule, SystemCtx, World};

fn ctx() -> SystemCtx {
    SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
}

fn spawn_n(world: &mut World, n: usize) -> Vec<EntityId> {
    (0..n).map(|_| world.spawn()).collect()
}

fn link(world: &mut World, child: EntityId, parent: EntityId) {
    world.insert(child, Parent(parent)).unwrap();
}

/// The next `n` spawned ids, in order.
fn next_spawns(world: &mut World, n: usize) -> Vec<EntityId> {
    spawn_n(world, n)
}

#[test]
fn cascade_order_and_free_list_reuse() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 8);
    // 3 ─┬─ 6 ── 5      1 under 3; 0, 2, 4, 7 unrelated
    //    └─ 1
    link(&mut world, ids[6], ids[3]);
    link(&mut world, ids[1], ids[3]);
    link(&mut world, ids[5], ids[6]);

    world.despawn(ids[3]).unwrap();

    for dead in [3, 1, 5, 6] {
        assert!(!world.is_alive(ids[dead]), "slot {dead} should be dead");
    }
    for alive in [0, 2, 4, 7] {
        assert!(world.is_alive(ids[alive]), "slot {alive} should survive");
    }
    // Freed 3, 1, 5, 6: the free list is LIFO, so the highest descendant slot is reused first.
    let reused: Vec<u32> = next_spawns(&mut world, 4)
        .iter()
        .map(|id| id.index)
        .collect();
    assert_eq!(reused, vec![6, 5, 1, 3]);
}

#[test]
fn cascade_removes_every_component_of_the_doomed() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 3);
    link(&mut world, ids[1], ids[0]);
    world.insert(ids[1], 7u32).unwrap();
    world.insert(ids[2], 9u32).unwrap();
    world.despawn(ids[0]).unwrap();
    assert_eq!(world.entities().collect::<Vec<_>>(), vec![ids[2]]);
    assert_eq!(world.get::<u32>(ids[2]).unwrap(), Some(&9));
}

/// Two identical worlds over a random forest (cycles and self-parents included).
fn forest(seed: u64) -> (World, Vec<EntityId>, DeterministicRng) {
    let mut rng = DeterministicRng::new(seed);
    let mut world = World::new();
    let ids = spawn_n(&mut world, 24);
    for i in 0..ids.len() {
        if rng.gen_range_u32(0..10) < 6 {
            let parent = rng.gen_range_usize(0..ids.len());
            link(&mut world, ids[i], ids[parent]);
        }
    }
    (world, ids, rng)
}

#[test]
fn despawn_many_matches_a_despawn_loop_over_random_forests() {
    for seed in 0..64u64 {
        let (mut many, ids, mut rng) = forest(seed);
        let (mut looped, _, _) = forest(seed);
        // Repeats, ancestors and descendants in either order, all fall out of random picks.
        let doomed: Vec<EntityId> = (0..12)
            .map(|_| ids[rng.gen_range_usize(0..ids.len())])
            .collect();

        let many_codes: Vec<_> = many
            .despawn_many(&doomed)
            .iter()
            .map(|d| d.code.to_string())
            .collect();
        let loop_codes: Vec<_> = doomed
            .iter()
            .filter_map(|&id| looped.despawn(id).err())
            .map(|d| d.code.to_string())
            .collect();

        assert_eq!(many_codes, loop_codes, "seed {seed}: diagnostics");
        assert_eq!(
            many.entities().collect::<Vec<_>>(),
            looped.entities().collect::<Vec<_>>(),
            "seed {seed}: survivors"
        );
        assert_eq!(
            next_spawns(&mut many, 30),
            next_spawns(&mut looped, 30),
            "seed {seed}: free-list order"
        );
    }
}

#[test]
fn a_descendant_listed_after_its_ancestor_reports_l0002_once() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 3);
    link(&mut world, ids[1], ids[0]);
    let errors = world.despawn_many(&[ids[0], ids[1], ids[2]]);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.to_string(), "L0002");
    assert_eq!(world.entities().count(), 0);

    // The other order: no error, the child is already gone when the parent cascades.
    let mut world = World::new();
    let ids = spawn_n(&mut world, 3);
    link(&mut world, ids[1], ids[0]);
    assert!(world.despawn_many(&[ids[1], ids[0], ids[2]]).is_empty());
}

#[test]
fn a_stale_parent_handle_is_not_cascaded() {
    let mut world = World::new();
    let old = world.spawn();
    let child = world.spawn();
    link(&mut world, child, old);
    world.despawn(old).unwrap(); // cascades: child dies too
    assert!(!world.is_alive(child));

    // A raw insert of a dead id parents nothing to the slot's new occupant.
    let mut world = World::new();
    let old = world.spawn();
    world.despawn(old).unwrap();
    let occupant = world.spawn();
    assert_eq!(occupant.index, old.index);
    let orphan = world.spawn();
    link(&mut world, orphan, old);
    assert!(ChildMap::build(&world).children(occupant).is_empty());
    world.despawn(occupant).unwrap();
    assert!(world.is_alive(orphan));
}

#[test]
fn cycles_and_self_parents_terminate() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 3);
    link(&mut world, ids[0], ids[1]);
    link(&mut world, ids[1], ids[0]);
    link(&mut world, ids[2], ids[2]);
    let map = ChildMap::build(&world);
    assert_eq!(map.descendants(ids[0]), vec![ids[1]]);
    assert!(
        map.descendants(ids[2]).is_empty(),
        "the root is never re-collected"
    );
    world.despawn(ids[0]).unwrap();
    assert!(!world.is_alive(ids[0]) && !world.is_alive(ids[1]));
    world.despawn(ids[2]).unwrap();
    assert_eq!(world.entities().count(), 0);
}

#[test]
fn child_map_orders_children_and_descendants_ascending() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 6);
    // Parent 4 ─ children 5, 1, 3 (linked out of order); 2 under 5; 0 unrelated.
    link(&mut world, ids[5], ids[4]);
    link(&mut world, ids[1], ids[4]);
    link(&mut world, ids[3], ids[4]);
    link(&mut world, ids[2], ids[5]);
    let map = ChildMap::build(&world);
    assert_eq!(map.children(ids[4]), &[ids[1], ids[3], ids[5]]);
    assert_eq!(
        map.descendants(ids[4]),
        vec![ids[1], ids[2], ids[3], ids[5]]
    );
    assert!(map.children(ids[0]).is_empty());
}

#[test]
fn a_dangling_parent_is_unreachable_from_a_live_root() {
    let mut world = World::new();
    let ids = spawn_n(&mut world, 3);
    let dead = ids[2];
    world.despawn(dead).unwrap();
    link(&mut world, ids[1], dead);
    let map = ChildMap::build(&world);
    assert!(map.descendants(ids[0]).is_empty());
    assert_eq!(
        map.children(dead),
        &[ids[1]],
        "the entry exists but names no live root"
    );
}

#[test]
fn a_sim_cascade_takes_presentation_descendants_in_sim_phase() {
    let mut world = World::new();
    let sim = world.spawn();
    let other = world.spawn();
    let pres = world.spawn_presentation();
    let grandchild = world.spawn_presentation();
    link(&mut world, pres, sim);
    link(&mut world, grandchild, pres);

    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "despawn",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            world.despawn(sim).unwrap();
        }),
    );
    schedule.run(&mut world, &ctx());

    assert!(!world.is_alive(pres) && !world.is_alive(grandchild));
    assert!(world.is_alive(other));
}

#[test]
fn a_presentation_despawn_leaves_a_sim_child_alive() {
    let mut world = World::new();
    let pres = world.spawn_presentation();
    let sim_child = world.spawn();
    let pres_child = world.spawn_presentation();
    link(&mut world, sim_child, pres);
    link(&mut world, pres_child, pres);

    let mut schedule = Schedule::new();
    schedule.add_presentation_system_shared(
        "despawn",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            world.despawn(pres).unwrap();
        }),
    );
    schedule.run(&mut world, &ctx());

    assert!(!world.is_alive(pres) && !world.is_alive(pres_child));
    assert!(
        world.is_alive(sim_child),
        "a sim id never moves for a presentation despawn"
    );
    // The sim allocator is untouched: the next sim spawn is a fresh slot, not sim_child's.
    assert_ne!(world.spawn().index, sim_child.index);
}

#[test]
fn child_map_in_sim_phase_hides_presentation_children() {
    struct Seen(Vec<EntityId>);

    let mut world = World::new();
    let sim = world.spawn();
    let sim_child = world.spawn();
    let pres_child = world.spawn_presentation();
    link(&mut world, sim_child, sim);
    link(&mut world, pres_child, sim);
    world.insert_resource(Seen(Vec::new()));

    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "probe",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            let seen = ChildMap::build(world).children(sim).to_vec();
            world.resource_mut::<Seen>().unwrap().0 = seen;
        }),
    );
    schedule.run(&mut world, &ctx());

    assert_eq!(world.resource::<Seen>().unwrap().0, vec![sim_child]);
    assert_eq!(
        ChildMap::build(&world).children(sim),
        &[sim_child, pres_child]
    );
}
