//! A claimed entity's `Collider` edit reaches the solver by rebuilding its body — shape, body
//! kind, mass, friction, restitution, damping, lock and `sleep_after_ticks`, none of which
//! `create` re-reads on its own (a `gravity_scale`-only edit is in place — `tests/gravity_scale.rs`).
//! An untouched `Collider` must never trigger it.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{ContactPhase, PhysicsSpace, PhysicsWorld, ShapeKind, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn rigid(shape: Shape, body: RigidBody) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(body),
        layer: None,
        sensor: false,
    }
}

fn static_(shape: Shape) -> Collider {
    Collider {
        shape,
        body: Body::new_static(),
        layer: None,
        sensor: false,
    }
}

fn free_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    schedule
}

fn spawn(world: &mut World, translation: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(translation))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    entity
}

fn set_collider(world: &mut World, entity: EntityId, collider: Collider) {
    *world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider") = collider;
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("a claimed body installs the solver")
}

fn velocity_of(world: &World, entity: EntityId) -> Velocity {
    *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity")
}

fn bits3(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

fn bits4(v: [f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// The pose and velocity of `entity`, as exact bits — the determinism trace's one comparison unit.
type StateBits = ([u32; 3], [u32; 4], [u32; 3], [u32; 3]);

fn state_bits(world: &World, entity: EntityId) -> StateBits {
    let pose = pose_of(world, entity);
    let velocity = velocity_of(world, entity);
    (
        bits3(pose.translation),
        bits4(pose.rotation),
        bits3(velocity.linear),
        bits3(velocity.angular),
    )
}

#[test]
fn an_edited_collider_rebuilds_with_the_new_mass_and_shape() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        [0.0, 5.0, 0.0],
        rigid(
            Shape::Box {
                half_extents: [0.5; 3],
            },
            RigidBody {
                mass: 1.0,
                ..RigidBody::default()
            },
        ),
    );
    let mut schedule = free_schedule();

    for frame in 0..20 {
        tick(&mut world, &mut schedule, frame);
    }
    let before_pose = pose_of(&world, entity);
    let before_velocity = velocity_of(&world, entity);

    set_collider(
        &mut world,
        entity,
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 4.0,
                ..RigidBody::default()
            },
        ),
    );
    tick(&mut world, &mut schedule, 20);

    assert_eq!(
        physics(&world).mass(entity),
        Some(4.0),
        "the rebuilt body must carry the edited mass"
    );
    assert_eq!(
        physics(&world).shape_kind(entity),
        Some(ShapeKind::Ball),
        "the rebuilt body must carry the edited shape"
    );

    let after_pose = pose_of(&world, entity);
    let dt = 1.0 / 60.0_f32;
    let max_travel = before_velocity.linear[1].abs() * dt + 9.81 * dt * dt + 0.01;
    assert!(
        (after_pose.translation[1] - before_pose.translation[1]).abs() <= max_travel,
        "a rebuild must carry the body over at its current pose, not teleport it to the spawn \
         pose: before {before_pose:?} after {after_pose:?} (max one-tick travel {max_travel})"
    );

    let after_velocity = velocity_of(&world, entity);
    let expected_y = before_velocity.linear[1] - 9.81 * dt;
    assert!(
        (after_velocity.linear[1] - expected_y).abs() < 1e-2,
        "a rebuild must carry the body's velocity forward and let gravity keep accelerating it \
         by exactly one more tick, not reset it: before {before_velocity:?} after \
         {after_velocity:?} expected_y {expected_y}"
    );
}

#[test]
fn a_rebuild_keeps_the_momentum_of_a_body_with_no_velocity_component() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 50.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            rigid(
                Shape::Sphere { radius: 0.5 },
                RigidBody {
                    mass: 1.0,
                    ..RigidBody::default()
                },
            ),
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = free_schedule();
    for frame in 0..20 {
        tick(&mut world, &mut schedule, frame);
    }
    let y19 = pose_of(&world, entity).translation[1];
    tick(&mut world, &mut schedule, 20);
    let y20 = pose_of(&world, entity).translation[1];
    let per_tick = y19 - y20;

    set_collider(
        &mut world,
        entity,
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 4.0,
                ..RigidBody::default()
            },
        ),
    );
    tick(&mut world, &mut schedule, 21);
    let y21 = pose_of(&world, entity).translation[1];
    let rebuilt = y20 - y21;

    // Free fall: the displacement grows by g·dt² each tick, so a kept velocity moves one tick
    // further than the last; a zeroed one moves a single g·dt² step.
    let dt = 1.0 / 60.0_f32;
    let expected = per_tick + 9.81 * dt * dt;
    assert!(
        (rebuilt - expected).abs() < 1e-3,
        "a rebuilt body must keep falling at its speed: expected {expected} per tick, got \
         {rebuilt} (previous tick {per_tick})"
    );
}

