//! `cylinder` and `compound` colliders end to end: a cylinder rests on its flat face and rolls on
//! its side; a compound is one body with one authored mass spread over its parts by volume, a
//! centre of mass off the entity origin, and one owning entity in contacts, queries and blasts,
//! obeying layers, gravity scale and a snapshot resume.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    Blast, CollisionLayers, ContactPhase, Falloff, PhysicsError, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter, install,
};
use ludion_spatial::{Body, Collider, CompoundPart, Joint, JointKind, RigidBody, Shape, Velocity};

fn settings(gravity: [f32; 3], layers: Option<CollisionLayers>) -> PhysicsSettings {
    PhysicsSettings {
        gravity,
        layers,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn schedule_with(settings: PhysicsSettings) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, settings);
    schedule
}

fn falling_schedule() -> Schedule {
    schedule_with(PhysicsSettings::new(PhysicsSpace::Free))
}

fn zero_gravity_schedule() -> Schedule {
    schedule_with(settings([0.0; 3], None))
}

fn collider(shape: Shape, body: Body, layer: Option<&str>) -> Collider {
    Collider {
        shape,
        body,
        layer: layer.map(str::to_string),
    }
}

fn rigid(mass: f32) -> Body {
    Body::Rigid(RigidBody {
        mass,
        ..RigidBody::default()
    })
}

fn part(shape: Shape, translation: [f32; 3]) -> CompoundPart {
    CompoundPart {
        shape,
        translation,
        rotation: [0.0; 3],
    }
}

fn cube(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

fn sphere(radius: f32) -> Shape {
    Shape::Sphere { radius }
}

fn cylinder(half_height: f32, radius: f32) -> Shape {
    Shape::Cylinder {
        half_height,
        radius,
    }
}

fn spawn(world: &mut World, pose: TestPose, collider: Collider, velocity: Velocity) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, pose)
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, velocity)
        .expect("a fresh entity accepts a velocity");
    entity
}

/// A static floor whose top face is `y = 0`.
fn spawn_floor(world: &mut World) -> EntityId {
    spawn(
        world,
        TestPose::at([0.0, -0.5, 0.0]),
        collider(
            Shape::Box {
                half_extents: [20.0, 0.5, 20.0],
            },
            Body::new_static(),
            None,
        ),
        Velocity::default(),
    )
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity")
}

