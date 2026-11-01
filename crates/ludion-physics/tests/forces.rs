//! `Plan/physics.md` W2/10 (b): a game system writes `Velocity` and the body responds,
//! deterministically — waking it if asleep — and `PhysicsWorld::apply_impulse` is the one way to
//! apply an impulse. Also covers (a)'s cross-check: every hull the `Collider` validator
//! accepts builds the parry shape the table promises.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, SemanticValidate, SystemCtx, World};
use ludion_physics::{NoParent, PhysicsSettings, PhysicsSpace, PhysicsWorld, ShapeKind, install};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

fn zero_gravity() -> PhysicsSettings {
    PhysicsSettings {
        gravity: [0.0; 3],
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn rigid(shape: Shape, body: RigidBody) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(body),
        layer: None,
        sensor: false,
    }
}

/// Applies a `[4, 0, 0]` impulse to the world's one entity, on frame 1 only — registered before
/// [`install`], the position the write system's contract requires of a game system that writes
/// velocities.
fn apply_impulse_at_frame_1(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let entity = world.entities().next().expect("the test spawns one entity");
    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("tick 0's write system created the world");
    physics
        .apply_impulse(entity, [4.0, 0.0, 0.0])
        .expect("a finite impulse queues");
}

fn bits3(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

fn bits4(v: [f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// The pose and velocity of an entity, as exact bits: (translation, rotation, linear, angular).
type StateBits = ([u32; 3], [u32; 4], [u32; 3], [u32; 3]);

/// The pose and velocity of `entity`, as exact bits — the determinism trace's one comparison unit.
fn state_bits(world: &World, entity: EntityId) -> StateBits {
    let pose = pose_of(world, entity);
    let velocity = world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity");
    (
        bits3(pose.translation),
        bits4(pose.rotation),
        bits3(velocity.linear),
        bits3(velocity.angular),
    )
}

fn impulse_world() -> (World, EntityId, Schedule) {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            rigid(
                Shape::Sphere { radius: 0.5 },
                RigidBody {
                    mass: 2.0,
                    ..RigidBody::default()
                },
            ),
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");

    let mut schedule = Schedule::new();
    schedule.add_system("apply_impulse_at_frame_1", apply_impulse_at_frame_1);
    install::<TestPose, NoParent>(&mut schedule, zero_gravity());
    (world, entity, schedule)
}

#[test]
fn a_game_impulse_changes_velocity_by_impulse_over_mass() {
    let (mut world, entity, mut schedule) = impulse_world();

    tick(&mut world, &mut schedule, 0);
    let before_x = pose_of(&world, entity).translation[0];
    tick(&mut world, &mut schedule, 1);

    let velocity = world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("its velocity");
    assert!(
        (velocity.linear[0] - 2.0).abs() < 1e-3,
        "impulse 4.0 over mass 2.0 must resolve to linear.x ~= 2.0: {velocity:?}"
    );
    assert!(
        pose_of(&world, entity).translation[0] > before_x,
        "the impulse must carry the body forward"
    );
}

#[test]
fn a_game_write_wakes_a_sleeping_body() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            rigid(
                Shape::Sphere { radius: 0.5 },
                RigidBody {
                    sleep_after_ticks: Some(2),
                    ..RigidBody::default()
                },
            ),
        )
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");

    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, zero_gravity());

    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        let asleep = world
            .resource::<PhysicsWorld>()
            .and_then(|physics| physics.is_sleeping(entity));
        frame += 1;
        if asleep == Some(true) {
            break;
        }
        assert!(frame < 100, "the resting body never fell asleep");
    }

    let before_y = pose_of(&world, entity).translation[1];
    world
        .get_mut::<Velocity>(entity)
        .expect("a live entity")
        .expect("its velocity")
        .linear = [0.0, 3.0, 0.0];
    tick(&mut world, &mut schedule, frame);

    assert_eq!(
        world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the solver")
            .is_sleeping(entity),
        Some(false),
        "a game write must wake the sleeping body"
    );
    assert!(
        pose_of(&world, entity).translation[1] > before_y,
        "the written velocity must carry the body upward"
    );
}

#[test]
fn forces_are_deterministic() {
    fn trace(ticks: u64) -> Vec<StateBits> {
        let (mut world, entity, mut schedule) = impulse_world();
        (0..ticks)
            .map(|frame| {
                tick(&mut world, &mut schedule, frame);
                state_bits(&world, entity)
            })
            .collect()
    }

    let first = trace(60);
    let second = trace(60);
    assert_eq!(
        first, second,
        "the same schedule, run twice from fresh worlds, must produce bit-identical traces"
    );
}

fn shape_kind_of(collider: Collider) -> Option<ShapeKind> {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0; 3]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, zero_gravity());
    tick(&mut world, &mut schedule, 0);
    world
        .resource::<PhysicsWorld>()
        .and_then(|physics| physics.shape_kind(entity))
}

/// Every hull the `Collider` validator accepts builds a `ConvexPolyhedron`, and every heightfield
/// grid it accepts builds a `HeightField` — the cross-check the validator's own unit tests (in
/// `ludion-spatial`) cannot make, since they never touch `ludion-physics`.
#[test]
fn every_hull_the_validator_accepts_builds() {
    let accepted_hulls = [
        Shape::Convex {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
        },
        Shape::Convex {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        },
    ];
    for shape in accepted_hulls {
        let collider = Collider {
            shape: shape.clone(),
            body: Body::new_static(),
            layer: None,
            sensor: false,
        };
        let json = serde_json::to_value(&collider).unwrap();
        assert!(
            Collider::validate_semantics(&json).is_empty(),
            "{shape:?} must validate"
        );
        assert_eq!(
            shape_kind_of(collider),
            Some(ShapeKind::ConvexPolyhedron),
            "{shape:?} must build a ConvexPolyhedron"
        );
    }

    let rejected_hulls = [
        Shape::Convex {
            points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        },
        Shape::Convex {
            points: vec![[1.0, 1.0, 1.0]; 4],
        },
        Shape::Convex {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
            ],
        },
        Shape::Convex {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
        },
    ];
    for shape in rejected_hulls {
        let collider = Collider {
            shape: shape.clone(),
            body: Body::new_static(),
            layer: None,
            sensor: false,
        };
        let json = serde_json::to_value(&collider).unwrap();
        assert!(
            !Collider::validate_semantics(&json).is_empty(),
            "{shape:?} must be rejected"
        );
    }

    let accepted_grid = Shape::Heightfield {
        rows: 3,
        cols: 2,
        heights: vec![0.0; 6],
        scale: [1.0, 1.0, 1.0],
    };
    let collider = Collider {
        shape: accepted_grid.clone(),
        body: Body::new_static(),
        layer: None,
        sensor: false,
    };
    let json = serde_json::to_value(&collider).unwrap();
    assert!(Collider::validate_semantics(&json).is_empty());
    assert_eq!(shape_kind_of(collider), Some(ShapeKind::HeightField));

    let rejected_grid = Shape::Heightfield {
        rows: 1,
        cols: 4,
        heights: vec![0.0; 4],
        scale: [1.0, 1.0, 1.0],
    };
    let collider = Collider {
        shape: rejected_grid,
        body: Body::new_static(),
        layer: None,
        sensor: false,
    };
    let json = serde_json::to_value(&collider).unwrap();
    assert!(!Collider::validate_semantics(&json).is_empty());
}
