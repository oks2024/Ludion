//! ECS conformance suite (architecture doc §4: written to define the contract).
//!
//! Every determinism and safety guarantee the rest of the engine relies on is pinned here with an
//! exact-match assertion: generational safety, the LIFO free-list order, ascending-index query
//! order, join semantics, registry round-trip and
//! registration order, schedule order, and resources.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{MAX_ENTITY_SLOTS, Parent, Registry, Schedule, SystemCtx, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- Test components -------------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Velocity {
    dx: i32,
    dy: i32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Health(u32);

#[derive(Serialize, Deserialize, JsonSchema, Clone, PartialEq, Debug)]
struct Name(String);

// --- 1. Generational safety ------------------------------------------------------------------

#[test]
fn stale_handle_is_an_error_not_a_silent_hit() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 1, y: 2 }).unwrap();
    assert!(world.is_alive(e));

    world.despawn(e).unwrap();
    assert!(!world.is_alive(e));

    // get / get_mut / insert / remove / despawn all reject the stale handle with L0002.
    let err = world.get::<Position>(e).unwrap_err();
    assert_eq!(err.code, code::DEAD_ENTITY);
    assert!(world.get_mut::<Position>(e).is_err());
    assert_eq!(
        world.insert(e, Position { x: 9, y: 9 }).unwrap_err().code,
        code::DEAD_ENTITY
    );
    assert!(world.remove::<Position>(e).is_err());
    assert_eq!(world.despawn(e).unwrap_err().code, code::DEAD_ENTITY);

    // has() is a total predicate: false for a dead handle, never an error.
    assert!(!world.has::<Position>(e));
}

#[test]
fn respawned_slot_has_a_bumped_generation() {
    let mut world = World::new();
    let a = world.spawn();
    assert_eq!(a, EntityId::new(0, 0));
    world.despawn(a).unwrap();

    let reused = world.spawn();
    assert_eq!(reused.index(), a.index(), "the slot index is reused");
    assert_eq!(reused.generation(), 1, "the generation is bumped");

    // The old handle no longer matches the entity now occupying its slot.
    assert!(!world.is_alive(a));
    assert!(world.is_alive(reused));
}

// --- 2. Free-list pop order (LIFO, exact match) ----------------------------------------------

#[test]
fn free_list_pop_order_is_lifo_exact() {
    // spawn a, b, c; despawn b, a; respawn twice; assert exact indices + generations.
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    assert_eq!(a, EntityId::new(0, 0));
    assert_eq!(b, EntityId::new(1, 0));
    assert_eq!(c, EntityId::new(2, 0));

    world.despawn(b).unwrap(); // free = [1]
    world.despawn(a).unwrap(); // free = [1, 0]

    // LIFO: the most recently freed slot (a's index 0) comes back first.
    let first = world.spawn();
    let second = world.spawn();
    assert_eq!(first, EntityId::new(0, 1));
    assert_eq!(second, EntityId::new(1, 1));

    // c was untouched.
    assert!(world.is_alive(c));
}

// --- 3. Query iteration order (ascending index, scrambled spawns) ----------------------------

#[test]
fn query_iterates_in_ascending_index_after_scramble() {
    let mut world = World::new();
    let e0 = world.spawn(); // index 0
    let e1 = world.spawn(); // index 1
    let e2 = world.spawn(); // index 2
    let e3 = world.spawn(); // index 3
    world.despawn(e1).unwrap(); // free index 1
    let e4 = world.spawn(); // reuses index 1 (generation 1)
    assert_eq!(e4.index(), 1);

    // Insert in a deliberately scrambled order; iteration must still be by ascending index.
    world.insert(e3, Position { x: 30, y: 0 }).unwrap(); // index 3
    world.insert(e0, Position { x: 0, y: 0 }).unwrap(); // index 0
    world.insert(e4, Position { x: 40, y: 0 }).unwrap(); // index 1
    world.insert(e2, Position { x: 20, y: 0 }).unwrap(); // index 2

    let order: Vec<i32> = world.query::<(&Position,)>().map(|(p,)| p.x).collect();
    // index order 0, 1, 2, 3 -> tags 0, 40, 20, 30
    assert_eq!(order, vec![0, 40, 20, 30]);
}

// --- 4. Join semantics (only entities with ALL components) -----------------------------------

#[test]
fn query_joins_only_entities_having_all_components() {
    let mut world = World::new();
    let both_a = world.spawn();
    let only_pos = world.spawn();
    let both_b = world.spawn();
    let only_vel = world.spawn();

    world.insert(both_a, Position { x: 1, y: 0 }).unwrap();
    world.insert(both_a, Velocity { dx: 10, dy: 0 }).unwrap();
    world.insert(only_pos, Position { x: 2, y: 0 }).unwrap();
    world.insert(both_b, Position { x: 3, y: 0 }).unwrap();
    world.insert(both_b, Velocity { dx: 30, dy: 0 }).unwrap();
    world.insert(only_vel, Velocity { dx: 99, dy: 0 }).unwrap();

    let pairs: Vec<(i32, i32)> = world
        .query::<(&Position, &Velocity)>()
        .map(|(p, v)| (p.x, v.dx))
        .collect();
    assert_eq!(pairs, vec![(1, 10), (3, 30)]);
}

