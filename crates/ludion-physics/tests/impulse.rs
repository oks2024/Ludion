//! Impulses on the wrapper: at-centre, at-point and radial calls queued on the `PhysicsWorld`
//! resource and applied by the write system just before the step — in call order, on `rigid`
//! bodies only, waking sleepers, with no `Velocity` component required — and the blast hits they
//! leave, which ride the snapshot.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    Blast, BlastHit, CollisionLayers, Falloff, PhysicsError, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter, install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn zero_gravity() -> PhysicsSettings {
    PhysicsSettings {
        gravity: [0.0; 3],
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn schedule_with(settings: PhysicsSettings) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, settings);
    schedule
}

fn free_schedule() -> Schedule {
    schedule_with(zero_gravity())
}

fn collider(shape: Shape, body: Body, layer: Option<&str>) -> Collider {
    Collider {
        shape,
        body,
        layer: layer.map(str::to_string),
        sensor: false,
    }
}

fn sphere(radius: f32) -> Shape {
    Shape::Sphere { radius }
}

fn cube(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

fn rigid(mass: f32) -> Body {
    Body::Rigid(RigidBody {
        mass,
        ..RigidBody::default()
    })
}

/// A body of `shape` at `at`, with a `Velocity` component only when `with_velocity`.
fn spawn(
    world: &mut World,
    shape: Shape,
    body: Body,
    at: [f32; 3],
    with_velocity: bool,
) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider(shape, body, None))
        .expect("a fresh entity accepts a collider");
    if with_velocity {
        world
            .insert(entity, Velocity::default())
            .expect("a fresh entity accepts a velocity");
    }
    entity
}

fn physics(world: &mut World) -> &mut PhysicsWorld {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity")
}

fn blast(center: [f32; 3], radius: f32, impulse: f32, falloff: Falloff) -> Blast {
    Blast {
        center,
        radius,
        impulse,
        falloff,
        filter: QueryFilter::new(),
    }
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "expected {expected}, got {actual}"
    );
}

fn close3(actual: [f32; 3], expected: [f32; 3]) {
    for (a, e) in actual.into_iter().zip(expected) {
        close(a, e);
    }
}

/// Ticks frame 0 so bodies exist and the world has stepped once — what a blast reads.
fn warm(world: &mut World, schedule: &mut Schedule) {
    tick(world, schedule, 0);
}

#[test]
fn an_impulse_at_the_centre_moves_a_body_with_no_velocity_component_and_never_spins_it() {
    let mut world = World::new();
    let bare = spawn(&mut world, sphere(0.5), rigid(2.0), [0.0; 3], false);
    let twin = spawn(&mut world, sphere(0.5), rigid(2.0), [0.0, 5.0, 0.0], true);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    let start = pose_of(&world, bare).translation[0];
    for entity in [bare, twin] {
        physics(&mut world)
            .apply_impulse(entity, [4.0, 0.0, 0.0])
            .expect("a finite impulse queues");
    }
    tick(&mut world, &mut schedule, 1);

    assert!(pose_of(&world, bare).translation[0] > start);
    let velocity = velocity_of(&world, twin);
    close3(velocity.linear, [2.0, 0.0, 0.0]);
    assert_eq!(velocity.angular, [0.0; 3]);
}

#[test]
fn an_impulse_at_a_point_off_the_centre_spins_the_body() {
    let mut world = World::new();
    let body = spawn(&mut world, cube(0.5), rigid(2.0), [0.0; 3], true);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    physics(&mut world)
        .apply_impulse_at_point(body, [4.0, 0.0, 0.0], [0.0, 1.0, 0.0])
        .expect("finite inputs queue");
    tick(&mut world, &mut schedule, 1);

    let velocity = velocity_of(&world, body);
    close3(velocity.linear, [2.0, 0.0, 0.0]);
    assert!(
        velocity.angular[2] < -0.1,
        "an impulse along +x above the centre spins about -z: {:?}",
        velocity.angular
    );
}

