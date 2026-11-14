//! Ray cast, shape cast and overlap: sort order, `QueryFilter`, bad-shape and bad-input errors, and
//! restore-parity / purity against a live scene.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CollisionLayers, NoParent, PhysicsError, PhysicsSettings, PhysicsSpace, PhysicsWorld,
    QueryFilter, QueryHit, Sweep, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn box_collider(body: Body) -> Collider {
    Collider {
        shape: Shape::Box {
            half_extents: [0.5; 3],
        },
        body,
        layer: None,
        sensor: false,
    }
}

fn spawn_static_box(world: &mut World, x: f32) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([x, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, box_collider(Body::new_static()))
        .expect("a fresh entity accepts a collider");
    entity
}

#[test]
fn only_fixed_keeps_static_colliders_for_every_query() {
    let mut world = World::new();
    let mut spawn = |x: f32, body: Body| {
        let entity = world.spawn();
        world.insert(entity, TestPose::at([x, 0.0, 0.0])).unwrap();
        world.insert(entity, box_collider(body)).unwrap();
        entity
    };
    let fixed = spawn(2.0, Body::new_static());
    let kinematic = spawn(4.0, Body::new_kinematic());
    let rigid = spawn(6.0, Body::Rigid(RigidBody::default()));
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    let physics = physics(&world);
    let only = QueryFilter::new().only_fixed();

    let all = physics
        .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(all.len(), 3, "{all:?}");
    let cast = physics
        .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, &only)
        .unwrap();
    assert_eq!(cast.iter().map(|h| h.entity).collect::<Vec<_>>(), [fixed]);

    let spanning = Shape::Box {
        half_extents: [4.0, 1.0, 1.0],
    };
    let id = [0.0, 0.0, 0.0, 1.0];
    let all = physics
        .overlap(&spanning, [4.0, 0.0, 0.0], id, &QueryFilter::new())
        .unwrap();
    assert_eq!(all, [fixed, kinematic, rigid]);
    let hit = physics
        .overlap(&spanning, [4.0, 0.0, 0.0], id, &only)
        .unwrap();
    assert_eq!(hit, [fixed]);

    let probe = Sweep::new([3.0, 0.0, 0.0], [10.0, 0.0, 0.0], 0.0);
    let swept = |filter: &QueryFilter| {
        let filter = physics.sweep_filter(filter).unwrap();
        physics.first_hit(&probe, &filter).map(|h| h.entity)
    };
    assert_eq!(swept(&QueryFilter::new()), Some(kinematic));
    assert_eq!(
        swept(&only),
        None,
        "the fixed box is behind the probe; the rigid one is dropped"
    );
    let back = Sweep::new([3.0, 0.0, 0.0], [-10.0, 0.0, 0.0], 0.0);
    let filter = physics.sweep_filter(&only).unwrap();
    assert_eq!(
        physics.first_hit(&back, &filter).map(|h| h.entity),
        Some(fixed)
    );
}

fn free_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world once a body claims it")
}