#[test]
fn query_mut_applies_and_reads_second_column() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 0, y: 0 }).unwrap();
    world.insert(e, Velocity { dx: 5, dy: -3 }).unwrap();

    for (pos, vel) in world.query::<(&mut Position, &Velocity)>() {
        pos.x += vel.dx;
        pos.y += vel.dy;
    }
    assert_eq!(
        world.get::<Position>(e).unwrap(),
        Some(&Position { x: 5, y: -3 })
    );
}

#[test]
fn query_over_three_columns() {
    let mut world = World::new();
    let full = world.spawn();
    let partial = world.spawn();
    world.insert(full, Position { x: 1, y: 1 }).unwrap();
    world.insert(full, Velocity { dx: 2, dy: 2 }).unwrap();
    world.insert(full, Health(3)).unwrap();
    world.insert(partial, Position { x: 9, y: 9 }).unwrap();
    world.insert(partial, Velocity { dx: 9, dy: 9 }).unwrap();

    let hits: Vec<u32> = world
        .query::<(&Position, &Velocity, &Health)>()
        .map(|(_, _, h)| h.0)
        .collect();
    assert_eq!(hits, vec![3]);
}

#[test]
#[should_panic(expected = "same component type twice")]
fn duplicate_component_type_in_query_panics() {
    let mut world = World::new();
    let _ = world.spawn();
    // Aliasing a single storage is rejected.
    let _iter = world.query::<(&mut Position, &Position)>();
}

// --- 4b. query_ids: the same join, plus the correct EntityId per row -------------------------

#[test]
fn query_ids_yields_ascending_ids_matching_entities_ground_truth() {
    // Scrambled spawn/despawn/reuse, exactly as `query_iterates_in_ascending_index_after_scramble`,
    // but asserting the id column against `entities()` + `get` ground truth.
    let mut world = World::new();
    let e0 = world.spawn(); // index 0
    let e1 = world.spawn(); // index 1
    let e2 = world.spawn(); // index 2
    let e3 = world.spawn(); // index 3
    world.despawn(e1).unwrap(); // free index 1
    let e4 = world.spawn(); // reuses index 1 at generation 1

    // Insert in scrambled order; iteration must still be by ascending index, with the live id.
    world.insert(e3, Position { x: 30, y: 0 }).unwrap();
    world.insert(e0, Position { x: 0, y: 0 }).unwrap();
    world.insert(e4, Position { x: 40, y: 0 }).unwrap();
    world.insert(e2, Position { x: 20, y: 0 }).unwrap();

    let rows: Vec<(EntityId, i32)> = world
        .query_ids::<(&Position,)>()
        .map(|(id, (p,))| (id, p.x))
        .collect();

    // Ascending index order, each carrying the *current* generation (e4 at index 1, generation 1).
    assert_eq!(rows, vec![(e0, 0), (e4, 40), (e2, 20), (e3, 30)]);

    // Ground truth: the same ids `query` would visit are exactly the live entities that have a
    // Position, in `entities()` order.
    let ground: Vec<(EntityId, i32)> = world
        .entities()
        .filter_map(|id| world.get::<Position>(id).unwrap().map(|p| (id, p.x)))
        .collect();
    assert_eq!(rows, ground);
}

#[test]
fn query_ids_carries_the_live_generation_after_slot_reuse() {
    // Despawn and reuse a slot, then confirm the id-yielding query reports the *live* generation,
    // never the stale one — and that the yielded handle is a valid, alive handle.
    let mut world = World::new();
    let a = world.spawn();
    world.insert(a, Health(1)).unwrap();
    world.despawn(a).unwrap(); // slot 0 freed, generation bumps

    let reused = world.spawn(); // reuses slot 0 at generation 1
    assert_eq!(reused.index(), a.index());
    assert_ne!(reused.generation(), a.generation());
    world.insert(reused, Health(2)).unwrap();

    let ids: Vec<EntityId> = world.query_ids::<(&Health,)>().map(|(id, _)| id).collect();
    assert_eq!(ids, vec![reused]);
    assert!(world.is_alive(ids[0]), "the yielded id is a live handle");
    assert!(!world.is_alive(a), "the stale handle stays dead");
}

#[test]
fn query_ids_mutates_through_mut_columns() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 0, y: 0 }).unwrap();
    world.insert(e, Velocity { dx: 5, dy: -3 }).unwrap();

    for (id, (pos, vel)) in world.query_ids::<(&mut Position, &Velocity)>() {
        assert_eq!(id, e);
        pos.x += vel.dx;
        pos.y += vel.dy;
    }
    assert_eq!(
        world.get::<Position>(e).unwrap(),
        Some(&Position { x: 5, y: -3 })
    );
}

#[test]
#[should_panic(expected = "same component type twice")]
fn duplicate_component_type_in_query_ids_panics() {
    let mut world = World::new();
    let _ = world.spawn();
    let _iter = world.query_ids::<(&mut Position, &Position)>();
}

#[test]
fn query_ids_records_the_same_shape_as_query() {
    // An id-yielding query is the SAME shape as the plain one: the recorder must not tell them apart.
    fn record(run: impl FnOnce(&mut World)) -> Vec<Vec<(std::any::TypeId, bool)>> {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position { x: 0, y: 0 }).unwrap();
        world.insert(e, Velocity { dx: 1, dy: 1 }).unwrap();

        world.begin_query_recording();
        world.set_recording_system("s");
        run(&mut world);
        let obs = world.take_query_recording().unwrap();
        obs.shapes("s").map(<[_]>::to_vec).collect()
    }

    let via_query = record(|w| for _ in w.query::<(&Position, &mut Velocity)>() {});
    let via_query_ids = record(|w| for _ in w.query_ids::<(&Position, &mut Velocity)>() {});

    assert!(!via_query.is_empty(), "the plain query recorded a shape");
    assert_eq!(
        via_query, via_query_ids,
        "query_ids records an identical observed shape to query"
    );
}