/// Three spheres on `+x` whose surfaces sit 3, 1 and 2 from the origin, spawned in that order so
/// entity order and distance order differ, plus a static and a kinematic box inside the radius.
struct Row {
    far: EntityId,
    near: EntityId,
    mid: EntityId,
    fixed: EntityId,
    kinematic: EntityId,
}

fn row(world: &mut World) -> Row {
    let far = spawn(world, sphere(0.5), rigid(1.0), [3.5, 0.0, 0.0], false);
    let near = spawn(world, sphere(0.5), rigid(1.0), [1.5, 0.0, 0.0], false);
    let mid = spawn(world, sphere(0.5), rigid(1.0), [2.5, 0.0, 0.0], false);
    let fixed = spawn(world, cube(0.5), Body::new_static(), [0.0, 2.0, 0.0], false);
    let kinematic = spawn(
        world,
        cube(0.5),
        Body::new_kinematic(),
        [0.0, -2.0, 0.0],
        false,
    );
    Row {
        far,
        near,
        mid,
        fixed,
        kinematic,
    }
}

fn hits_of(world: &World) -> Vec<BlastHit> {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installed a world")
        .blast_hits()
        .to_vec()
}

fn fire(world: &mut World, schedule: &mut Schedule, blast: Blast) -> Vec<BlastHit> {
    warm(world, schedule);
    physics(world)
        .radial_impulse(blast)
        .expect("a valid blast queues");
    tick(world, schedule, 1);
    hits_of(world)
}

#[test]
fn a_constant_blast_hits_only_rigid_bodies_in_entity_order_along_the_line_from_the_centre() {
    let mut world = World::new();
    let row = row(&mut world);
    let mut schedule = free_schedule();
    let static_pose = pose_of(&world, row.fixed);
    let kinematic_pose = pose_of(&world, row.kinematic);

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([0.0; 3], 5.0, 10.0, Falloff::Constant),
    );

    let entities: Vec<EntityId> = hits.iter().map(|hit| hit.entity).collect();
    assert_eq!(entities, vec![row.far, row.near, row.mid]);
    for hit in &hits {
        assert_eq!(hit.blast, 0);
        close3(hit.impulse, [10.0, 0.0, 0.0]);
    }
    assert_eq!(pose_of(&world, row.fixed), static_pose);
    assert_eq!(pose_of(&world, row.kinematic), kinematic_pose);
}

#[test]
fn a_linear_blast_fades_with_the_distance_to_each_surface() {
    let mut world = World::new();
    let row = row(&mut world);
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([0.0; 3], 5.0, 10.0, Falloff::Linear),
    );

    let magnitude = |entity| {
        hits.iter()
            .find(|hit| hit.entity == entity)
            .expect("the body is in the blast")
            .impulse[0]
    };
    close(magnitude(row.near), 10.0 * (1.0 - 1.0 / 5.0));
    close(magnitude(row.mid), 10.0 * (1.0 - 2.0 / 5.0));
    close(magnitude(row.far), 10.0 * (1.0 - 3.0 / 5.0));
    assert!(magnitude(row.near) > magnitude(row.mid) && magnitude(row.mid) > magnitude(row.far));
    let near = hits.iter().find(|hit| hit.entity == row.near).unwrap();
    close3(near.point, [1.0, 0.0, 0.0]);
}

#[test]
fn an_inverse_square_blast_scales_by_the_clamped_distance() {
    let mut world = World::new();
    let row = row(&mut world);
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast(
            [0.0; 3],
            5.0,
            10.0,
            Falloff::InverseSquare { min_distance: 1.0 },
        ),
    );

    let magnitude = |entity| {
        hits.iter()
            .find(|hit| hit.entity == entity)
            .expect("the body is in the blast")
            .impulse[0]
    };
    close(magnitude(row.near), 10.0);
    close(magnitude(row.mid), 10.0 / 4.0);
    close(magnitude(row.far), 10.0 / 9.0);
}