#[test]
fn ray_hits_are_sorted_by_distance() {
    let mut world = World::new();
    // Spawned far-first so entity order is not distance order.
    let far = spawn_static_box(&mut world, 6.0);
    let near = spawn_static_box(&mut world, 2.0);
    let mid = spawn_static_box(&mut world, 4.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    assert_eq!(
        pose_of(&world, near).translation,
        [2.0, 0.0, 0.0],
        "a static body must not move"
    );

    let hits = physics(&world)
        .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
        .expect("a finite ray");
    assert_eq!(
        hits.iter().map(|h| h.entity).collect::<Vec<_>>(),
        vec![near, mid, far],
        "{hits:?}"
    );
    assert_eq!(
        hits.iter().map(|h| h.distance).collect::<Vec<_>>(),
        vec![1.5, 3.5, 5.5],
        "{hits:?}"
    );
    assert_eq!(hits[0].point, [1.5, 0.0, 0.0], "{hits:?}");
    for hit in &hits {
        assert_eq!(hit.normal, [-1.0, 0.0, 0.0], "{hits:?}");
    }

    let excluded = physics(&world)
        .ray_cast(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            100.0,
            &QueryFilter::new().exclude(near),
        )
        .expect("a finite ray");
    assert_eq!(
        excluded.iter().map(|h| h.entity).collect::<Vec<_>>(),
        vec![mid, far]
    );

    let capped = physics(&world)
        .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 4.0, &QueryFilter::new())
        .expect("a finite ray");
    assert_eq!(
        capped.iter().map(|h| h.entity).collect::<Vec<_>>(),
        vec![near, mid]
    );

    let scaled = physics(&world)
        .ray_cast([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 100.0, &QueryFilter::new())
        .expect("a finite ray");
    assert_eq!(
        scaled.iter().map(|h| h.distance).collect::<Vec<_>>(),
        vec![1.5, 3.5, 5.5],
        "normalization must make an un-unit direction give the same distances"
    );

    let huge = physics(&world)
        .ray_cast(
            [0.0, 0.0, 0.0],
            [1e20, 0.0, 0.0],
            100.0,
            &QueryFilter::new(),
        )
        .expect("a direction whose squared length leaves f32 still normalizes");
    assert_eq!(
        huge.iter().map(|h| h.distance).collect::<Vec<_>>(),
        vec![1.5, 3.5, 5.5],
        "{huge:?}"
    );
}

#[test]
fn shape_cast_hits_are_sorted_by_distance() {
    let mut world = World::new();
    let far = spawn_static_box(&mut world, 6.0);
    let near = spawn_static_box(&mut world, 2.0);
    let mid = spawn_static_box(&mut world, 4.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let sphere = Shape::Sphere { radius: 0.5 };
    let hits = physics(&world)
        .shape_cast(
            &sphere,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            100.0,
            &QueryFilter::new(),
        )
        .expect("a finite shape cast");
    assert_eq!(
        hits.iter().map(|h| h.entity).collect::<Vec<_>>(),
        vec![near, mid, far],
        "{hits:?}"
    );
    // GJK-based conservative advancement for a round shape converges to a looser tolerance than
    // the ray's exact slab test; 1e-3 comfortably covers the ~3.5e-4 residual observed here.
    const TOL: f32 = 1e-3;
    for (hit, expected) in hits.iter().zip([1.0_f32, 3.0, 5.0]) {
        assert!((hit.distance - expected).abs() < TOL, "{hits:?}");
    }
    assert!((hits[0].point[0] - 1.5).abs() < TOL, "{hits:?}");
    assert!((hits[0].normal[0] - -1.0).abs() < TOL, "{hits:?}");

    let overlapping = physics(&world)
        .shape_cast(
            &sphere,
            [2.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            10.0,
            &QueryFilter::new(),
        )
        .expect("a finite shape cast");
    let near_hit = overlapping
        .iter()
        .find(|h| h.entity == near)
        .expect("a sphere starting inside the box is a hit on it");
    assert_eq!(near_hit.distance, 0.0, "{overlapping:?}");
}

#[test]
fn overlap_is_sorted_by_entity() {
    let mut world = World::new();
    let far = spawn_static_box(&mut world, 6.0);
    let near = spawn_static_box(&mut world, 2.0);
    let mid = spawn_static_box(&mut world, 4.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let spanning = Shape::Box {
        half_extents: [3.0, 1.0, 1.0],
    };
    let mut expected = vec![near, mid, far];
    expected.sort();
    let hits = physics(&world)
        .overlap(
            &spanning,
            [4.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            &QueryFilter::new(),
        )
        .expect("a finite overlap");
    assert_eq!(hits, expected);

    let excluded = physics(&world)
        .overlap(
            &spanning,
            [4.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            &QueryFilter::new().exclude(mid),
        )
        .expect("a finite overlap");
    let expected_excluded: Vec<EntityId> = expected.iter().copied().filter(|&e| e != mid).collect();
    assert_eq!(excluded, expected_excluded);

    let empty_space = physics(&world)
        .overlap(
            &Shape::Sphere { radius: 0.1 },
            [100.0, 100.0, 100.0],
            [0.0, 0.0, 0.0, 1.0],
            &QueryFilter::new(),
        )
        .expect("a finite overlap");
    assert!(empty_space.is_empty());
}

#[test]
fn a_query_shape_the_solver_cannot_build_is_an_error() {
    let mut world = World::new();
    spawn_static_box(&mut world, 0.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let degenerate = Shape::Convex {
        points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
    };
    let err = physics(&world)
        .overlap(
            &degenerate,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            &QueryFilter::new(),
        )
        .expect_err("three collinear points cannot build a hull");
    match err {
        PhysicsError::QueryShape { code, .. } => {
            assert_eq!(code, ludion_core::codes::code::COLLIDER_DEGENERATE_HULL);
        }
        other => panic!("expected QueryShape, got {other:?}"),
    }
}

#[test]
fn bad_query_input_is_an_error() {
    let world = PhysicsWorld::new(TickRate::DEFAULT);

    let err = world
        .ray_cast(
            [f32::NAN, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            1.0,
            &QueryFilter::new(),
        )
        .expect_err("a NaN origin is bad input");
    assert!(
        matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("origin")),
        "{err}"
    );

    let err = world
        .ray_cast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 1.0, &QueryFilter::new())
        .expect_err("a zero direction is bad input");
    assert!(
        matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("direction")),
        "{err}"
    );

    let err = world
        .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], -1.0, &QueryFilter::new())
        .expect_err("a negative max_distance is bad input");
    assert!(
        matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("max_distance")),
        "{err}"
    );

    let err = world
        .ray_cast(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            f32::INFINITY,
            &QueryFilter::new(),
        )
        .expect_err("an infinite max_distance is bad input");
    assert!(
        matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("max_distance")),
        "{err}"
    );

    let err = world
        .overlap(
            &Shape::Sphere { radius: 0.5 },
            [0.0, 0.0, 0.0],
            [f32::NAN, 0.0, 0.0, 1.0],
            &QueryFilter::new(),
        )
        .expect_err("a NaN rotation is bad input");
    assert!(
        matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("rotation")),
        "{err}"
    );

    let ok = world
        .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0, &QueryFilter::new())
        .expect("a zero max_distance is valid");
    assert!(ok.is_empty());
}