/// Records `run`'s accesses under system `"s"` over a world holding one entity with a `Position` and
/// a `Velocity`, returning the shapes observed for that system.
fn record_accesses(run: impl FnOnce(&mut World, EntityId)) -> Vec<Vec<(std::any::TypeId, bool)>> {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 0, y: 0 }).unwrap();
    world.insert(e, Velocity { dx: 1, dy: 1 }).unwrap();

    world.begin_query_recording();
    world.set_recording_system("s");
    run(&mut world, e);
    let obs = world.take_query_recording().unwrap();
    obs.shapes("s").map(<[_]>::to_vec).collect()
}

fn read(type_id: std::any::TypeId) -> Vec<(std::any::TypeId, bool)> {
    vec![(type_id, false)]
}

fn write(type_id: std::any::TypeId) -> Vec<(std::any::TypeId, bool)> {
    vec![(type_id, true)]
}

#[test]
fn per_entity_accessors_record_one_component_shapes() {
    // The snapshot-then-`get_mut` idiom the `&mut self` borrow model forces must be as visible to the
    // drift verifier as a typed query is.
    let position = std::any::TypeId::of::<Position>();
    let velocity = std::any::TypeId::of::<Velocity>();
    let health = std::any::TypeId::of::<Health>();

    assert_eq!(
        record_accesses(|w, e| {
            let _ = w.get::<Position>(e).unwrap();
        }),
        vec![read(position)],
        "get records a read"
    );
    assert_eq!(
        record_accesses(|w, e| {
            let _ = w.get_mut::<Position>(e).unwrap();
        }),
        vec![write(position)],
        "get_mut records a write"
    );
    assert_eq!(
        record_accesses(|w, e| {
            w.insert(e, Health(1)).unwrap();
        }),
        vec![write(health)],
        "insert records a write"
    );
    assert_eq!(
        record_accesses(|w, e| {
            w.remove::<Velocity>(e).unwrap();
        }),
        vec![write(velocity)],
        "remove records a write"
    );
    assert!(
        record_accesses(|w, e| assert!(w.has::<Position>(e))).is_empty(),
        "has reads presence, not a value: it records nothing"
    );
}

#[test]
fn per_entity_accesses_record_even_when_they_miss() {
    // The observation is the system's access *intent*: whether this run's data satisfied it is not
    // what the drift gate (or a chunking scheduler reading the same set) is asking.
    let health = std::any::TypeId::of::<Health>();

    assert_eq!(
        record_accesses(|w, e| assert!(w.get::<Health>(e).unwrap().is_none())),
        vec![read(health)],
        "a live entity without the component still records the read"
    );
    assert_eq!(
        record_accesses(|w, e| {
            w.despawn(e).unwrap();
            assert!(w.get::<Health>(e).is_err());
        }),
        vec![read(health)],
        "a dead handle still records the read"
    );
}

#[test]
fn accesses_outside_recording_or_attribution_record_nothing() {
    let mut world = World::new();
    let e = world.spawn();

    // Not recording at all.
    world.insert(e, Health(1)).unwrap();
    let _ = world.get::<Health>(e).unwrap();
    assert!(world.take_query_recording().is_none());

    // Recording, but no system tagged: nothing to attribute the access to.
    world.begin_query_recording();
    let _ = world.get_mut::<Health>(e).unwrap();
    let obs = world.take_query_recording().unwrap();
    assert_eq!(obs.systems().count(), 0);
}

#[test]
fn recorded_accesses_are_attributed_to_the_tagged_system() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(1)).unwrap();

    world.begin_query_recording();
    world.set_recording_system("reader");
    let _ = world.get::<Health>(e).unwrap();
    world.set_recording_system("writer");
    let _ = world.get_mut::<Health>(e).unwrap();
    let obs = world.take_query_recording().unwrap();

    assert_eq!(obs.systems().collect::<Vec<_>>(), vec!["reader", "writer"]);
    let health = std::any::TypeId::of::<Health>();
    assert_eq!(
        obs.shapes("reader").map(<[_]>::to_vec).collect::<Vec<_>>(),
        vec![read(health)]
    );
    assert_eq!(
        obs.shapes("writer").map(<[_]>::to_vec).collect::<Vec<_>>(),
        vec![write(health)]
    );
}

// --- 6. Registry round-trip ------------------------------------------------------------------

#[test]
fn registry_round_trips_a_component_through_json() {
    let mut registry = Registry::new();
    registry.register::<Position>("Position").unwrap();
    registry.register::<Health>("Health").unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 4, y: 5 }).unwrap();
    world.insert(e, Health(77)).unwrap();

    // Serialize every registered component off `e`, in registration order.
    let serialized: Vec<(String, serde_json::Value)> = registry
        .iter()
        .filter_map(|info| {
            info.serialize(&world, e)
                .unwrap()
                .map(|v| (info.name().to_string(), v))
        })
        .collect();
    assert_eq!(serialized.len(), 2);

    // Rebuild in a fresh world and compare.
    let mut fresh = World::new();
    let e2 = fresh.spawn();
    for (name, value) in &serialized {
        registry
            .deserialize_insert(name, value, &mut fresh, e2)
            .unwrap();
    }
    assert_eq!(
        fresh.get::<Position>(e2).unwrap(),
        Some(&Position { x: 4, y: 5 })
    );
    assert_eq!(fresh.get::<Health>(e2).unwrap(), Some(&Health(77)));
}