#[test]
fn an_off_centre_blast_spins_a_long_body() {
    let mut world = World::new();
    let plank = spawn(
        &mut world,
        Shape::Box {
            half_extents: [2.0, 0.25, 0.25],
        },
        rigid(1.0),
        [0.0; 3],
        true,
    );
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([1.5, 1.5, 0.0], 3.0, 4.0, Falloff::Constant),
    );

    assert_eq!(hits.len(), 1);
    close3(hits[0].point, [1.5, 0.25, 0.0]);
    let velocity = velocity_of(&world, plank);
    assert!(velocity.linear[1] < -0.1, "{:?}", velocity.linear);
    assert!(velocity.angular[2].abs() > 0.1, "{:?}", velocity.angular);
}

#[test]
fn a_blast_wakes_sleeping_bodies() {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(0.5), rigid(1.0), [0.0; 3], true);
    let mut schedule = free_schedule();

    let mut frame = 0;
    while physics_sleeping(&mut world, &mut schedule, ball, frame) != Some(true) {
        frame += 1;
        assert!(frame < 200, "the resting ball never fell asleep");
    }
    physics(&mut world)
        .radial_impulse(blast([-2.0, 0.0, 0.0], 3.0, 4.0, Falloff::Constant))
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, frame + 1);

    assert_eq!(physics(&mut world).is_sleeping(ball), Some(false));
    assert!(velocity_of(&world, ball).linear[0] > 1.0);
}

/// Ticks `frame`, then reads `entity`'s sleep state.
fn physics_sleeping(
    world: &mut World,
    schedule: &mut Schedule,
    entity: EntityId,
    frame: u64,
) -> Option<bool> {
    tick(world, schedule, frame);
    physics(world).is_sleeping(entity)
}

#[test]
fn a_centre_inside_a_body_pushes_it_away_from_its_own_centre_of_mass() {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(1.0), rigid(1.0), [0.0; 3], true);
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([0.3, 0.0, 0.0], 2.0, 10.0, Falloff::Linear),
    );

    assert_eq!(hits.len(), 1);
    close3(hits[0].impulse, [-10.0, 0.0, 0.0]);
    close3(hits[0].point, [0.3, 0.0, 0.0]);
    assert!(velocity_of(&world, ball).linear[0] < -1.0);
}

#[test]
fn a_centre_exactly_at_the_centre_of_mass_hits_nothing() {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(1.0), rigid(1.0), [0.0; 3], true);
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([0.0; 3], 2.0, 10.0, Falloff::Constant),
    );

    assert!(hits.is_empty());
    assert_eq!(velocity_of(&world, ball).linear, [0.0; 3]);
}

fn declared_layers() -> CollisionLayers {
    CollisionLayers::new(
        vec!["default".to_string(), "debris".to_string()],
        vec![0, 0],
    )
}

#[test]
fn the_filter_drops_an_excluded_entity_and_an_ignored_layer() {
    let mut world = World::new();
    let mut spawn_on = |layer: Option<&str>, x: f32| {
        let entity = world.spawn();
        world.insert(entity, TestPose::at([x, 0.0, 0.0])).unwrap();
        world
            .insert(entity, collider(sphere(0.4), rigid(1.0), layer))
            .unwrap();
        entity
    };
    let a = spawn_on(None, 1.0);
    let b = spawn_on(Some("debris"), 2.0);
    let c = spawn_on(None, 3.0);
    let mut schedule = schedule_with(PhysicsSettings {
        layers: Some(declared_layers()),
        ..zero_gravity()
    });
    warm(&mut world, &mut schedule);

    let mut skip_debris = blast([0.0; 3], 5.0, 1.0, Falloff::Constant);
    skip_debris.filter = QueryFilter::new().ignore_layer("debris");
    let mut skip_c = blast([0.0; 3], 5.0, 1.0, Falloff::Constant);
    skip_c.filter = QueryFilter::new().exclude(c);
    physics(&mut world).radial_impulse(skip_debris).unwrap();
    physics(&mut world).radial_impulse(skip_c).unwrap();
    tick(&mut world, &mut schedule, 1);

    let hits: Vec<(u32, EntityId)> = hits_of(&world)
        .iter()
        .map(|hit| (hit.blast, hit.entity))
        .collect();
    assert_eq!(hits, vec![(0, a), (0, c), (1, a), (1, b)]);
}

