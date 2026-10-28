//! Sensor colliders: built as rapier sensors that never enter the contact graph, skipped by every
//! query unless the filter opts in, and refused on a rigid body or a surface-only shape.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par::with_threads;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CollisionLayers, ExitCause, PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter,
    SensorEvent, SensorPhase, install, teleport_kinematic,
};
use ludion_spatial::{Body, Collider, CompoundPart, RigidBody, Shape};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn cube(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

fn collider(shape: Shape, body: Body, sensor: bool) -> Collider {
    Collider {
        shape,
        body,
        layer: None,
        sensor,
    }
}

fn spawn(world: &mut World, at: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    entity
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("a body claimed the world")
}

fn set_sensor(world: &mut World, entity: EntityId, sensor: bool) {
    world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider")
        .sensor = sensor;
}

/// A rigid ball dropped from above a static slab, with a sensor box around its path or without one.
fn drop_through(with_sensor: bool) -> (World, Option<EntityId>, EntityId) {
    let mut world = World::new();
    let mut sensor = None;
    if with_sensor {
        sensor = Some(spawn(
            &mut world,
            [0.0, 2.0, 0.0],
            collider(cube(1.0), Body::new_static(), true),
        ));
    }
    let ball = spawn(
        &mut world,
        [0.0, 5.0, 0.0],
        collider(
            Shape::Sphere { radius: 0.25 },
            Body::Rigid(RigidBody::default()),
            false,
        ),
    );
    (world, sensor, ball)
}

#[test]
fn a_rigid_ball_falls_through_a_sensor_as_if_it_were_not_there() {
    let (mut with, sensor, with_ball) = drop_through(true);
    let (mut without, _, without_ball) = drop_through(false);
    let sensor = sensor.expect("the sensor was spawned");
    let mut with_schedule = schedule();
    let mut without_schedule = schedule();
    for frame in 0..90 {
        tick(&mut with, &mut with_schedule, frame);
        tick(&mut without, &mut without_schedule, frame);
        let touched = physics(&with)
            .contact_events()
            .iter()
            .any(|event| event.a == sensor || event.b == sensor);
        assert!(!touched, "tick {frame}: a sensor entered the contact graph");
    }
    let (a, b) = (pose_of(&with, with_ball), pose_of(&without, without_ball));
    assert_eq!(a.translation, b.translation);
    assert_eq!(a.rotation, b.rotation);
    assert!(
        a.translation[1] < 1.0,
        "the ball must have passed the sensor"
    );
}

/// A sensor box at x = 2 between the origin and a solid box at x = 6.
fn lined_up() -> (World, Schedule, EntityId, EntityId) {
    let mut world = World::new();
    let sensor = spawn(
        &mut world,
        [2.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), true),
    );
    let solid = spawn(
        &mut world,
        [6.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), false),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    (world, schedule, sensor, solid)
}

#[test]
fn queries_skip_sensors_unless_the_filter_includes_them() {
    let (world, _, sensor, solid) = lined_up();
    let physics = physics(&world);
    let plain = QueryFilter::new();
    let opted_in = QueryFilter::new().include_sensors();
    let probe = cube(0.25);

    let ray = |filter: &QueryFilter| -> Vec<EntityId> {
        physics
            .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, filter)
            .expect("a finite ray")
            .iter()
            .map(|hit| hit.entity)
            .collect()
    };
    assert_eq!(ray(&plain), vec![solid]);
    assert_eq!(ray(&opted_in), vec![sensor, solid]);

    let cast = |filter: &QueryFilter| -> Vec<EntityId> {
        physics
            .shape_cast(&probe, [0.0; 3], IDENTITY, [1.0, 0.0, 0.0], 100.0, filter)
            .expect("a finite cast")
            .iter()
            .map(|hit| hit.entity)
            .collect()
    };
    assert_eq!(cast(&plain), vec![solid]);
    assert_eq!(cast(&opted_in), vec![sensor, solid]);

    let wide = cube(10.0);
    let overlap = |filter: &QueryFilter| -> Vec<EntityId> {
        let mut found = physics
            .overlap(&wide, [4.0, 0.0, 0.0], IDENTITY, filter)
            .expect("a finite probe");
        found.sort();
        found
    };
    assert_eq!(overlap(&plain), vec![solid]);
    let mut both = vec![sensor, solid];
    both.sort();
    assert_eq!(overlap(&opted_in), both);
}