fn run(world: &mut World, schedule: &mut Schedule, frames: std::ops::Range<u64>) {
    for frame in frames {
        tick(world, schedule, frame);
    }
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[test]
fn a_cylinder_stands_on_its_flat_face() {
    let mut world = World::new();
    spawn_floor(&mut world);
    let can = spawn(
        &mut world,
        TestPose::at([0.0, 2.0, 0.0]),
        collider(cylinder(0.5, 0.4), rigid(1.0), None),
        Velocity::default(),
    );
    let mut schedule = falling_schedule();

    run(&mut world, &mut schedule, 0..180);

    let pose = pose_of(&world, can);
    assert!(
        (pose.translation[1] - 0.5).abs() < 0.02,
        "the flat face rests on the floor top: {pose:?}"
    );
    for axis in 0..3 {
        assert!(
            pose.rotation[axis].abs() < 1e-3,
            "a flat drop leaves the rotation at identity: {pose:?}"
        );
    }
    assert!((pose.rotation[3].abs() - 1.0).abs() < 1e-3, "{pose:?}");
}

#[test]
fn a_cylinder_on_its_side_rolls() {
    let mut world = World::new();
    spawn_floor(&mut world);
    // Rotated 90° about x, the axis lies along z and the round face meets the floor.
    let lying = TestPose {
        translation: [0.0, 0.4, 0.0],
        rotation: euler_xyz_degrees_to_quat([90.0, 0.0, 0.0]),
    };
    let can = spawn(
        &mut world,
        lying,
        collider(cylinder(0.5, 0.4), rigid(1.0), None),
        Velocity {
            linear: [2.0, 0.0, 0.0],
            angular: [0.0; 3],
        },
    );
    let mut schedule = falling_schedule();

    run(&mut world, &mut schedule, 0..60);

    let velocity = velocity_of(&world, can);
    assert!(
        velocity.angular[2].abs() > 0.5,
        "friction turns the slide into a roll about the axis: {velocity:?}"
    );
    assert!(pose_of(&world, can).translation[0] > 1.0);
}

/// Box half `0.5` at `x = 0` and box half `1.0` at `x = 3`: volumes 1 and 8, so the centre of mass
/// sits at `24 / 9` on `x`, off the entity origin.
fn lopsided(mass: f32) -> Collider {
    collider(
        Shape::Compound {
            parts: vec![part(cube(0.5), [0.0; 3]), part(cube(1.0), [3.0, 0.0, 0.0])],
        },
        rigid(mass),
        None,
    )
}

/// The angular velocity one tick after an upward impulse of `1` at world `point`, in zero gravity.
fn spin_after_impulse_at(point: [f32; 3]) -> ([f32; 3], Option<f32>) {
    let mut world = World::new();
    let body = spawn(
        &mut world,
        TestPose::at([0.0; 3]),
        lopsided(5.0),
        Velocity::default(),
    );
    let mut schedule = zero_gravity_schedule();
    tick(&mut world, &mut schedule, 0);
    let mass = physics(&world).mass(body);
    world
        .resource_mut::<PhysicsWorld>()
        .expect("installed")
        .apply_impulse_at_point(body, [0.0, 1.0, 0.0], point)
        .expect("finite inputs queue");
    tick(&mut world, &mut schedule, 1);
    (velocity_of(&world, body).angular, mass)
}

#[test]
fn compound_mass_is_the_authored_mass_spread_by_volume() {
    let (through_com, mass) = spin_after_impulse_at([24.0 / 9.0, 0.0, 0.0]);
    let mass = mass.expect("the compound built a body");
    assert!(
        (mass - 5.0).abs() < 1e-3,
        "the authored mass, not the sum of parts: {mass}"
    );
    assert!(
        length(through_com) < 1e-4,
        "an impulse through the geometric centre of mass does not spin the body: {through_com:?}"
    );

    let (through_origin, _) = spin_after_impulse_at([0.0; 3]);
    assert!(
        length(through_origin) > 0.05,
        "the same impulse at the entity origin is off the centre of mass and spins it: \
         {through_origin:?}"
    );
}

#[test]
fn contact_events_name_the_compound_entity() {
    let mut world = World::new();
    let floor = spawn_floor(&mut world);
    // A wide plate with a sphere hanging below it at x = 0.6, z = 0.4: the sphere lands first.
    let plate = spawn(
        &mut world,
        TestPose::at([0.0, 3.0, 0.0]),
        collider(
            Shape::Compound {
                parts: vec![
                    part(
                        Shape::Box {
                            half_extents: [1.0, 0.25, 1.0],
                        },
                        [0.0; 3],
                    ),
                    part(sphere(0.25), [0.6, -0.5, 0.4]),
                ],
            },
            rigid(2.0),
            None,
        ),
        Velocity::default(),
    );
    let mut schedule = falling_schedule();

    let mut started = None;
    for frame in 0..120 {
        tick(&mut world, &mut schedule, frame);
        if let Some(event) = physics(&world)
            .contact_events()
            .iter()
            .find(|event| event.phase == ContactPhase::Started)
        {
            started = Some(*event);
            break;
        }
    }

    let event = started.expect("the compound lands on the floor");
    let pair = [event.a, event.b];
    assert!(pair.contains(&floor) && pair.contains(&plate), "{event:?}");
    assert!(
        (event.point[0] - 0.6).abs() < 0.1 && (event.point[2] - 0.4).abs() < 0.1,
        "the point is under the offset sphere, not the entity origin: {event:?}"
    );
    assert!(event.point[1].abs() < 0.1, "{event:?}");
    assert!(event.normal[1].abs() > 0.99, "{event:?}");
}

fn layer_table() -> CollisionLayers {
    // `default` at index 0, `ghost` at 1; `ghost` and `default` ignore each other.
    CollisionLayers::new(
        vec!["default".to_string(), "ghost".to_string()],
        vec![0b10, 0b01],
    )
}

#[test]
fn a_compound_obeys_its_layer() {
    let drop = |layer: Option<&str>| -> f32 {
        let mut world = World::new();
        spawn_floor(&mut world);
        let body = spawn(
            &mut world,
            TestPose::at([0.0, 2.0, 0.0]),
            collider(
                Shape::Compound {
                    parts: vec![
                        part(cube(0.5), [0.0; 3]),
                        part(sphere(0.25), [0.0, 0.75, 0.0]),
                    ],
                },
                rigid(1.0),
                layer,
            ),
            Velocity::default(),
        );
        let mut schedule = schedule_with(settings([0.0, -9.81, 0.0], Some(layer_table())));
        run(&mut world, &mut schedule, 0..180);
        pose_of(&world, body).translation[1]
    };

    assert!(
        drop(Some("ghost")) < -5.0,
        "the ghost compound falls through the floor"
    );
    let rested = drop(None);
    assert!(
        rested > 0.3 && rested < 1.0,
        "the default compound rests on the floor: {rested}"
    );
}

/// A static compound: a box at the origin and a sphere at `x = 3`.
fn spawn_two_part_static(world: &mut World) -> EntityId {
    spawn(
        world,
        TestPose::at([0.0; 3]),
        collider(
            Shape::Compound {
                parts: vec![
                    part(cube(0.5), [0.0; 3]),
                    part(sphere(0.25), [3.0, 0.0, 0.0]),
                ],
            },
            Body::new_static(),
            None,
        ),
        Velocity::default(),
    )
}

#[test]
fn queries_report_the_owning_entity() {
    let mut world = World::new();
    let body = spawn_two_part_static(&mut world);
    let mut schedule = zero_gravity_schedule();
    tick(&mut world, &mut schedule, 0);
    let physics = physics(&world);
    let filter = QueryFilter::new();

    // Aimed at the offset sphere only: the box at the entity origin is 3 units to the side.
    let hits = physics
        .ray_cast([3.0, 5.0, 0.0], [0.0, -1.0, 0.0], 10.0, &filter)
        .expect("a valid ray");
    assert_eq!(
        hits.iter().map(|hit| hit.entity).collect::<Vec<_>>(),
        vec![body]
    );
    assert!((hits[0].point[1] - 0.25).abs() < 1e-3, "{:?}", hits[0]);

    let query = cylinder(0.5, 0.25);
    let identity = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(
        physics
            .overlap(&query, [3.0, 0.0, 0.0], identity, &filter)
            .expect("a cylinder is a query shape"),
        vec![body]
    );
    let cast = physics
        .shape_cast(
            &query,
            [3.0, 5.0, 0.0],
            identity,
            [0.0, -1.0, 0.0],
            10.0,
            &filter,
        )
        .expect("a cylinder is a query shape");
    assert_eq!(
        cast.iter().map(|hit| hit.entity).collect::<Vec<_>>(),
        vec![body]
    );

    let compound_query = Shape::Compound {
        parts: vec![part(sphere(0.25), [0.0; 3])],
    };
    assert!(matches!(
        physics.shape_cast(
            &compound_query,
            [3.0, 5.0, 0.0],
            identity,
            [0.0, -1.0, 0.0],
            10.0,
            &filter
        ),
        Err(PhysicsError::QueryInput(_))
    ));
    assert!(matches!(
        physics.overlap(&compound_query, [3.0, 0.0, 0.0], identity, &filter),
        Err(PhysicsError::QueryInput(_))
    ));
}

#[test]
fn a_blast_hits_a_compound_through_its_nearest_part() {
    let mut world = World::new();
    let body = spawn(
        &mut world,
        TestPose::at([0.0; 3]),
        collider(
            Shape::Compound {
                parts: vec![
                    part(cube(0.5), [0.0; 3]),
                    part(sphere(0.25), [3.0, 0.0, 0.0]),
                ],
            },
            rigid(1.0),
            None,
        ),
        Velocity::default(),
    );
    let mut schedule = zero_gravity_schedule();
    tick(&mut world, &mut schedule, 0);

    // The sphere's surface is 1.75 from the centre, the box's and the entity origin are 4.5 and 5.
    physics_mut(&mut world)
        .radial_impulse(Blast {
            center: [5.0, 0.0, 0.0],
            radius: 2.5,
            impulse: 4.0,
            falloff: Falloff::Constant,
            filter: QueryFilter::new(),
        })
        .expect("a valid blast queues");
    tick(&mut world, &mut schedule, 1);

    let hits = physics(&world).blast_hits();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].entity, body);
    assert!((hits[0].point[0] - 3.25).abs() < 1e-3, "{:?}", hits[0]);
}