#[test]
fn a_heightfield_on_a_rigid_body_is_refused() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        [0.0; 3],
        rigid(
            Shape::Heightfield {
                rows: 2,
                cols: 2,
                heights: vec![0.0; 4],
                scale: [1.0; 3],
            },
            RigidBody::default(),
        ),
    );
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    let rejections = world
        .resource_mut::<PhysicsWorld>()
        .expect("the claim installs the solver")
        .take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_MESH_ON_RIGID);
    assert!(
        rejections[0].reason.contains("no interior"),
        "{rejections:?}"
    );
}

#[test]
fn an_untouched_collider_never_rebuilds() {
    let mut world = World::new();
    spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        static_(Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        }),
    );
    let ball = spawn(
        &mut world,
        [0.0, 2.0, 0.0],
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            },
        ),
    );
    let mut schedule = free_schedule();

    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        frame += 1;
        if physics(&world).is_sleeping(ball) == Some(true) {
            break;
        }
        assert!(frame < 300, "the ball never fell asleep");
    }

    for _ in 0..60 {
        tick(&mut world, &mut schedule, frame);
        frame += 1;
        assert_eq!(
            physics(&world).is_sleeping(ball),
            Some(true),
            "an untouched Collider must never rebuild, and a spurious rebuild would wake the body"
        );
    }
}

#[test]
fn a_rebuild_wakes_the_bodies_touching_it() {
    let mut world = World::new();
    spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        static_(Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        }),
    );
    let bottom = spawn(
        &mut world,
        [0.0, 0.5, 0.0],
        rigid(
            Shape::Box {
                half_extents: [0.5; 3],
            },
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            },
        ),
    );
    let top = spawn(
        &mut world,
        [0.0, 1.5, 0.0],
        rigid(
            Shape::Box {
                half_extents: [0.5; 3],
            },
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            },
        ),
    );
    let mut schedule = free_schedule();

    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        frame += 1;
        if physics(&world).is_sleeping(bottom) == Some(true)
            && physics(&world).is_sleeping(top) == Some(true)
        {
            break;
        }
        assert!(frame < 300, "the stack never settled asleep");
    }

    set_collider(
        &mut world,
        bottom,
        rigid(
            Shape::Box {
                half_extents: [0.1; 3],
            },
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            },
        ),
    );
    tick(&mut world, &mut schedule, frame);

    assert_eq!(
        physics(&world).is_sleeping(bottom),
        Some(false),
        "the rebuilt body itself must start awake"
    );
    assert_eq!(
        physics(&world).is_sleeping(top),
        Some(false),
        "a body touching the rebuilt one must be woken before the old body is removed"
    );
}

#[test]
fn a_rebuild_while_touching_emits_no_second_started() {
    let mut world = World::new();
    let floor = spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        static_(Shape::Box {
            half_extents: [20.0, 0.5, 20.0],
        }),
    );
    let ball = spawn(
        &mut world,
        [0.0, 2.0, 0.0],
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                sleep_after_ticks: Some(300),
                ..RigidBody::default()
            },
        ),
    );
    let mut schedule = free_schedule();
    let pair = if floor < ball {
        (floor, ball)
    } else {
        (ball, floor)
    };

    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        frame += 1;
        if !physics(&world).contact_events().is_empty() {
            break;
        }
        assert!(frame < 300, "the ball never touched the floor");
    }

    set_collider(
        &mut world,
        ball,
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 1.0,
                friction: 0.9,
                restitution: 0.0,
                sleep_after_ticks: Some(300),
                ..RigidBody::default()
            },
        ),
    );
    tick(&mut world, &mut schedule, frame);

    let events = physics(&world).contact_events();
    assert!(
        !events
            .iter()
            .any(|event| { event.phase == ContactPhase::Started && (event.a, event.b) == pair }),
        "a rebuild of a still-touching body must not emit a second Started: {events:?}"
    );
}