#[test]
fn a_rigid_sensor_or_a_heightfield_sensor_gets_no_body() {
    let mut world = World::new();
    let rigid = spawn(
        &mut world,
        [0.0; 3],
        collider(cube(0.5), Body::Rigid(RigidBody::default()), true),
    );
    let heightfield = spawn(
        &mut world,
        [5.0, 0.0, 0.0],
        collider(
            Shape::Heightfield {
                rows: 2,
                cols: 2,
                heights: vec![0.0; 4],
                scale: [1.0; 3],
            },
            Body::new_static(),
            true,
        ),
    );
    let ok = spawn(
        &mut world,
        [10.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), true),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);

    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the valid sensor claimed the world");
    assert!(physics.shape_kind(rigid).is_none());
    assert!(physics.shape_kind(heightfield).is_none());
    assert!(physics.shape_kind(ok).is_some());
    let rejections = physics.take_rejections();
    let codes: Vec<_> = rejections.iter().map(|r| (r.entity, r.code)).collect();
    assert_eq!(
        codes,
        vec![
            (rigid, code::COLLIDER_SENSOR_ON_RIGID),
            (heightfield, code::COLLIDER_SENSOR_NO_INTERIOR),
        ],
        "{rejections:?}"
    );
}

#[test]
fn toggling_sensor_on_a_live_body_rebuilds_it() {
    let mut world = World::new();
    let crate_ = spawn(
        &mut world,
        [3.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_static(), false),
    );
    let mut schedule = schedule();
    tick(&mut world, &mut schedule, 0);
    let hits = |world: &World| {
        physics(world)
            .ray_cast([0.0; 3], [1.0, 0.0, 0.0], 100.0, &QueryFilter::new())
            .expect("a finite ray")
            .len()
    };
    assert_eq!(hits(&world), 1);

    set_sensor(&mut world, crate_, true);
    tick(&mut world, &mut schedule, 1);
    assert_eq!(hits(&world), 0, "the rebuilt body is a sensor");

    set_sensor(&mut world, crate_, false);
    tick(&mut world, &mut schedule, 2);
    assert_eq!(hits(&world), 1, "toggling back restores the hit");
}

// ---- Events ----------------------------------------------------------------------------------

const ENTERED: SensorPhase = SensorPhase::Entered;

fn exited(cause: ExitCause) -> SensorPhase {
    SensorPhase::Exited { cause }
}

fn event(sensor: EntityId, other: EntityId, phase: SensorPhase) -> SensorEvent {
    SensorEvent {
        sensor,
        other,
        phase,
    }
}

/// What a game system saw each frame; it runs before the physics triple, so entry `F` is what
/// tick `F - 1` produced.
#[derive(Default)]
struct Log(Vec<Vec<SensorEvent>>);

fn record(world: &mut World, _ctx: &SystemCtx) {
    let events = world
        .resource::<PhysicsWorld>()
        .map(|physics| physics.sensor_events().to_vec())
        .unwrap_or_default();
    let mut log = world.remove_resource::<Log>().unwrap_or_default();
    log.0.push(events);
    world.insert_resource(log);
}

fn recording_schedule(settings: PhysicsSettings) -> Schedule {
    let mut schedule = Schedule::new();
    schedule.add_system("record", record);
    install::<TestPose>(&mut schedule, settings);
    schedule
}

fn free() -> PhysicsSettings {
    PhysicsSettings::new(PhysicsSpace::Free)
}