#[test]
fn a_bad_call_is_a_query_input_error_and_queues_nothing() {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(0.5), rigid(1.0), [1.0, 0.0, 0.0], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    let nan = f32::NAN;
    let mut bad_layer = blast([0.0; 3], 3.0, 1.0, Falloff::Constant);
    bad_layer.filter = QueryFilter::new().ignore_layer("dumy");
    let blasts = vec![
        blast([nan, 0.0, 0.0], 3.0, 1.0, Falloff::Constant),
        blast([0.0; 3], f32::INFINITY, 1.0, Falloff::Constant),
        blast([0.0; 3], 0.0, 1.0, Falloff::Constant),
        blast([0.0; 3], -1.0, 1.0, Falloff::Constant),
        blast([0.0; 3], 3.0, nan, Falloff::Constant),
        blast(
            [0.0; 3],
            3.0,
            1.0,
            Falloff::InverseSquare { min_distance: 0.0 },
        ),
        blast(
            [0.0; 3],
            3.0,
            1.0,
            Falloff::InverseSquare { min_distance: nan },
        ),
        bad_layer,
    ];
    let world_physics = physics(&mut world);
    for bad in blasts {
        let result = world_physics.radial_impulse(bad.clone());
        assert!(
            matches!(result, Err(PhysicsError::QueryInput(_))),
            "{bad:?} gave {result:?}"
        );
    }
    assert!(matches!(
        world_physics.apply_impulse(ball, [nan, 0.0, 0.0]),
        Err(PhysicsError::QueryInput(_))
    ));
    assert!(matches!(
        world_physics.apply_impulse_at_point(ball, [1.0, 0.0, 0.0], [0.0, f32::NEG_INFINITY, 0.0]),
        Err(PhysicsError::QueryInput(_))
    ));
    assert!(world_physics.to_bytes().is_ok(), "nothing was queued");
    assert_eq!(
        world_physics
            .radial_impulse(blast([0.0; 3], 3.0, 1.0, Falloff::Constant))
            .unwrap(),
        0,
        "a rejected call does not take a blast index"
    );
}

#[test]
fn a_call_naming_a_body_that_cannot_take_an_impulse_is_dropped_and_reported_once() {
    let mut world = World::new();
    let fixed = spawn(&mut world, cube(0.5), Body::new_static(), [0.0; 3], false);
    let gone = spawn(&mut world, sphere(0.5), rigid(1.0), [5.0, 0.0, 0.0], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);
    world.despawn(gone).expect("a live entity despawns");

    let fixed_pose = pose_of(&world, fixed);
    physics(&mut world)
        .apply_impulse(fixed, [4.0, 0.0, 0.0])
        .unwrap();
    physics(&mut world)
        .apply_impulse_at_point(gone, [4.0, 0.0, 0.0], [5.0, 1.0, 0.0])
        .unwrap();
    tick(&mut world, &mut schedule, 1);

    assert_eq!(pose_of(&world, fixed), fixed_pose);
    let misses = physics(&mut world).take_impulse_misses();
    let entities: Vec<EntityId> = misses.iter().map(|miss| miss.entity).collect();
    assert_eq!(entities, vec![fixed, gone]);
    assert!(
        misses[0].reason.contains("static or kinematic"),
        "{misses:?}"
    );
    assert!(misses[1].reason.contains("no rigid body"), "{misses:?}");
    assert!(physics(&mut world).take_impulse_misses().is_empty());
}

#[test]
fn the_hits_last_one_tick() {
    let mut world = World::new();
    let row = row(&mut world);
    let mut schedule = free_schedule();

    let hits = fire(
        &mut world,
        &mut schedule,
        blast([0.0; 3], 5.0, 1.0, Falloff::Constant),
    );
    assert_eq!(hits.len(), 3);
    let _ = row;

    tick(&mut world, &mut schedule, 2);
    assert!(hits_of(&world).is_empty());
}