#[test]
fn an_edit_to_a_degenerate_hull_drops_the_body_and_an_edit_back_restores_it() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        [0.0, 5.0, 0.0],
        rigid(
            Shape::Box {
                half_extents: [0.5; 3],
            },
            RigidBody::default(),
        ),
    );
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    assert_eq!(physics(&world).body_count(), 1);

    set_collider(
        &mut world,
        entity,
        rigid(
            Shape::Convex {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            },
            RigidBody::default(),
        ),
    );
    tick(&mut world, &mut schedule, 1);
    assert_eq!(
        physics(&world).body_count(),
        0,
        "a degenerate hull must drop the body, not keep the old one"
    );
    let rejections = world
        .resource_mut::<PhysicsWorld>()
        .expect("still installed")
        .take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_DEGENERATE_HULL);

    set_collider(
        &mut world,
        entity,
        rigid(
            Shape::Box {
                half_extents: [0.5; 3],
            },
            RigidBody::default(),
        ),
    );
    tick(&mut world, &mut schedule, 2);
    assert_eq!(
        physics(&world).body_count(),
        1,
        "editing the Collider back to a buildable shape must rebuild the body"
    );
}

#[test]
fn a_rebuild_is_deterministic_and_survives_a_snapshot() {
    fn falling_world() -> (World, EntityId, Schedule) {
        let mut world = World::new();
        let entity = spawn(
            &mut world,
            [0.0, 5.0, 0.0],
            rigid(
                Shape::Box {
                    half_extents: [0.5; 3],
                },
                RigidBody {
                    mass: 1.0,
                    ..RigidBody::default()
                },
            ),
        );
        let schedule = free_schedule();
        (world, entity, schedule)
    }

    fn edit_a() -> Collider {
        rigid(
            Shape::Sphere { radius: 0.5 },
            RigidBody {
                mass: 4.0,
                ..RigidBody::default()
            },
        )
    }

    fn edit_b() -> Collider {
        rigid(
            Shape::Box {
                half_extents: [0.3; 3],
            },
            RigidBody {
                mass: 2.0,
                ..RigidBody::default()
            },
        )
    }

    // Uninterrupted reference: both edits applied in one continuous run.
    let (mut reference_world, reference_entity, mut reference_schedule) = falling_world();
    let mut reference_trace = Vec::with_capacity(60);
    for frame in 0..60u64 {
        if frame == 20 {
            set_collider(&mut reference_world, reference_entity, edit_a());
        }
        if frame == 40 {
            set_collider(&mut reference_world, reference_entity, edit_b());
        }
        tick(&mut reference_world, &mut reference_schedule, frame);
        reference_trace.push(state_bits(&reference_world, reference_entity));
    }

    // Captured: identical for the first 30 ticks (including the first edit), then snapshot the
    // solver bytes and the ECS state a real restore would carry alongside them.
    let (mut captured_world, captured_entity, mut captured_schedule) = falling_world();
    for frame in 0..30u64 {
        if frame == 20 {
            set_collider(&mut captured_world, captured_entity, edit_a());
        }
        tick(&mut captured_world, &mut captured_schedule, frame);
    }
    let bytes = physics(&captured_world)
        .to_bytes()
        .expect("the solver state encodes");
    let pose_at_30 = pose_of(&captured_world, captured_entity);
    let velocity_at_30 = velocity_of(&captured_world, captured_entity);
    let collider_at_30 = captured_world
        .get::<Collider>(captured_entity)
        .expect("a live entity")
        .expect("its collider")
        .clone();

    // Restored: a fresh world carrying the same entity and ECS state (a `PhysicsWorld` snapshot
    // never carries `Transform`/`Velocity` — the engine's own restore rebuilds those separately),
    // then the second edit and the remaining 30 ticks.
    let mut restored_world = World::new();
    let restored_entity = restored_world.spawn();
    assert_eq!(
        restored_entity, captured_entity,
        "a fresh world's first spawn must reproduce the captured entity id"
    );
    restored_world.insert(restored_entity, pose_at_30).unwrap();
    restored_world
        .insert(restored_entity, collider_at_30)
        .unwrap();
    restored_world
        .insert(restored_entity, velocity_at_30)
        .unwrap();
    restored_world.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the captured bytes decode"),
    );
    let mut restored_schedule = free_schedule();

    let mut restored_trace = Vec::with_capacity(30);
    for frame in 30..60u64 {
        if frame == 40 {
            set_collider(&mut restored_world, restored_entity, edit_b());
        }
        tick(&mut restored_world, &mut restored_schedule, frame);
        restored_trace.push(state_bits(&restored_world, restored_entity));
    }

    assert_eq!(
        restored_trace,
        reference_trace[30..],
        "a rebuild after a restore must match the uninterrupted trace bit-for-bit — the body's \
         source Collider must ride the snapshot"
    );
}