/// A world, the recording schedule stepping it and the next frame index.
struct Rig {
    world: World,
    schedule: Schedule,
    frame: u64,
}

impl Rig {
    fn new(world: World) -> Rig {
        Rig::with(world, free())
    }

    fn with(world: World, settings: PhysicsSettings) -> Rig {
        Rig {
            world,
            schedule: recording_schedule(settings),
            frame: 0,
        }
    }

    /// Runs one tick and returns the events its step produced.
    fn tick(&mut self) -> Vec<SensorEvent> {
        tick(&mut self.world, &mut self.schedule, self.frame);
        self.frame += 1;
        physics(&self.world).sensor_events().to_vec()
    }

    fn log(&self) -> &[Vec<SensorEvent>] {
        &self
            .world
            .resource::<Log>()
            .expect("record always inserts one")
            .0
    }

    fn physics(&mut self) -> &mut PhysicsWorld {
        self.world
            .resource_mut::<PhysicsWorld>()
            .expect("a body claimed the world")
    }

    fn run_until_event(&mut self, limit: u32) -> Vec<SensorEvent> {
        for _ in 0..limit {
            let events = self.tick();
            if !events.is_empty() {
                return events;
            }
        }
        panic!("no sensor event within {limit} ticks");
    }
}

fn move_to(world: &mut World, entity: EntityId, at: [f32; 3]) {
    world
        .get_mut::<TestPose>(entity)
        .expect("a live entity")
        .expect("its pose")
        .translation = at;
}

fn kinematic_sphere(world: &mut World, at: [f32; 3]) -> EntityId {
    spawn(
        world,
        at,
        collider(Shape::Sphere { radius: 0.25 }, Body::new_kinematic(), false),
    )
}

const FAR: [f32; 3] = [10.0, 0.0, 0.0];

/// A static unit-half sensor at the origin and a kinematic sphere far away.
fn zone_and_visitor() -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [0.0; 3],
        collider(cube(1.0), Body::new_static(), true),
    );
    let visitor = kinematic_sphere(&mut world, FAR);
    (world, zone, visitor)
}

#[test]
fn a_body_written_inside_a_sensor_enters_on_that_ticks_step_and_is_read_the_next_tick() {
    let (world, zone, visitor) = zone_and_visitor();
    let mut rig = Rig::new(world);
    for _ in 0..10 {
        assert!(rig.tick().is_empty());
    }
    move_to(&mut rig.world, visitor, [0.0; 3]);
    assert_eq!(rig.tick(), vec![event(zone, visitor, ENTERED)]);
    assert_eq!(physics(&rig.world).sensor_occupants(zone), [visitor]);
    assert!(rig.tick().is_empty());
    for _ in 0..8 {
        rig.tick();
    }
    move_to(&mut rig.world, visitor, FAR);
    let out = rig.tick();
    assert_eq!(
        out,
        vec![event(zone, visitor, exited(ExitCause::Separated))]
    );
    assert!(physics(&rig.world).sensor_occupants(zone).is_empty());
    rig.tick();

    let log = rig.log();
    assert!(log[10].is_empty(), "frame 10's reader sees step 9");
    assert_eq!(log[11], vec![event(zone, visitor, ENTERED)]);
    assert!(log[12].is_empty());
    assert_eq!(log[21], out);
}

#[test]
fn a_rigid_ball_falling_through_a_sensor_enters_then_exits() {
    let (world, sensor, ball) = drop_through(true);
    let sensor = sensor.expect("the sensor was spawned");
    let mut rig = Rig::new(world);
    let mut seen = Vec::new();
    for _ in 0..90 {
        seen.extend(rig.tick());
    }
    assert_eq!(
        seen,
        vec![
            event(sensor, ball, ENTERED),
            event(sensor, ball, exited(ExitCause::Separated)),
        ]
    );
}