#[test]
fn registry_deserialize_error_names_the_component() {
    let mut registry = Registry::new();
    registry.register::<Name>("Name").unwrap();

    let mut world = World::new();
    let e = world.spawn();
    // Name expects a string; feed it a number.
    let err = registry
        .deserialize_insert("Name", &serde_json::json!(42), &mut world, e)
        .unwrap_err();
    assert_eq!(err.code, code::COMPONENT_DESERIALIZE_FAILED);
    assert!(
        err.message.contains("Name"),
        "message should name the component: {}",
        err.message
    );
}

#[test]
fn registry_reports_unknown_component_name() {
    let registry = Registry::new();
    let mut world = World::new();
    let e = world.spawn();
    let err = registry.serialize("Nope", &world, e).unwrap_err();
    assert_eq!(err.code, code::UNKNOWN_COMPONENT);
    let _ = &mut world;
}

// --- 7. Registration-order iteration ---------------------------------------------------------

#[test]
fn registry_iterates_in_registration_order() {
    let mut registry = Registry::new();
    // Register in a non-alphabetical order to prove it is registration order, not sorted.
    registry.register::<Velocity>("Velocity").unwrap();
    registry.register::<Position>("Position").unwrap();
    registry.register::<Health>("Health").unwrap();

    let names: Vec<&str> = registry.iter().map(|i| i.name()).collect();
    assert_eq!(names, vec!["Velocity", "Position", "Health"]);
    assert_eq!(registry.len(), 3);
}

// --- 8. Duplicate registration errors --------------------------------------------------------

#[test]
fn duplicate_name_registration_errors() {
    let mut registry = Registry::new();
    registry.register::<Position>("Thing").unwrap();
    // Same name, different type.
    let err = registry.register::<Velocity>("Thing").unwrap_err();
    assert_eq!(err.code, code::DUPLICATE_COMPONENT_REGISTRATION);
}

#[test]
fn duplicate_type_registration_errors() {
    let mut registry = Registry::new();
    registry.register::<Position>("Position").unwrap();
    // Same type, different name.
    let err = registry.register::<Position>("AlsoPosition").unwrap_err();
    assert_eq!(err.code, code::DUPLICATE_COMPONENT_REGISTRATION);
    assert!(err.message.contains("Position"));
}

// --- 9. Schedule runs in registration order --------------------------------------------------

#[test]
fn schedule_runs_systems_in_registration_order() {
    fn first(world: &mut World, _ctx: &SystemCtx) {
        world
            .resource_mut::<Vec<&'static str>>()
            .unwrap()
            .push("first");
    }
    fn second(world: &mut World, _ctx: &SystemCtx) {
        world
            .resource_mut::<Vec<&'static str>>()
            .unwrap()
            .push("second");
    }
    fn third(world: &mut World, _ctx: &SystemCtx) {
        world
            .resource_mut::<Vec<&'static str>>()
            .unwrap()
            .push("third");
    }

    let mut world = World::new();
    world.insert_resource(Vec::<&'static str>::new());

    let mut schedule = Schedule::new();
    schedule.add_system("first", first);
    schedule.add_system("second", second);
    schedule.add_system("third", third);
    assert_eq!(
        schedule.system_names().collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );

    let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);
    schedule.run(&mut world, &ctx);
    schedule.run(&mut world, &ctx);

    assert_eq!(
        *world.resource::<Vec<&'static str>>().unwrap(),
        vec!["first", "second", "third", "first", "second", "third"]
    );
}

// --- 10. Resources ---------------------------------------------------------------------------

#[test]
fn resources_insert_get_mutate_remove() {
    let mut world = World::new();
    assert!(world.resource::<u32>().is_none());

    assert_eq!(world.insert_resource(5u32), None);
    assert_eq!(world.resource::<u32>(), Some(&5));

    *world.resource_mut::<u32>().unwrap() += 1;
    assert_eq!(world.resource::<u32>(), Some(&6));

    // Insert replaces and returns the old value.
    assert_eq!(world.insert_resource(100u32), Some(6));
    assert_eq!(world.remove_resource::<u32>(), Some(100));
    assert!(world.resource::<u32>().is_none());
}

// --- 11. Live-entity enumeration order --------------------------------------------------------

#[test]
fn entities_iterates_live_in_ascending_index_order() {
    let mut world = World::new();
    let a = world.spawn(); // index 0
    let b = world.spawn(); // index 1
    let c = world.spawn(); // index 2
    world.despawn(b).unwrap();
    let d = world.spawn(); // reuses index 1 at generation 1

    // Ascending slot index, with the *current* generation of each slot — never spawn order.
    let live: Vec<EntityId> = world.entities().collect();
    assert_eq!(live, vec![a, d, c]);
    assert_eq!(
        live,
        vec![
            EntityId::new(0, 0),
            EntityId::new(1, 1),
            EntityId::new(2, 0)
        ]
    );
}

// --- Hierarchy -------------------------------------------------------------------------------