fn physics_mut(world: &mut World) -> &mut PhysicsWorld {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installed a world")
}

fn scaled_compound(gravity_scale: f32, mass: f32) -> Collider {
    collider(
        Shape::Compound {
            parts: vec![
                part(cube(0.5), [0.0; 3]),
                part(sphere(0.25), [0.0, 0.75, 0.0]),
            ],
        },
        Body::Rigid(RigidBody {
            mass,
            gravity_scale,
            ..RigidBody::default()
        }),
        None,
    )
}

fn set_collider(world: &mut World, entity: EntityId, collider: Collider) {
    *world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider") = collider;
}

#[test]
fn gravity_scale_applies_to_a_compound() {
    let mut world = World::new();
    let floater = spawn(
        &mut world,
        TestPose::at([0.0, 5.0, 0.0]),
        scaled_compound(0.0, 1.0),
        Velocity::default(),
    );
    let mut schedule = falling_schedule();
    run(&mut world, &mut schedule, 0..60);
    assert_eq!(
        pose_of(&world, floater).translation[1],
        5.0,
        "a zero scale keeps the compound exactly in place"
    );

    set_collider(&mut world, floater, scaled_compound(-1.0, 1.0));
    run(&mut world, &mut schedule, 60..120);
    assert_eq!(physics(&world).gravity_scale(floater), Some(-1.0));
    assert!(
        pose_of(&world, floater).translation[1] > 5.5,
        "a negative scale rises: {:?}",
        pose_of(&world, floater)
    );
}