#[test]
fn two_kinematic_bodies_moved_towards_each_other_enter_then_exit() {
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [-5.0, 0.0, 0.0],
        collider(cube(0.5), Body::new_kinematic(), true),
    );
    let visitor = kinematic_sphere(&mut world, [5.0, 0.0, 0.0]);
    let mut rig = Rig::new(world);
    let mut seen = Vec::new();
    for step in 0..100 {
        let shift = 0.1 * step as f32;
        move_to(&mut rig.world, zone, [-5.0 + shift, 0.0, 0.0]);
        move_to(&mut rig.world, visitor, [5.0 - shift, 0.0, 0.0]);
        seen.extend(rig.tick());
    }
    assert_eq!(
        seen,
        vec![
            event(zone, visitor, ENTERED),
            event(zone, visitor, exited(ExitCause::Separated)),
        ]
    );
}

#[test]
fn overlapping_sensors_never_report_each_other() {
    let mut world = World::new();
    let a = spawn(
        &mut world,
        [0.0; 3],
        collider(cube(1.0), Body::new_static(), true),
    );
    let b = spawn(
        &mut world,
        [0.5, 0.0, 0.0],
        collider(cube(1.0), Body::new_kinematic(), true),
    );
    let mut rig = Rig::new(world);
    for _ in 0..10 {
        assert!(rig.tick().is_empty());
    }
    assert!(physics(&rig.world).sensor_occupants(a).is_empty());
    assert!(physics(&rig.world).sensor_occupants(b).is_empty());
}

fn layers(ignoring: bool) -> CollisionLayers {
    let (a, b) = if ignoring { (0b100, 0b010) } else { (0, 0) };
    CollisionLayers::new(
        vec!["default".into(), "a".into(), "b".into()],
        vec![0, a, b],
    )
}

fn layered(layer: &str, collider: Collider) -> Collider {
    Collider {
        layer: Some(layer.to_string()),
        ..collider
    }
}

fn layered_pair() -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [0.0; 3],
        layered("a", collider(cube(1.0), Body::new_static(), true)),
    );
    let visitor = spawn(
        &mut world,
        [0.0; 3],
        layered(
            "b",
            collider(Shape::Sphere { radius: 0.25 }, Body::new_kinematic(), false),
        ),
    );
    (world, zone, visitor)
}

fn settings_with(table: CollisionLayers) -> PhysicsSettings {
    PhysicsSettings {
        layers: Some(table),
        ..free()
    }
}

#[test]
fn a_pair_whose_layers_ignore_each_other_never_reports() {
    let (world, _, _) = layered_pair();
    let mut rig = Rig::with(world, settings_with(layers(true)));
    for _ in 0..5 {
        assert!(rig.tick().is_empty());
    }
}

#[test]
fn a_live_layer_edit_that_ignores_an_occupied_pair_exits_it_as_removed() {
    let (world, zone, visitor) = layered_pair();
    let mut rig = Rig::with(world, settings_with(layers(false)));
    assert_eq!(rig.tick(), vec![event(zone, visitor, ENTERED)]);
    rig.schedule = recording_schedule(settings_with(layers(true)));
    assert_eq!(
        rig.run_until_event(3),
        vec![event(zone, visitor, exited(ExitCause::Removed))]
    );
}

fn occupied(visitors: usize) -> (Rig, EntityId, Vec<EntityId>) {
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [0.0; 3],
        collider(cube(2.0), Body::new_static(), true),
    );
    let crowd: Vec<_> = (0..visitors)
        .map(|n| kinematic_sphere(&mut world, [n as f32 - 1.0, 0.0, 0.0]))
        .collect();
    let mut rig = Rig::new(world);
    assert_eq!(rig.tick().len(), visitors);
    (rig, zone, crowd)
}

fn exits_of(zone: EntityId, crowd: &[EntityId], cause: ExitCause) -> Vec<SensorEvent> {
    let mut expected: Vec<_> = crowd
        .iter()
        .map(|&other| event(zone, other, exited(cause)))
        .collect();
    expected.sort_by_key(|e| e.other);
    expected
}