#[test]
fn parent_is_a_plain_registrable_component() {
    let mut registry = Registry::new();
    registry.register::<Parent>("Parent").unwrap();

    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();
    world.insert(child, Parent(parent)).unwrap();

    // Round-trips through the registry like any other component.
    let value = registry
        .serialize("Parent", &world, child)
        .unwrap()
        .unwrap();
    let mut fresh = World::new();
    let p2 = fresh.spawn();
    let c2 = fresh.spawn();
    let _ = p2;
    registry
        .deserialize_insert("Parent", &value, &mut fresh, c2)
        .unwrap();
    assert_eq!(fresh.get::<Parent>(c2).unwrap().unwrap().0, parent);
}

#[test]
fn parent_is_the_only_hierarchy_entity_ref() {
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Leash {
        #[schemars(with = "ludion_ecs::EntityRefSchema")]
        holder: EntityId,
    }

    let mut registry = Registry::new();
    registry.register::<Parent>("Parent").unwrap();
    registry.register::<Leash>("Leash").unwrap();

    let parent = registry.get("Parent").unwrap().entity_refs();
    assert_eq!(parent.len(), 1);
    assert_eq!(parent[0].suffix, "");
    assert!(!parent[0].allow_self && !parent[0].optional && parent[0].hierarchy);

    let leash = registry.get("Leash").unwrap().entity_refs();
    assert_eq!(leash.len(), 1);
    assert!(!leash[0].hierarchy);
}