#[test]
fn a_non_unit_rotation_is_bad_query_input() {
    let world = PhysicsWorld::new(TickRate::DEFAULT);
    let shape = Shape::Sphere { radius: 0.5 };

    for rotation in [[0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 2.0]] {
        let err = world
            .shape_cast(
                &shape,
                [0.0, 0.0, 0.0],
                rotation,
                [1.0, 0.0, 0.0],
                1.0,
                &QueryFilter::new(),
            )
            .expect_err("a non-unit rotation is bad input");
        assert!(
            matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("rotation")),
            "{err}"
        );

        let err = world
            .overlap(&shape, [0.0, 0.0, 0.0], rotation, &QueryFilter::new())
            .expect_err("a non-unit rotation is bad input");
        assert!(
            matches!(&err, PhysicsError::QueryInput(msg) if msg.contains("rotation")),
            "{err}"
        );
    }

    let unit = euler_xyz_degrees_to_quat([30.0, 45.0, 60.0]);
    world
        .shape_cast(
            &shape,
            [0.0, 0.0, 0.0],
            unit,
            [1.0, 0.0, 0.0],
            1.0,
            &QueryFilter::new(),
        )
        .expect("a unit quaternion from euler_xyz_degrees_to_quat is valid input");
    world
        .overlap(&shape, [0.0, 0.0, 0.0], unit, &QueryFilter::new())
        .expect("a unit quaternion from euler_xyz_degrees_to_quat is valid input");
}