#[test]
fn a_visitor_despawned_inside_exits_as_despawned_naming_the_dead_id() {
    let (mut rig, zone, crowd) = occupied(1);
    rig.world.despawn(crowd[0]).expect("a live entity");
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Despawned));
    assert!(physics(&rig.world).sensor_occupants(zone).is_empty());
}

#[test]
fn a_despawned_sensor_exits_each_of_its_occupants_sorted() {
    let (mut rig, zone, crowd) = occupied(3);
    rig.world.despawn(zone).expect("a live entity");
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Despawned));
}

#[test]
fn a_removed_collider_or_a_toggled_sensor_exits_as_removed() {
    let (mut rig, zone, crowd) = occupied(1);
    rig.world
        .remove::<Collider>(crowd[0])
        .expect("a live entity");
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Removed));

    let (mut rig, zone, crowd) = occupied(3);
    set_sensor(&mut rig.world, zone, false);
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Removed));
}

#[test]
fn a_teleported_visitor_exits_as_separated() {
    let (mut rig, zone, crowd) = occupied(1);
    move_to(&mut rig.world, crowd[0], FAR);
    teleport_kinematic(&mut rig.world, crowd[0]);
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Separated));
}

#[test]
fn a_visitor_present_at_the_first_step_or_spawned_inside_later_enters_on_its_first_step() {
    let (world, zone, first) = zone_and_visitor();
    let mut rig = Rig::new(world);
    move_to(&mut rig.world, first, [0.0; 3]);
    assert_eq!(rig.tick(), vec![event(zone, first, ENTERED)]);
    let later = kinematic_sphere(&mut rig.world, [0.2, 0.0, 0.0]);
    assert_eq!(rig.tick(), vec![event(zone, later, ENTERED)]);
}

#[test]
fn events_are_sorted_with_no_duplicate_pair_whatever_the_spawn_order() {
    // Sensors at x = 0, 3, 6 (half 1.2) and four visitors over them; the spawn order is shuffled.
    let order = [4, 0, 6, 2, 5, 1, 3];
    let mut world = World::new();
    for item in order {
        if item < 3 {
            spawn(
                &mut world,
                [3.0 * item as f32, 0.0, 0.0],
                collider(cube(1.2), Body::new_static(), true),
            );
        } else {
            let x = [0.5, 2.0, 3.0, 5.0][item - 3];
            kinematic_sphere(&mut world, [x, 0.0, 0.0]);
        }
    }
    let mut rig = Rig::new(world);
    let events = rig.tick();
    assert!(events.len() >= 4, "{events:?}");
    let mut sorted = events.clone();
    sorted.sort_by_key(|e| (e.sensor, e.other));
    sorted.dedup_by_key(|e| (e.sensor, e.other));
    assert_eq!(events, sorted);
}

/// Every transition kind: a kinematic visitor that comes and goes, and a rigid ball dropped
/// through the zone.
fn scenario() -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [0.0, 2.0, 0.0],
        collider(cube(1.0), Body::new_static(), true),
    );
    spawn(
        &mut world,
        [0.0, 0.0, 0.0],
        collider(
            Shape::Box {
                half_extents: [5.0, 0.2, 5.0],
            },
            Body::new_static(),
            false,
        ),
    );
    spawn(
        &mut world,
        [0.0, 5.0, 0.0],
        collider(
            Shape::Sphere { radius: 0.25 },
            Body::Rigid(RigidBody::default()),
            false,
        ),
    );
    let visitor = kinematic_sphere(&mut world, FAR);
    (world, zone, visitor)
}