#[test]
fn a_marked_entity_reference_in_a_list_map_or_tuple_is_refused_at_registration() {
    use std::collections::BTreeMap;

    use ludion_ecs::EntityRefSchema;

    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Pack {
        #[schemars(with = "Vec<EntityRefSchema>")]
        members: Vec<EntityId>,
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Named {
        #[schemars(with = "BTreeMap<String, EntityRefSchema>")]
        by_name: BTreeMap<String, EntityId>,
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Pair {
        #[schemars(with = "(EntityRefSchema, EntityRefSchema)")]
        both: (EntityId, EntityId),
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Fine {
        #[schemars(with = "EntityRefSchema")]
        holder: EntityId,
    }

    let mut registry = Registry::new();
    for (name, result, suffix) in [
        ("Pack", registry.register::<Pack>("Pack"), "/members/*"),
        ("Named", registry.register::<Named>("Named"), "/by_name/*"),
        ("Pair", registry.register::<Pair>("Pair"), "/both/0"),
    ] {
        let error = result.expect_err("a collection of references is refused");
        assert_eq!(error.code, code::ENTITY_REF_IN_COLLECTION, "{name}");
        assert!(error.message.contains(suffix), "{name}: {}", error.message);
        assert!(registry.get(name).is_none(), "{name} was not registered");
    }
    registry.register::<Fine>("Fine").unwrap();
    assert_eq!(registry.get("Fine").unwrap().entity_refs().len(), 1);
}

#[test]
fn a_sanctioned_collection_registers_with_a_pattern_field_and_an_unsanctioned_one_names_the_proxy()
{
    use ludion_ecs::{EntityRefCollection, EntityRefSchema};

    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Event {
        #[schemars(with = "Option<EntityRefSchema>")]
        from: Option<EntityId>,
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Inbox {
        #[schemars(with = "EntityRefCollection<Vec<Event>>")]
        inbox: Vec<Event>,
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Bare {
        inbox: Vec<Event>,
    }
    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct Plain {
        #[schemars(with = "Vec<EntityRefSchema>")]
        members: Vec<EntityId>,
    }

    let mut registry = Registry::new();
    registry.register::<Inbox>("Inbox").unwrap();
    let fields = registry.get("Inbox").unwrap().entity_refs();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].suffix, "/inbox/*/from");
    assert!(fields[0].collection && fields[0].nullable);

    // `Bare` holds the same Event list without the proxy: its schema carries a plain `$ref`
    // list, so the nested marker is found inside a collection.
    let error = registry.register::<Bare>("Bare").expect_err("unsanctioned");
    assert_eq!(error.code, code::ENTITY_REF_IN_COLLECTION);
    let error = registry
        .register::<Plain>("Plain")
        .expect_err("unsanctioned");
    assert_eq!(error.code, code::ENTITY_REF_IN_COLLECTION);
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|h| h.contains("EntityRefCollection")),
        "{error:?}"
    );
}

// --- Restore validation ----------------------------------------------------------------------

fn allocator_state(generations: &[u32], free: &[u32]) -> ludion_ecs::AllocatorState {
    serde_json::from_value(serde_json::json!({ "generations": generations, "free": free }))
        .expect("well-formed allocator state")
}

#[test]
fn restore_allocator_rejects_a_duplicate_free_entry_and_leaves_the_world_alone() {
    let mut world = World::new();
    let survivor = world.spawn();

    let error = world
        .restore_allocator(&allocator_state(&[0, 0, 0], &[1, 1]))
        .unwrap_err();
    assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
    assert_eq!(error.path.as_deref(), Some("/free/1"));
    assert!(
        world.is_alive(survivor),
        "a refused restore changes nothing"
    );
}

#[test]
fn restore_allocator_rejects_an_out_of_range_free_entry_without_a_later_panic() {
    let mut world = World::new();
    let error = world
        .restore_allocator(&allocator_state(&[0, 0], &[7]))
        .unwrap_err();
    assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
    // The refused state never became the allocator, so spawning is unaffected.
    assert_eq!(world.spawn(), EntityId::new(0, 0));
}

#[test]
fn restore_entities_rejects_a_duplicate_slot_index() {
    let mut world = World::new();
    let error = world
        .restore_entities(&[EntityId::new(1, 0), EntityId::new(1, 5)])
        .unwrap_err();
    assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
}

#[test]
fn restore_entities_rejects_a_slot_index_past_the_limit_and_leaves_the_world_intact() {
    let mut world = World::new();
    let survivor = world.spawn();
    let error = world
        .restore_entities(&[EntityId::new(MAX_ENTITY_SLOTS, 0)])
        .unwrap_err();
    assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
    assert_eq!(error.path.as_deref(), Some("/0"));
    assert!(world.is_alive(survivor));
    assert_eq!(world.entity_count(), 1);
}

#[test]
fn a_valid_allocator_state_round_trips_the_same_spawn_sequence() {
    let mut original = World::new();
    let ids: Vec<EntityId> = (0..4).map(|_| original.spawn()).collect();
    original.despawn(ids[1]).unwrap();
    let state = original
        .allocator_state()
        .expect("a recycled world carries state");

    let mut restored = World::new();
    restored.restore_allocator(&state).unwrap();
    let direct: Vec<EntityId> = (0..3).map(|_| original.spawn()).collect();
    let after: Vec<EntityId> = (0..3).map(|_| restored.spawn()).collect();
    assert_eq!(after, direct);
}

// --- Batch despawn ≡ sequential despawn ------------------------------------------------------

/// Everything observable about a world that `despawn` touches: live set, each component column's
/// contents, and the allocator.
type Observed = (
    Vec<EntityId>,
    Vec<(EntityId, Position)>,
    Vec<(EntityId, Health)>,
    Vec<(EntityId, Name)>,
    Option<ludion_ecs::AllocatorState>,
);

fn observe(world: &mut World) -> Observed {
    let live = world.entities().collect();
    let positions = world
        .query_ids::<(&Position,)>()
        .map(|(id, (p,))| (id, *p))
        .collect();
    let healths = world
        .query_ids::<(&Health,)>()
        .map(|(id, (h,))| (id, *h))
        .collect();
    let names = world
        .query_ids::<(&Name,)>()
        .map(|(id, (n,))| (id, n.clone()))
        .collect();
    (live, positions, healths, names, world.allocator_state())
}

/// Spawns one entity carrying the components selected by the low bits of `mask`.
fn spawn_with(world: &mut World, mask: u32, seed: u32) -> EntityId {
    let id = world.spawn();
    if mask & 1 != 0 {
        let x = seed as i32;
        world.insert(id, Position { x, y: -x }).unwrap();
    }
    if mask & 2 != 0 {
        world.insert(id, Health(seed)).unwrap();
    }
    if mask & 4 != 0 {
        world.insert(id, Name(format!("e{seed}"))).unwrap();
    }
    id
}

fn shuffle<T>(items: &mut [T], rng: &mut ludion_core::rng::DeterministicRng) {
    for i in (1..items.len()).rev() {
        items.swap(i, rng.gen_range_u32(0..i as u32 + 1) as usize);
    }
}

/// Live ids, every previously despawned handle (stale, or dead and unrecycled), indices that never
/// existed, and duplicates of all of those, shuffled.
fn doomed_list(
    world: &World,
    graveyard: &[EntityId],
    rng: &mut ludion_core::rng::DeterministicRng,
) -> Vec<EntityId> {
    let mut list = Vec::new();
    for id in world.entities() {
        if rng.gen_range_u32(0..3) == 0 {
            list.push(id);
        }
    }
    for &id in graveyard {
        if rng.gen_range_u32(0..4) == 0 {
            list.push(id);
        }
    }
    for _ in 0..rng.gen_range_u32(0..6) {
        list.push(EntityId::new(rng.gen_range_u32(0..400), 0));
    }
    for _ in 0..rng.gen_range_u32(0..8) {
        if !list.is_empty() {
            list.push(list[rng.gen_range_u32(0..list.len() as u32) as usize]);
        }
    }
    shuffle(&mut list, rng);
    list
}

#[test]
fn despawn_many_matches_sequential_despawn_under_randomized_churn() {
    use ludion_core::rng::DeterministicRng;

    for seed in 0..64u64 {
        let mut rng = DeterministicRng::new(0xDE5_9A17 ^ seed);
        let mut sequential = World::new();
        let mut batch = World::new();
        let mut graveyard: Vec<EntityId> = Vec::new();

        for n in 0..rng.gen_range_u32(0..200) {
            let mask = rng.gen_range_u32(0..8);
            let a = spawn_with(&mut sequential, mask, n);
            let b = spawn_with(&mut batch, mask, n);
            assert_eq!(a, b);
        }

        for round in 0..4 {
            let ids = doomed_list(&sequential, &graveyard, &mut rng);

            let mut expected = Vec::new();
            for &id in &ids {
                if let Err(diagnostic) = sequential.despawn(id) {
                    expected.push(diagnostic);
                }
            }
            let actual = batch.despawn_many(&ids);
            assert_eq!(actual, expected, "seed {seed} round {round}: diagnostics");
            assert_eq!(
                observe(&mut batch),
                observe(&mut sequential),
                "seed {seed} round {round}: world state after despawn"
            );
            graveyard.extend(ids.iter().copied());

            // Re-spawn into the freed slots so the next round mixes new handles with the stale
            // ones of those same slots.
            for n in 0..rng.gen_range_u32(0..40) {
                let mask = rng.gen_range_u32(0..8);
                let a = spawn_with(&mut sequential, mask, 1000 + n);
                let b = spawn_with(&mut batch, mask, 1000 + n);
                assert_eq!(a, b, "seed {seed} round {round}: respawn ids");
            }
            assert_eq!(
                observe(&mut batch),
                observe(&mut sequential),
                "seed {seed} round {round}: world state after respawn"
            );
        }

        let next_sequential: Vec<EntityId> = (0..20).map(|_| sequential.spawn()).collect();
        let next_batch: Vec<EntityId> = (0..20).map(|_| batch.spawn()).collect();
        assert_eq!(next_batch, next_sequential, "seed {seed}: next spawns");
    }
}

#[test]
fn despawn_many_reports_a_repeated_id_once_and_frees_it_once() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();

    let errors = world.despawn_many(&[a, a]);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, code::DEAD_ENTITY);
    assert!(!world.is_alive(a));
    assert!(world.is_alive(b));

    assert_eq!(
        world.spawn(),
        EntityId::new(0, 1),
        "slot 0 freed exactly once"
    );
    assert_eq!(
        world.spawn(),
        EntityId::new(2, 0),
        "no second free entry for slot 0"
    );
}