/// A hinged bob whose compound body has its centre of mass off the pose: two small spheres, one at
/// the pose and one 0.3 out. Limits `[-10, 10]` degrees around the pose the joint captured.
fn hinged_compound_bob(mass: f32) -> (World, EntityId) {
    let mut world = World::new();
    let anchor = spawn(
        &mut world,
        TestPose::at([0.0; 3]),
        collider(sphere(0.1), Body::new_static(), None),
        Velocity::default(),
    );
    let bob = spawn(
        &mut world,
        TestPose::at([1.0, 0.0, 0.0]),
        collider(
            Shape::Compound {
                parts: vec![
                    part(sphere(0.1), [0.0; 3]),
                    part(sphere(0.1), [0.3, 0.0, 0.0]),
                ],
            },
            rigid(mass),
            None,
        ),
        Velocity::default(),
    );
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Hinge {
                    axis: [0.0, 0.0, 1.0],
                    limits: Some([-10.0, 10.0]),
                },
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
            },
        )
        .expect("a fresh entity accepts a joint");
    (world, bob)
}

fn angle_z(world: &World, entity: EntityId) -> f32 {
    let [x, y, _] = pose_of(world, entity).translation;
    ludion_core::math::atan2(y, x).to_degrees()
}

/// A scale-only edit is applied in place, so the hinge keeps the zero it captured at build time
/// and the bob swings from its lower limit to its upper one; a rebuild would re-capture the zero
/// at the resting pose and the bob would stop near 0°.
#[test]
fn a_scale_only_edit_keeps_a_compounds_joint_zero() {
    let (mut world, bob) = hinged_compound_bob(1.0);
    let mut schedule = falling_schedule();
    run(&mut world, &mut schedule, 0..120);
    let before = angle_z(&world, bob);
    assert!(
        (before + 10.0).abs() < 2.0,
        "the bob rests at its lower limit: {before}"
    );

    let edited = scaled_compound_of(&world, bob, -1.0);
    set_collider(&mut world, bob, edited);
    run(&mut world, &mut schedule, 120..360);
    let after = angle_z(&world, bob);
    assert!(
        (after - 10.0).abs() < 2.0,
        "the bob settles at the upper limit: {after}"
    );
    assert_eq!(physics(&world).is_joint_active(bob), Some(true));
}