/// Drives the scenario's kinematic visitor in at frame 5 and out at frame 20.
fn drive(rig: &mut Rig, visitor: EntityId, until: u64) -> Vec<Vec<SensorEvent>> {
    let mut stream = Vec::new();
    while rig.frame < until {
        match rig.frame {
            5 => move_to(&mut rig.world, visitor, [0.0, 2.0, 0.0]),
            20 => move_to(&mut rig.world, visitor, FAR),
            _ => {}
        }
        stream.push(rig.tick());
    }
    stream
}

#[test]
fn a_snapshot_taken_mid_occupancy_continues_with_the_same_events() {
    let (world_a, zone, visitor) = scenario();
    let (world_b, _, _) = scenario();
    let mut a = Rig::new(world_a);
    let mut b = Rig::new(world_b);
    let mut stream_a = drive(&mut a, visitor, 12);
    let mut stream_b = drive(&mut b, visitor, 12);
    assert_eq!(stream_a, stream_b);
    assert_eq!(a.physics().sensor_occupants(zone), [visitor]);

    let bytes = b.physics().to_bytes().expect("the solver state encodes");
    b.world.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the bytes just written decode"),
    );
    assert_eq!(b.physics().sensor_occupants(zone), [visitor]);
    stream_a.extend(drive(&mut a, visitor, 90));
    stream_b.extend(drive(&mut b, visitor, 90));
    assert_eq!(stream_a, stream_b);
    let flat: Vec<_> = stream_a.iter().flatten().collect();
    assert_eq!(
        flat.iter().filter(|e| e.phase == ENTERED).count(),
        2,
        "the visitor and the ball each enter once: {flat:?}"
    );
}

fn full_stream(solver_iterations: u32) -> Vec<Vec<SensorEvent>> {
    let (world, _, visitor) = scenario();
    let mut rig = Rig::with(
        world,
        PhysicsSettings {
            solver_iterations,
            ..free()
        },
    );
    drive(&mut rig, visitor, 120)
}

#[test]
fn the_event_stream_is_the_same_at_any_thread_count_and_solver_iteration_count() {
    let reference = full_stream(4);
    assert!(reference.iter().any(|events| !events.is_empty()));
    for threads in [1, 2, 8] {
        assert_eq!(with_threads(threads, || full_stream(4)), reference);
    }
    for iterations in [1, 8] {
        assert_eq!(full_stream(iterations), reference);
    }
}

#[test]
fn a_frozen_tick_reports_nothing_and_the_thaw_step_reports_the_entry() {
    let (world, zone, visitor) = zone_and_visitor();
    let mut rig = Rig::new(world);
    rig.tick();
    move_to(&mut rig.world, visitor, [0.0; 3]);
    rig.physics().freeze();
    assert!(rig.tick().is_empty());
    assert_eq!(rig.tick(), vec![event(zone, visitor, ENTERED)]);
    assert!(rig.tick().is_empty());
}

#[test]
fn a_despawn_on_a_frozen_tick_still_reports_despawned_on_the_thaw_step() {
    let (mut rig, zone, crowd) = occupied(1);
    rig.physics().freeze();
    rig.world.despawn(crowd[0]).expect("a live entity");
    assert!(rig.tick().is_empty());
    assert_eq!(rig.tick(), exits_of(zone, &crowd, ExitCause::Despawned));
}

#[test]
fn a_compound_sensor_overlapped_on_both_parts_enters_once() {
    let part = |x: f32| CompoundPart {
        shape: cube(0.5),
        translation: [x, 0.0, 0.0],
        rotation: [0.0; 3],
    };
    let mut world = World::new();
    let zone = spawn(
        &mut world,
        [0.0; 3],
        collider(
            Shape::Compound {
                parts: vec![part(-2.0), part(2.0)],
            },
            Body::new_static(),
            true,
        ),
    );
    let wide = spawn(
        &mut world,
        [0.0; 3],
        collider(Shape::Sphere { radius: 2.3 }, Body::new_kinematic(), false),
    );
    let mut rig = Rig::new(world);
    assert_eq!(rig.tick(), vec![event(zone, wide, ENTERED)]);
}