// --- Randomized join equivalence -------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GA(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GB(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GC(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GD(u32);

type Mirror = [std::collections::BTreeMap<u32, u32>; 4];
type Rows = Vec<(u32, Vec<u32>)>;
type Keep<'a> = &'a mut dyn FnMut(usize, u32) -> bool;

/// `n` spawns, a quarter despawned so slot indices have holes, then component `t` inserted (value =
/// the entity's index) on every live entity `keep(t, index)` accepts, in shuffled order so storages
/// splice as well as append. Every insert is mirrored in a `BTreeMap`.
fn join_world(
    n: usize,
    keep: Keep,
    rng: &mut ludion_core::rng::DeterministicRng,
) -> (World, Mirror) {
    let mut world = World::new();
    let ids: Vec<EntityId> = (0..n).map(|_| world.spawn()).collect();
    for &id in &ids {
        if rng.gen_range_u32(0..4) == 0 {
            world.despawn(id).unwrap();
        }
    }
    let mut live: Vec<EntityId> = world.entities().collect();
    shuffle(&mut live, rng);
    let mut mirror = Mirror::default();
    for &id in &live {
        let i = id.index();
        for (t, map) in mirror.iter_mut().enumerate() {
            if !keep(t, i) {
                continue;
            }
            match t {
                0 => drop(world.insert(id, GA(i)).unwrap()),
                1 => drop(world.insert(id, GB(i)).unwrap()),
                2 => drop(world.insert(id, GC(i)).unwrap()),
                _ => drop(world.insert(id, GD(i)).unwrap()),
            }
            map.insert(i, i);
        }
    }
    (world, mirror)
}

/// The naive join: the first column's keys ascending, kept when every other column has the key.
fn join_reference(mirror: &Mirror, cols: &[usize]) -> Rows {
    mirror[cols[0]]
        .keys()
        .filter(|k| cols[1..].iter().all(|&c| mirror[c].contains_key(k)))
        .map(|&k| (k, cols.iter().map(|&c| mirror[c][&k]).collect()))
        .collect()
}

fn check_joins(label: &str, world: &mut World, mirror: &mut Mirror, delta: u32) {
    // Runs a tuple through `query` and `query_ids` and compares both with the reference.
    macro_rules! check {
        ($cols:expr, $q:ty, $values:expr) => {{
            let expected = join_reference(mirror, &$cols);
            let plain: Vec<Vec<u32>> = world.query::<$q>().map($values).collect();
            assert_eq!(
                plain,
                expected.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
                "{label}: query {}",
                stringify!($q),
            );
            let (indices, rows): (Vec<u32>, Vec<_>) = world
                .query_ids::<$q>()
                .map(|(id, row)| (id.index(), row))
                .unzip();
            let with_ids: Rows = indices
                .into_iter()
                .zip(rows.into_iter().map($values))
                .collect();
            assert_eq!(with_ids, expected, "{label}: query_ids {}", stringify!($q));
        }};
    }

    check!([0], (&GA,), |(a,)| vec![a.0]);
    check!([0, 1], (&GA, &GB), |(a, b)| vec![a.0, b.0]);
    check!([3, 0], (&GD, &GA), |(d, a)| vec![d.0, a.0]);
    check!([0, 1, 2, 3], (&GA, &GB, &GC, &GD), |(a, b, c, d)| vec![
        a.0, b.0, c.0, d.0
    ]);
    check!([0, 1, 2], (&GA, &mut GB, &GC), |(a, b, c)| vec![
        a.0, b.0, c.0
    ]);

    // Writing through the `&mut` column must land on exactly the joined rows.
    let joined: Vec<u32> = join_reference(mirror, &[0, 1, 2])
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    for (_, b, _) in world.query::<(&GA, &mut GB, &GC)>() {
        b.0 += delta;
    }
    for (_, (_, b, _)) in world.query_ids::<(&GA, &mut GB, &GC)>() {
        b.0 += delta;
    }
    for k in &joined {
        *mirror[1].get_mut(k).unwrap() += 2 * delta;
    }
    for id in world.entities().collect::<Vec<_>>() {
        let expected = mirror[1].get(&id.index()).copied();
        assert_eq!(
            world.get::<GB>(id).unwrap().map(|b| b.0),
            expected,
            "{label}: GB of entity {} after the write",
            id.index()
        );
    }
}