/// A static floor plus a dozen falling rigid bodies, alternating spheres and boxes.
fn build_falling_scene(world: &mut World) {
    let floor = world.spawn();
    world
        .insert(floor, TestPose::at([0.0, -0.5, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            floor,
            Collider {
                shape: Shape::Box {
                    half_extents: [10.0, 0.5, 10.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");

    for index in 0..12u32 {
        let entity = world.spawn();
        let x = (index % 4) as f32 * 1.3 - 2.0;
        let z = (index / 4) as f32 * 1.3 - 1.3;
        let y = 2.0 + index as f32 * 0.4;
        world
            .insert(entity, TestPose::at([x, y, z]))
            .expect("a fresh entity accepts a pose");
        let shape = if index % 2 == 0 {
            Shape::Sphere { radius: 0.4 }
        } else {
            Shape::Box {
                half_extents: [0.4; 3],
            }
        };
        world
            .insert(
                entity,
                Collider {
                    shape,
                    body: Body::Rigid(RigidBody {
                        mass: 1.0,
                        restitution: 0.2,
                        sleep_after_ticks: Some(30),
                        ..RigidBody::default()
                    }),
                    layer: None,
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");
        world
            .insert(entity, Velocity::default())
            .expect("a fresh entity accepts a velocity");
    }
}

/// The bits of every field of a hit, so two batteries compare exactly rather than through `f32`'s
/// `==` (which would equate a `NaN` with itself falsely and distinct zero signs falsely too).
type HitBits = (EntityId, u32, [u32; 3], [u32; 3]);

fn hit_bits(hit: &QueryHit) -> HitBits {
    (
        hit.entity,
        hit.distance.to_bits(),
        hit.point.map(f32::to_bits),
        hit.normal.map(f32::to_bits),
    )
}

fn hits_bits(hits: &[QueryHit]) -> Vec<HitBits> {
    hits.iter().map(hit_bits).collect()
}

/// One fixed battery of queries run against a stepped falling scene: three rays (one of them
/// through the pile), a shape cast and an overlap.
///
/// Every query's geometry is chosen to hit the static floor no matter where the falling bodies
/// have got to by the tick the battery runs at — a query landing on a moving body only by luck
/// would make the non-emptiness checks below flaky rather than deterministic.
struct Battery {
    ray_side: Vec<QueryHit>,
    ray_down: Vec<QueryHit>,
    ray_through_pile: Vec<QueryHit>,
    shape: Vec<QueryHit>,
    overlap: Vec<EntityId>,
}

/// The `x`/`z` of the falling body at grid index 1 — `x = -0.7`, `z = -1.3` in
/// [`build_falling_scene`] — so [`Battery::ray_through_pile`] and [`Battery::shape`] sweep straight
/// down that body's column before reaching the floor beneath it.
const PILE_COLUMN: [f32; 2] = [-0.7, -1.3];

fn run_battery(physics: &PhysicsWorld) -> Battery {
    Battery {
        // Horizontal, into the floor's side face from outside its footprint: a guaranteed hit
        // independent of the pile.
        ray_side: physics
            .ray_cast(
                [-15.0, -0.5, 0.0],
                [1.0, 0.0, 0.0],
                30.0,
                &QueryFilter::new(),
            )
            .expect("a finite ray"),
        // Vertical, straight onto the floor's top from above: a guaranteed hit independent of the
        // pile.
        ray_down: physics
            .ray_cast(
                [0.0, 10.0, 0.0],
                [0.0, -1.0, 0.0],
                20.0,
                &QueryFilter::new(),
            )
            .expect("a finite ray"),
        // Vertical through one pile column: always reaches the floor beneath it, and passes
        // through wherever that column's falling body currently is on the way down.
        ray_through_pile: physics
            .ray_cast(
                [PILE_COLUMN[0], 10.0, PILE_COLUMN[1]],
                [0.0, -1.0, 0.0],
                20.0,
                &QueryFilter::new(),
            )
            .expect("a finite ray"),
        shape: physics
            .shape_cast(
                &Shape::Sphere { radius: 0.3 },
                [PILE_COLUMN[0], 10.0, PILE_COLUMN[1]],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, -1.0, 0.0],
                20.0,
                &QueryFilter::new(),
            )
            .expect("a finite shape cast"),
        overlap: physics
            .overlap(
                &Shape::Box {
                    half_extents: [5.0, 5.0, 5.0],
                },
                [0.0, 2.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
                &QueryFilter::new(),
            )
            .expect("a finite overlap"),
    }
}

fn assert_batteries_bit_identical(a: &Battery, b: &Battery) {
    assert_eq!(hits_bits(&a.ray_side), hits_bits(&b.ray_side));
    assert_eq!(hits_bits(&a.ray_down), hits_bits(&b.ray_down));
    assert_eq!(
        hits_bits(&a.ray_through_pile),
        hits_bits(&b.ray_through_pile)
    );
    assert_eq!(hits_bits(&a.shape), hits_bits(&b.shape));
    assert_eq!(a.overlap, b.overlap);
}

#[test]
fn queries_are_deterministic_and_pure() {
    let mut first = World::new();
    build_falling_scene(&mut first);
    let mut schedule_a = free_schedule();
    for frame in 0..40 {
        tick(&mut first, &mut schedule_a, frame);
    }

    let mut second = World::new();
    build_falling_scene(&mut second);
    let mut schedule_b = free_schedule();
    for frame in 0..40 {
        tick(&mut second, &mut schedule_b, frame);
    }

    let bytes_before = physics(&first)
        .to_bytes()
        .expect("the solver state encodes");
    let battery_first = run_battery(physics(&first));
    let bytes_after = physics(&first)
        .to_bytes()
        .expect("the solver state encodes");
    assert_eq!(
        bytes_before, bytes_after,
        "a query must not mutate the world it reads"
    );
    assert!(
        !battery_first.overlap.is_empty(),
        "the overlap box spans the whole drop column, so it must hit something"
    );
    assert!(
        !battery_first.ray_side.is_empty(),
        "the side ray always reaches the floor"
    );
    assert!(
        !battery_first.ray_down.is_empty(),
        "the downward ray always reaches the floor"
    );
    assert!(
        !battery_first.ray_through_pile.is_empty(),
        "the column ray always reaches the floor beneath it"
    );
    assert!(
        !battery_first.shape.is_empty(),
        "the column shape cast always reaches the floor beneath it"
    );

    let battery_second = run_battery(physics(&second));
    assert_batteries_bit_identical(&battery_first, &battery_second);

    let third = PhysicsWorld::from_bytes(&bytes_before, TickRate::DEFAULT)
        .expect("the bytes this run just wrote decode");
    let battery_third = run_battery(&third);
    assert_batteries_bit_identical(&battery_first, &battery_third);
}

/// A layered world: static boxes at x = 2, 4, 6, 8 — `props`, `dummy`, unlayered, `dummy` — after
/// one tick. The masks are all 0 because only the query side is under test.
fn layered_row() -> (World, [EntityId; 4]) {
    let mut world = World::new();
    let layers = [Some("props"), Some("dummy"), None, Some("dummy")];
    let mut entities = [EntityId::new(0, 0); 4];
    for (i, layer) in layers.into_iter().enumerate() {
        let entity = spawn_static_box(&mut world, 2.0 + 2.0 * i as f32);
        let mut collider = box_collider(Body::new_static());
        collider.layer = layer.map(str::to_string);
        world
            .insert(entity, collider)
            .expect("a live entity accepts a collider");
        entities[i] = entity;
    }
    let mut schedule = Schedule::new();
    let table = CollisionLayers::new(
        vec!["default".into(), "dummy".into(), "props".into()],
        vec![0, 0, 0],
    );
    install::<TestPose, NoParent>(
        &mut schedule,
        PhysicsSettings {
            layers: Some(table),
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    tick(&mut world, &mut schedule, 0);
    (world, entities)
}

fn ray_entities(world: &World, filter: &QueryFilter) -> Vec<EntityId> {
    physics(world)
        .ray_cast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0, filter)
        .expect("a valid ray")
        .iter()
        .map(|h| h.entity)
        .collect()
}

fn overlap_entities(world: &World, filter: &QueryFilter) -> Vec<EntityId> {
    let spanning = Shape::Box {
        half_extents: [6.0, 1.0, 1.0],
    };
    physics(world)
        .overlap(&spanning, [5.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], filter)
        .expect("a valid overlap")
}

fn cast_entities(world: &World, filter: &QueryFilter) -> Vec<EntityId> {
    physics(world)
        .shape_cast(
            &Shape::Sphere { radius: 0.5 },
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            100.0,
            filter,
        )
        .expect("a valid shape cast")
        .iter()
        .map(|h| h.entity)
        .collect()
}

#[test]
fn a_layer_filter_drops_that_layers_hits() {
    let (world, [props, dummy_a, unlayered, dummy_b]) = layered_row();
    let all = QueryFilter::new();
    assert_eq!(
        ray_entities(&world, &all),
        vec![props, dummy_a, unlayered, dummy_b]
    );

    let no_dummies = QueryFilter::new().ignore_layer("dummy");
    assert_eq!(ray_entities(&world, &no_dummies), vec![props, unlayered]);
    assert_eq!(cast_entities(&world, &no_dummies), vec![props, unlayered]);
    let mut expected = vec![props, unlayered];
    expected.sort_unstable();
    assert_eq!(overlap_entities(&world, &no_dummies), expected);

    let no_default = QueryFilter::new().ignore_layer("default");
    assert_eq!(
        ray_entities(&world, &no_default),
        vec![props, dummy_a, dummy_b],
        "`default` names the unlayered colliders"
    );
}

#[test]
fn an_unknown_query_layer_is_an_error() {
    let (world, _) = layered_row();
    let typo = QueryFilter::new().ignore_layer("dumy");
    let sphere = Shape::Sphere { radius: 0.5 };
    let identity = [0.0, 0.0, 0.0, 1.0];
    let errors = [
        physics(&world)
            .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 10.0, &typo)
            .unwrap_err(),
        physics(&world)
            .shape_cast(&sphere, [0.0; 3], identity, [1.0, 0.0, 0.0], 10.0, &typo)
            .unwrap_err(),
        physics(&world)
            .overlap(&sphere, [0.0; 3], identity, &typo)
            .unwrap_err(),
    ];
    for err in errors {
        let PhysicsError::QueryInput(message) = err else {
            panic!("expected QueryInput, found {err:?}");
        };
        assert!(message.contains("`dummy`"), "{message}");
    }

    let mut unlayered_world = World::new();
    spawn_static_box(&mut unlayered_world, 2.0);
    let mut schedule = free_schedule();
    tick(&mut unlayered_world, &mut schedule, 0);
    let unlayered = physics(&unlayered_world);
    let ray = |name: &str| {
        unlayered.ray_cast(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            10.0,
            &QueryFilter::new().ignore_layer(name),
        )
    };
    assert_eq!(
        ray("default"),
        Ok(Vec::new()),
        "ignoring default drops every hit"
    );
    assert!(matches!(ray("dummy"), Err(PhysicsError::QueryInput(_))));
}

#[test]
fn exclude_and_layer_compose() {
    let (world, [props, _dummy_a, unlayered, _dummy_b]) = layered_row();
    let filter = QueryFilter::new().exclude(props).ignore_layer("dummy");
    assert_eq!(ray_entities(&world, &filter), vec![unlayered]);
    assert_eq!(cast_entities(&world, &filter), vec![unlayered]);
    assert_eq!(overlap_entities(&world, &filter), vec![unlayered]);
}

// Heightfield ray casts on grid lines (external-game request 0017).

const TERRAIN_ROWS: u32 = 41;
const TERRAIN_COLS: u32 = 151;

/// A static plane `y = 0.35 * c + 0.2 * r` over the external game's 151x41 grid of 2 m cells, so every point
/// has an analytic height.
fn spawn_terrain(world: &mut World, at: [f32; 3]) -> EntityId {
    let heights = (0..TERRAIN_ROWS)
        .flat_map(|r| (0..TERRAIN_COLS).map(move |c| 0.35 * c as f32 + 0.2 * r as f32))
        .collect();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Heightfield {
                    rows: TERRAIN_ROWS,
                    cols: TERRAIN_COLS,
                    heights,
                    scale: [300.0, 1.0, 80.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    entity
}

/// World position and analytic height at fractional grid coordinates `(c, r)`, the way a game
/// computes them rather than through parry.
fn terrain_point(at: [f32; 3], c: f32, r: f32) -> ([f32; 3], f32) {
    (
        [at[0] - 150.0 + 2.0 * c, at[1], at[2] - 40.0 + 2.0 * r],
        at[1] + 0.35 * c + 0.2 * r,
    )
}

fn terrain_world(at: [f32; 3]) -> (World, Schedule) {
    let mut world = World::new();
    spawn_terrain(&mut world, at);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    (world, schedule)
}

#[test]
fn a_vertical_ray_hits_a_heightfield_on_every_grid_line() {
    for at in [[0.0, 0.0, 0.0], [37.3, 2.5, -11.7]] {
        let (world, _schedule) = terrain_world(at);
        let mut misses: Vec<(u32, u32, &str, f32, f32)> = Vec::new();
        let mut casts = 0u32;
        let mut probe = |r: u32, c: u32, kind: &'static str, cf: f32, rf: f32| {
            let ([x, _, z], expected) = terrain_point(at, cf, rf);
            casts += 1;
            let hits = physics(&world)
                .ray_cast(
                    [x, at[1] + 500.0, z],
                    [0.0, -1.0, 0.0],
                    1000.0,
                    &QueryFilter::new(),
                )
                .expect("a finite ray");
            if hits
                .first()
                .is_none_or(|h| (h.point[1] - expected).abs() > 1e-3)
            {
                misses.push((r, c, kind, x, z));
            }
        };
        for r in 0..TERRAIN_ROWS {
            for c in 0..TERRAIN_COLS {
                probe(r, c, "vertex", c as f32, r as f32);
                for f in [0.25f32, 0.5] {
                    if c + 1 < TERRAIN_COLS {
                        probe(r, c, "x edge", c as f32 + f, r as f32);
                    }
                    if r + 1 < TERRAIN_ROWS {
                        probe(r, c, "z edge", c as f32, r as f32 + f);
                    }
                }
                if c + 1 < TERRAIN_COLS && r + 1 < TERRAIN_ROWS {
                    probe(r, c, "diagonal", c as f32 + 0.5, r as f32 + 0.5);
                }
            }
        }
        assert!(
            misses.is_empty(),
            "pose {at:?}: {} of {casts} vertical rays missed; first eight: {:?}",
            misses.len(),
            &misses[..misses.len().min(8)]
        );
    }
}

/// Oblique rays with no x motion, aimed at points on cell edges so the ray lies in a grid plane.
/// Residue, not covered: an oblique ray lying in a grid plane can still miss on a shared edge or
/// vertex, from f32 rounding in parry's zero-tolerance triangle test (measured 282 of 46488 for
/// in-plane rays travelling along x).
#[test]
fn an_oblique_ray_along_a_grid_line_hits_the_heightfield() {
    let at = [37.3, 2.5, -11.7];
    let (world, _schedule) = terrain_world(at);
    let mut misses: Vec<(u32, u32, Option<[f32; 3]>)> = Vec::new();
    let mut casts = 0u32;
    let raw = [0.0f32, -1.0, 0.7];
    let len = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
    let d = raw.map(|v| v / len);
    for r in 1..TERRAIN_ROWS - 1 {
        for c in 1..TERRAIN_COLS - 1 {
            for f in [0.25f32, 0.5] {
                for (cf, rf) in [(c as f32 + f, r as f32), (c as f32, r as f32 + f)] {
                    let ([x, _, z], y) = terrain_point(at, cf, rf);
                    let target = [x, y, z];
                    let origin = [
                        target[0] - 30.0 * d[0],
                        target[1] - 30.0 * d[1],
                        target[2] - 30.0 * d[2],
                    ];
                    casts += 1;
                    let hits = physics(&world)
                        .ray_cast(origin, d, 60.0, &QueryFilter::new())
                        .expect("a finite ray");
                    let hit = hits.first().map(|h| h.point);
                    if !hit.is_some_and(|p| (0..3).all(|a| (p[a] - target[a]).abs() <= 1e-3)) {
                        misses.push((r, c, hit));
                    }
                }
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{} of {casts} oblique rays missed; first eight (r, c, hit): {:?}",
        misses.len(),
        &misses[..misses.len().min(8)]
    );
}

#[test]
fn the_request_three_by_three_slope_hits_at_its_centre_vertex() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Heightfield {
                    rows: 3,
                    cols: 3,
                    heights: vec![0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0],
                    scale: [4.0, 1.0, 4.0],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    for (origin, expected) in [([0.0, 10.0, 0.0], 1.0), ([0.3, 10.0, 0.1], 1.15)] {
        let hits = physics(&world)
            .ray_cast(origin, [0.0, -1.0, 0.0], 20.0, &QueryFilter::new())
            .expect("a finite ray");
        let hit = hits
            .first()
            .unwrap_or_else(|| panic!("no hit from {origin:?}"));
        assert!(
            (hit.point[1] - expected).abs() < 1e-3,
            "from {origin:?}: {hit:?}"
        );
    }
}

fn spawn_collider(world: &mut World, at: [f32; 3], shape: Shape) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    let mut collider = box_collider(Body::new_static());
    collider.shape = shape;
    world.insert(entity, collider).unwrap();
    entity
}

#[test]
fn closest_points_rank_by_distance_then_entity() {
    let mut world = World::new();
    // Two boxes equidistant from the origin on opposite sides; spawned so entity order is known.
    let first = spawn_static_box(&mut world, 3.0);
    let second = spawn_static_box(&mut world, -3.0);
    let far = spawn_static_box(&mut world, 6.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let hits = physics(&world)
        .closest_points([0.0; 3], 100.0, &QueryFilter::new())
        .expect("a finite query");
    assert_eq!(
        hits.iter().map(|h| h.entity).collect::<Vec<_>>(),
        [first, second, far],
        "{hits:?}"
    );
    assert_eq!(hits[0].distance, 2.5);
    assert_eq!(hits[1].distance, 2.5);
    assert_eq!(hits[2].distance, 5.5);
    assert_eq!(hits[0].point, [2.5, 0.0, 0.0]);
    assert_eq!(hits[1].point, [-2.5, 0.0, 0.0]);
}

#[test]
fn closest_points_inside_a_solid_is_zero_at_the_query_point() {
    let mut world = World::new();
    let inside = spawn_static_box(&mut world, 0.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let hits = physics(&world)
        .closest_points([0.1, 0.2, 0.3], 1.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].entity, inside);
    assert_eq!(hits[0].distance, 0.0);
    assert_eq!(hits[0].point, [0.1, 0.2, 0.3]);
}

#[test]
fn closest_points_respect_max_distance() {
    let mut world = World::new();
    let near = spawn_static_box(&mut world, 2.0);
    spawn_static_box(&mut world, 6.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let hits = physics(&world)
        .closest_points([0.0; 3], 2.0, &QueryFilter::new())
        .unwrap();
    assert_eq!(hits.iter().map(|h| h.entity).collect::<Vec<_>>(), [near]);
    let none = physics(&world)
        .closest_points([0.0; 3], 1.0, &QueryFilter::new())
        .unwrap();
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn closest_points_honour_the_filter() {
    let (world, [props, dummy, plain, dummy_far]) = layered_row();
    let at = |filter: &QueryFilter| -> Vec<EntityId> {
        physics(&world)
            .closest_points([0.0; 3], 100.0, filter)
            .unwrap()
            .iter()
            .map(|h| h.entity)
            .collect()
    };
    assert_eq!(at(&QueryFilter::new()), [props, dummy, plain, dummy_far]);
    assert_eq!(
        at(&QueryFilter::new().exclude(props)),
        [dummy, plain, dummy_far]
    );
    assert_eq!(
        at(&QueryFilter::new().ignore_layer("dummy")),
        [props, plain]
    );
    assert_eq!(at(&QueryFilter::new().only_fixed()).len(), 4);
}

#[test]
fn a_long_surface_near_the_point_beats_a_small_ball_with_a_nearer_origin() {
    let mut world = World::new();
    let awning = spawn_collider(
        &mut world,
        [9.0, 0.0, 0.0],
        Shape::Box {
            half_extents: [8.0, 0.1, 0.5],
        },
    );
    let wheel = spawn_collider(&mut world, [0.0, 1.5, 0.0], Shape::Sphere { radius: 0.1 });
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let hits = physics(&world)
        .closest_points([0.0; 3], 5.0, &QueryFilter::new())
        .unwrap();
    // The awning's origin is 9 away, the wheel's 1.5, but the awning's edge is 1.0 from the point.
    assert_eq!(
        hits.iter().map(|h| h.entity).collect::<Vec<_>>(),
        [awning, wheel],
        "{hits:?}"
    );
    assert_eq!(hits[0].distance, 1.0);
    assert!((hits[1].distance - 1.4).abs() < 1e-5, "{hits:?}");
}

#[test]
fn closest_points_reject_bad_input() {
    let mut world = World::new();
    spawn_static_box(&mut world, 2.0);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    let p = physics(&world);
    let f = QueryFilter::new();
    for err in [
        p.closest_points([f32::NAN, 0.0, 0.0], 1.0, &f).unwrap_err(),
        p.closest_points([0.0; 3], -1.0, &f).unwrap_err(),
        p.closest_points([0.0; 3], f32::INFINITY, &f).unwrap_err(),
        p.closest_points([0.0; 3], 1.0, &QueryFilter::new().ignore_layer("nope"))
            .unwrap_err(),
    ] {
        assert!(matches!(err, PhysicsError::QueryInput(_)), "{err:?}");
    }
}