/// `entity`'s collider with only its `gravity_scale` changed.
fn scaled_compound_of(world: &World, entity: EntityId, gravity_scale: f32) -> Collider {
    let mut edited = world
        .get::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider")
        .clone();
    let Body::Rigid(body) = &mut edited.body else {
        panic!("the entity is not rigid");
    };
    body.gravity_scale = gravity_scale;
    edited
}

type StateBits = ([u32; 3], [u32; 4], [u32; 3], [u32; 3]);

fn state_bits(world: &World, entity: EntityId) -> StateBits {
    let pose = pose_of(world, entity);
    let velocity = velocity_of(world, entity);
    (
        pose.translation.map(f32::to_bits),
        pose.rotation.map(f32::to_bits),
        velocity.linear.map(f32::to_bits),
        velocity.angular.map(f32::to_bits),
    )
}

/// A compound with a rotated part, tumbling onto a floor.
fn tumbler() -> Collider {
    collider(
        Shape::Compound {
            parts: vec![
                part(cube(0.4), [0.0; 3]),
                CompoundPart {
                    shape: cylinder(0.5, 0.2),
                    translation: [0.7, 0.1, 0.0],
                    rotation: [0.0, 0.0, 60.0],
                },
            ],
        },
        rigid(2.0),
        None,
    )
}

const TUMBLE_POSE: [f32; 3] = [0.0, 3.0, 0.0];
const TUMBLE_SPIN: Velocity = Velocity {
    linear: [0.5, 0.0, 0.0],
    angular: [1.0, 2.0, 3.0],
};

#[test]
fn a_compound_survives_a_snapshot_bit_exact() {
    let start = || {
        let mut world = World::new();
        let floor = spawn_floor(&mut world);
        let body = spawn(
            &mut world,
            TestPose::at(TUMBLE_POSE),
            tumbler(),
            TUMBLE_SPIN,
        );
        (world, floor, body)
    };

    let (mut reference, _, reference_body) = start();
    let mut reference_schedule = falling_schedule();
    let mut reference_trace = Vec::with_capacity(90);
    for frame in 0..90u64 {
        tick(&mut reference, &mut reference_schedule, frame);
        reference_trace.push(state_bits(&reference, reference_body));
    }

    let (mut captured, floor, body) = start();
    let mut captured_schedule = falling_schedule();
    run(&mut captured, &mut captured_schedule, 0..30);
    let bytes = physics(&captured)
        .to_bytes()
        .expect("the solver state encodes");
    let pose_at_30 = pose_of(&captured, body);
    let velocity_at_30 = velocity_of(&captured, body);

    // A fresh world with the same entities and the ECS state a real restore would carry.
    let mut restored = World::new();
    let restored_floor = restored.spawn();
    assert_eq!(restored_floor, floor);
    restored
        .insert(restored_floor, TestPose::at([0.0, -0.5, 0.0]))
        .unwrap();
    restored
        .insert(
            restored_floor,
            captured.get::<Collider>(floor).unwrap().unwrap().clone(),
        )
        .unwrap();
    let restored_body = restored.spawn();
    assert_eq!(restored_body, body);
    restored.insert(restored_body, pose_at_30).unwrap();
    restored.insert(restored_body, tumbler()).unwrap();
    restored.insert(restored_body, velocity_at_30).unwrap();
    restored.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the captured bytes decode"),
    );
    let mut restored_schedule = falling_schedule();

    let mut restored_trace = Vec::with_capacity(60);
    for frame in 30..90u64 {
        tick(&mut restored, &mut restored_schedule, frame);
        restored_trace.push(state_bits(&restored, restored_body));
    }

    assert_eq!(
        restored_trace,
        reference_trace[30..],
        "a compound resumed from a snapshot must match the uninterrupted trace bit-for-bit"
    );
}