#[test]
fn query_join_matches_reference_under_randomized_columns() {
    use ludion_core::rng::DeterministicRng;

    // Per-type density in permille: absent, rare, sparse, half, everything.
    const DENSITIES: [u32; 5] = [0, 1, 50, 500, 1000];

    for seed in 0..64u64 {
        let mut rng = DeterministicRng::new(0x009A_110B ^ seed);
        let n = match rng.gen_range_u32(0..4) {
            0 => 0,
            1 => rng.gen_range_usize(1..8),
            2 => rng.gen_range_usize(8..200),
            _ => rng.gen_range_usize(1000..4000),
        };
        let density: [u32; 4] =
            std::array::from_fn(|_| DENSITIES[rng.gen_range_usize(0..DENSITIES.len())]);
        let mut pick = DeterministicRng::new(0x51DE ^ seed);
        let mut keep = |t: usize, _i: u32| pick.gen_range_u32(0..1000) < density[t];
        let (mut world, mut mirror) = join_world(n, &mut keep, &mut rng);
        check_joins(
            &format!("seed {seed} n {n} density {density:?}"),
            &mut world,
            &mut mirror,
            seed as u32 + 1,
        );
    }
}

#[test]
fn query_join_edge_layouts_match_reference() {
    use ludion_core::rng::DeterministicRng;

    let mut rng = DeterministicRng::new(0xED6E);
    let n = 600usize;
    let half = (n / 2) as u32;
    let layouts: [(&str, Keep); 4] = [
        ("every column empty", &mut |_, _| false),
        ("one column empty", &mut |t, _| t != 2),
        // A rare column whose keys all lie past the dense column's end: no row joins.
        ("rare past dense end", &mut |t, i| match t {
            0 => i < half,
            1 => i >= half + 50 && i % 100 == 0,
            _ => true,
        }),
        // A rare column at the very start against dense ones.
        ("rare at start", &mut |t, i| t != 1 || i < 3),
    ];
    for (label, keep) in layouts {
        let (mut world, mut mirror) = join_world(n, keep, &mut rng);
        check_joins(label, &mut world, &mut mirror, 7);
    }
}

// --- Spawn rank (Serious Isaac 0043) --------------------------------------------------------------

#[test]
fn spawn_ranks_are_monotonic_and_a_lifo_reuse_gets_a_fresh_rank() {
    let mut world = World::new();
    let ids: Vec<EntityId> = (0..4).map(|_| world.spawn()).collect();
    let ranks: Vec<u64> = ids
        .iter()
        .map(|&id| world.spawn_rank(id).unwrap())
        .collect();
    assert!(ranks.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(world.spawn_order(), None, "a fresh world is in slot order");

    // Free 1 then 3: LIFO hands slot 3 to the next spawn, then slot 1.
    world.despawn(ids[1]).unwrap();
    world.despawn(ids[3]).unwrap();
    assert_eq!(world.spawn_rank(ids[1]), None, "a stale handle has no rank");
    let a = world.spawn();
    let b = world.spawn();
    assert_eq!((a.index(), b.index()), (3, 1));
    let (ra, rb) = (world.spawn_rank(a).unwrap(), world.spawn_rank(b).unwrap());
    assert!(
        ra > ranks[3] && rb > ra,
        "a reused slot takes a fresh, later rank"
    );
    assert_eq!(world.spawn_order(), Some(vec![0, 2, 3, 1]));
}

#[test]
fn spawn_order_round_trips_through_a_restore() {
    let mut world = World::new();
    let ids: Vec<EntityId> = (0..4).map(|_| world.spawn()).collect();
    world.despawn(ids[1]).unwrap();
    world.despawn(ids[3]).unwrap();
    world.spawn();
    world.spawn();
    let order = world.spawn_order().expect("churned");
    assert!(
        world.allocator_state().is_none(),
        "every recycled slot is live again, so the live set rebuilds the allocator"
    );
    let live: Vec<EntityId> = world.entities().collect();

    let mut restored = World::new();
    restored.restore_entities(&live).unwrap();
    assert_eq!(
        restored.spawn_order(),
        None,
        "restore defaults to slot order"
    );
    restored.restore_spawn_order(&order).unwrap();
    assert_eq!(restored.spawn_order(), Some(order));
    let next = restored.spawn();
    assert_eq!(
        restored.spawn_rank(next).unwrap(),
        4,
        "a post-restore spawn ranks after every live entity"
    );
}

#[test]
fn restore_spawn_order_rejects_a_non_permutation_and_leaves_ranks_alone() {
    let mut world = World::new();
    for _ in 0..3 {
        world.spawn();
    }
    for bad in [&[0u32, 1][..], &[0, 1, 1], &[0, 1, 9]] {
        let error = world.restore_spawn_order(bad).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID, "{bad:?}");
    }
    assert_eq!(world.spawn_order(), None);
}

#[test]
fn a_recycled_but_reducible_allocator_still_omits_its_state() {
    // Slots 1 and 3 are recycled (generation 1) yet live again: the live ids carry the generations,
    // so the allocator is reducible although the ranks no longer follow slot order.
    let mut world = World::new();
    let ids: Vec<EntityId> = (0..4).map(|_| world.spawn()).collect();
    world.despawn(ids[1]).unwrap();
    world.despawn(ids[3]).unwrap();
    world.spawn();
    world.spawn();
    assert!(world.spawn_order().is_some());
    assert!(
        world.allocator_state().is_none(),
        "ranks are not part of allocator equality"
    );
}