#[test]
fn a_body_spawned_into_a_despawned_bodys_slot_is_not_hit_until_the_next_tick() {
    let mut world = World::new();
    let old = spawn(&mut world, sphere(0.5), rigid(1.0), [1.5, 0.0, 0.0], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    world.despawn(old).expect("a live entity despawns");
    let new = spawn(&mut world, sphere(0.5), rigid(1.0), [1.5, 0.0, 0.0], false);
    let point_blank = blast([0.0; 3], 5.0, 1.0, Falloff::Constant);
    physics(&mut world)
        .radial_impulse(point_blank.clone())
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 1);
    assert!(hits_of(&world).is_empty());

    physics(&mut world)
        .radial_impulse(point_blank)
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 2);
    let entities: Vec<EntityId> = hits_of(&world).iter().map(|hit| hit.entity).collect();
    assert_eq!(entities, vec![new]);
}

#[test]
fn a_rebuilt_body_is_not_hit_until_the_next_tick() {
    let mut world = World::new();
    let body = spawn(&mut world, sphere(0.5), rigid(1.0), [1.5, 0.0, 0.0], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    let edited = world
        .get_mut::<Collider>(body)
        .expect("a live entity")
        .expect("the entity carries a collider");
    let Body::Rigid(tunables) = &mut edited.body else {
        panic!("the body is rigid");
    };
    tunables.mass = 2.0;
    let point_blank = blast([0.0; 3], 5.0, 1.0, Falloff::Constant);
    physics(&mut world)
        .radial_impulse(point_blank.clone())
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 1);
    assert!(hits_of(&world).is_empty());

    physics(&mut world)
        .radial_impulse(point_blank)
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 2);
    assert_eq!(hits_of(&world).len(), 1);
}

#[test]
fn several_blasts_in_one_tick_number_from_zero_and_group_in_call_order() {
    let mut world = World::new();
    let near = spawn(&mut world, sphere(0.5), rigid(1.0), [1.5, 0.0, 0.0], false);
    let far = spawn(&mut world, sphere(0.5), rigid(1.0), [4.5, 0.0, 0.0], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    let wide = physics(&mut world)
        .radial_impulse(blast([0.0; 3], 6.0, 1.0, Falloff::Constant))
        .unwrap();
    let small = physics(&mut world)
        .radial_impulse(blast([0.0; 3], 2.0, 1.0, Falloff::Constant))
        .unwrap();
    assert_eq!((wide, small), (0, 1));
    tick(&mut world, &mut schedule, 1);

    let hits: Vec<(u32, EntityId)> = hits_of(&world)
        .iter()
        .map(|hit| (hit.blast, hit.entity))
        .collect();
    assert_eq!(hits, vec![(0, near), (0, far), (1, near)]);
}

#[test]
fn a_snapshot_refuses_a_world_with_queued_impulses_until_they_are_applied() {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(0.5), rigid(1.0), [0.0; 3], false);
    let mut schedule = free_schedule();
    warm(&mut world, &mut schedule);

    physics(&mut world)
        .apply_impulse(ball, [1.0, 0.0, 0.0])
        .unwrap();
    match physics(&mut world).to_bytes() {
        Err(PhysicsError::Encode(message)) => {
            assert!(message.contains("1 impulse calls are queued"), "{message}");
        }
        other => panic!("expected an Encode error, got {other:?}"),
    }
    tick(&mut world, &mut schedule, 1);
    assert!(physics(&mut world).to_bytes().is_ok());
}

/// The scene the snapshot test runs: a floor and a scatter of boxes under gravity, blasted every
/// [`BLAST_EVERY`] ticks from a centre that walks along `x`, and one box nudged at a point on the
/// same cadence.
const BLAST_EVERY: u64 = 25;
const TICKS: u64 = 150;
const SNAPSHOT_TICK: u64 = 50;

fn fire_on_cadence(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame == 0 || !frame.is_multiple_of(BLAST_EVERY) {
        return;
    }
    let target = world
        .entities()
        .nth(1)
        .expect("the scene has a floor and boxes");
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let x = (frame / BLAST_EVERY) as f32 - 3.0;
    physics
        .radial_impulse(blast([x, 0.2, 0.0], 4.0, 12.0, Falloff::Linear))
        .expect("a valid blast queues");
    physics
        .apply_impulse_at_point(target, [3.0, 4.0, 0.0], [0.0, 0.5, 0.2])
        .expect("finite inputs queue");
}

fn scatter(world: &mut World) {
    spawn(
        world,
        Shape::Box {
            half_extents: [10.0, 0.5, 10.0],
        },
        Body::new_static(),
        [0.0, -0.5, 0.0],
        false,
    );
    for i in 0..6 {
        let shape = if i % 2 == 0 { cube(0.4) } else { sphere(0.4) };
        spawn(
            world,
            shape,
            rigid(1.0 + i as f32 * 0.5),
            [i as f32 - 3.0, 0.4, (i % 3) as f32 * 0.3],
            true,
        );
    }
}

fn cadence_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    schedule.add_system("fire_on_cadence", fire_on_cadence);
    install::<TestPose>(&mut schedule, PhysicsSettings::new(PhysicsSpace::Free));
    schedule
}

type Frame = (Vec<(TestPose, Velocity)>, Vec<BlastHit>);

fn frame_state(world: &World) -> Frame {
    let bodies = world
        .entities()
        .map(|entity| (pose_of(world, entity), velocity_or_zero(world, entity)))
        .collect();
    (bodies, hits_of(world))
}

fn velocity_or_zero(world: &World, entity: EntityId) -> Velocity {
    world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .copied()
        .unwrap_or_default()
}

struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Option<Collider>>,
    poses: Vec<TestPose>,
    velocities: Vec<Option<Velocity>>,
    bytes: Vec<u8>,
}

fn capture(world: &mut World) -> Capture {
    let entities: Vec<EntityId> = world.entities().collect();
    let bytes = physics(world).to_bytes().expect("the solver state encodes");
    Capture {
        colliders: entities
            .iter()
            .map(|&e| world.get::<Collider>(e).expect("a live entity").cloned())
            .collect(),
        poses: entities.iter().map(|&e| pose_of(world, e)).collect(),
        velocities: entities
            .iter()
            .map(|&e| world.get::<Velocity>(e).expect("a live entity").copied())
            .collect(),
        entities,
        bytes,
    }
}

fn restore(captured: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&captured.entities)
        .expect("captured live set is valid");
    for (i, &entity) in captured.entities.iter().enumerate() {
        if let Some(collider) = &captured.colliders[i] {
            world.insert(entity, collider.clone()).expect("collider");
        }
        world.insert(entity, captured.poses[i]).expect("pose");
        if let Some(velocity) = captured.velocities[i] {
            world.insert(entity, velocity).expect("velocity");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&captured.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

#[test]
fn a_snapshot_taken_right_after_a_blast_resumes_bit_identically_with_its_hits() {
    let mut world = World::new();
    scatter(&mut world);
    let mut schedule = cadence_schedule();
    let mut reference: Vec<Frame> = Vec::new();
    let mut captured = None;
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        reference.push(frame_state(&world));
        if frame == SNAPSHOT_TICK {
            assert!(
                !hits_of(&world).is_empty(),
                "the snapshot tick is a blast tick"
            );
            captured = Some(capture(&mut world));
        }
    }
    let captured = captured.expect("the loop reaches the snapshot tick");
    let moved: std::collections::BTreeSet<[u32; 3]> = reference
        .iter()
        .map(|(bodies, _)| bodies[2].0.translation.map(f32::to_bits))
        .collect();
    assert!(moved.len() > 50, "the scene stays live: {}", moved.len());

    let mut world = restore(&captured);
    let mut schedule = cadence_schedule();
    for frame in (SNAPSHOT_TICK + 1)..TICKS {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(
            frame_state(&world),
            reference[frame as usize],
            "the resumed run diverges at tick {frame}"
        );
    }
}
