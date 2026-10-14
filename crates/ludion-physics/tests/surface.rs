//! The author surface's contract: every `Shape` maps to the parry shape the table promises, the
//! plane becomes axis locks, and the three body kinds own their pose the way they claim to.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{PhysicsSettings, PhysicsSpace, PhysicsWorld, ShapeKind, install};
use ludion_spatial::{AxisLock, Body, Collider, CompoundPart, Plane, RigidBody, Shape, Velocity};

/// Spawns one entity carrying `collider` at the origin and runs a single tick in `space`, so the
/// write system has built its body.
///
/// `plane` and `space` are separate arguments because they are separate facts: a world can hold a
/// `Plane` and still simulate [`PhysicsSpace::Free`], which is the case the engine's default plane
/// makes ordinary.
fn world_with(collider: Collider, plane: Option<Plane>, space: PhysicsSpace) -> (World, EntityId) {
    let mut world = World::new();
    if let Some(plane) = plane {
        world.insert_resource(plane);
    }
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, space);
    tick(&mut world, &mut schedule, 0);
    (world, entity)
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("a claimed collider installs the world on the first tick")
}

fn fixed(shape: Shape) -> Collider {
    Collider {
        shape,
        body: Body::new_static(),
        layer: None,
    }
}

fn rigid(shape: Shape) -> Collider {
    Collider {
        shape,
        body: Body::Rigid(RigidBody::default()),
        layer: None,
    }
}

#[test]
fn a_two_dimensional_static_or_kinematic_collider_stays_in_the_spatial_tier() {
    for collider in [
        Collider {
            shape: Shape::Circle { radius: 0.5 },
            body: Body::new_static(),
            layer: None,
        },
        Collider {
            shape: Shape::Aabb {
                half_extents: [1.0, 1.0],
            },
            body: Body::new_static(),
            layer: None,
        },
        Collider {
            shape: Shape::Circle { radius: 0.5 },
            body: Body::new_kinematic(),
            layer: None,
        },
        Collider {
            shape: Shape::Aabb {
                half_extents: [1.0, 1.0],
            },
            body: Body::new_kinematic(),
            layer: None,
        },
    ] {
        let described = format!("{collider:?}");
        let (world, _) = world_with(collider, Some(Plane::Xz), PhysicsSpace::Planar);
        // No bodies, and no resource at all: a twin-stick game's walls and movers cost the solver
        // nothing, even with `install` in its schedule.
        assert!(
            world.resource::<PhysicsWorld>().is_none(),
            "{described} should belong to the 2D index"
        );
    }
}

#[test]
fn a_rigid_in_plane_collider_is_claimed() {
    for shape in [
        Shape::Circle { radius: 0.5 },
        Shape::Aabb {
            half_extents: [1.0, 1.0],
        },
    ] {
        let described = format!("{shape:?}");
        let (world, entity) = world_with(rigid(shape), Some(Plane::Xz), PhysicsSpace::Planar);
        assert_eq!(physics(&world).body_count(), 1, "{described}");
        assert_eq!(
            physics(&world).entities().collect::<Vec<_>>(),
            vec![entity],
            "{described}"
        );
    }
}

#[test]
fn the_world_appears_only_on_the_tick_that_claims_something() {
    let mut world = World::new();
    let wall = world.spawn();
    world
        .insert(wall, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            wall,
            Collider {
                shape: Shape::Circle { radius: 0.5 },
                body: Body::new_static(),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);
    assert!(world.resource::<PhysicsWorld>().is_none());

    let prop = world.spawn();
    world
        .insert(prop, TestPose::at([0.0, 5.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(prop, rigid(Shape::Sphere { radius: 0.5 }))
        .expect("a fresh entity accepts a collider");
    tick(&mut world, &mut schedule, 1);
    assert_eq!(physics(&world).entities().collect::<Vec<_>>(), vec![prop]);
}

#[test]
fn every_shape_maps_to_the_shape_the_table_promises() {
    let cases = [
        (Shape::Circle { radius: 0.5 }, ShapeKind::Ball),
        (
            Shape::Aabb {
                half_extents: [0.5, 0.25],
            },
            ShapeKind::Cuboid,
        ),
        (Shape::Sphere { radius: 0.5 }, ShapeKind::Ball),
        (
            Shape::Box {
                half_extents: [0.5, 0.25, 0.75],
            },
            ShapeKind::Cuboid,
        ),
        (
            Shape::Capsule {
                half_height: 0.5,
                radius: 0.25,
            },
            ShapeKind::Capsule,
        ),
        (
            Shape::Convex {
                points: vec![
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                ],
            },
            ShapeKind::ConvexPolyhedron,
        ),
        (
            Shape::Cylinder {
                half_height: 0.5,
                radius: 0.25,
            },
            ShapeKind::Cylinder,
        ),
        (
            Shape::Compound {
                parts: vec![
                    CompoundPart {
                        shape: Shape::Sphere { radius: 0.5 },
                        translation: [0.0; 3],
                        rotation: [0.0; 3],
                    },
                    CompoundPart {
                        shape: Shape::Cylinder {
                            half_height: 0.5,
                            radius: 0.25,
                        },
                        translation: [1.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 90.0],
                    },
                ],
            },
            ShapeKind::Compound,
        ),
        (
            Shape::Heightfield {
                rows: 2,
                cols: 2,
                heights: vec![0.0, 0.1, 0.2, 0.3],
                scale: [1.0, 1.0, 1.0],
            },
            ShapeKind::HeightField,
        ),
    ];
    for (shape, expected) in cases {
        let described = format!("{shape:?}");
        // A heightfield has no interior, so it is the one shape a rigid body refuses.
        let collider = if matches!(shape, Shape::Heightfield { .. }) {
            fixed(shape)
        } else {
            rigid(shape)
        };
        let (world, entity) = world_with(collider, None, PhysicsSpace::Free);
        assert_eq!(
            physics(&world).shape_kind(entity),
            Some(expected),
            "{described}"
        );
    }
}

#[test]
fn a_shape_that_cannot_be_built_gets_no_body() {
    // Two points do not bound a volume, so there is no hull to build.
    let (world, entity) = world_with(
        rigid(Shape::Convex {
            points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        }),
        None,
        PhysicsSpace::Free,
    );
    assert_eq!(physics(&world).body_count(), 0);
    assert_eq!(physics(&world).shape_kind(entity), None);
}

/// A degenerate convex hull is reported once (`L1181`), not retried every tick, and the retry
/// resumes the moment the `Collider` changes to something buildable. `Collider::validate_semantics`
/// would catch this shape too, but this crate has to guard it independently because game code can
/// insert a `Collider` directly, skipping that validator.
#[test]
fn a_rejected_convex_hull_is_reported_once_and_not_retried() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            // Two points do not bound a volume, so parry's own hull builder refuses them too
            // (see `a_shape_that_cannot_be_built_gets_no_body`) — inserted directly, bypassing
            // `Collider::validate_semantics`, the reachable path for a rejection.
            rigid(Shape::Convex {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            }),
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);

    for frame in 0..3 {
        tick(&mut world, &mut schedule, frame);
    }
    let rejections = world
        .resource_mut::<PhysicsWorld>()
        .expect("the claim allocates a world even though the body fails to build")
        .take_rejections();
    assert_eq!(
        rejections.len(),
        1,
        "one rejection over three ticks of the same bad shape: {rejections:?}"
    );
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_DEGENERATE_HULL);

    for frame in 3..6 {
        tick(&mut world, &mut schedule, frame);
    }
    assert!(
        world
            .resource_mut::<PhysicsWorld>()
            .expect("still claimed")
            .take_rejections()
            .is_empty(),
        "an unchanged rejected shape is not retried"
    );

    world
        .insert(entity, rigid(Shape::Sphere { radius: 0.5 }))
        .expect("replacing the collider is an ordinary write");
    tick(&mut world, &mut schedule, 6);
    assert_eq!(
        physics(&world).body_count(),
        1,
        "the fixed shape now builds"
    );
    assert!(
        world
            .resource_mut::<PhysicsWorld>()
            .unwrap()
            .take_rejections()
            .is_empty(),
        "a shape that now builds reports nothing"
    );
}

/// A compound holding a nested compound, inserted directly past the validator, is rejected as
/// `L1189` — parry would panic on the nested composite — and builds no body.
#[test]
fn a_compound_with_a_nested_compound_is_rejected_once_without_a_body() {
    let leaf = CompoundPart {
        shape: Shape::Sphere { radius: 0.5 },
        translation: [0.0; 3],
        rotation: [0.0; 3],
    };
    let nested = CompoundPart {
        shape: Shape::Compound {
            parts: vec![leaf.clone()],
        },
        translation: [0.0; 3],
        rotation: [0.0; 3],
    };
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            rigid(Shape::Compound {
                parts: vec![leaf, nested],
            }),
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);

    for frame in 0..3 {
        tick(&mut world, &mut schedule, frame);
    }
    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the claim allocates a world even though the body fails to build");
    let rejections = physics.take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_COMPOUND_INVALID);
    assert!(
        rejections[0].reason.contains("compound part 1"),
        "{}",
        rejections[0].reason
    );
    assert_eq!(physics.body_count(), 0);
}

/// A compound part offset by a non-finite translation, inserted directly past the validator, is
/// rejected as `L1189` and builds no body.
#[test]
fn a_compound_with_a_non_finite_offset_is_rejected_without_a_body() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            rigid(Shape::Compound {
                parts: vec![CompoundPart {
                    shape: Shape::Sphere { radius: 0.5 },
                    translation: [f32::INFINITY, 0.0, 0.0],
                    rotation: [0.0; 3],
                }],
            }),
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);

    for frame in 0..3 {
        tick(&mut world, &mut schedule, frame);
    }
    let physics = world
        .resource_mut::<PhysicsWorld>()
        .expect("the claim allocates a world even though the body fails to build");
    let rejections = physics.take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_COMPOUND_INVALID);
    assert!(
        rejections[0].reason.contains("compound part 0"),
        "{}",
        rejections[0].reason
    );
    assert_eq!(physics.body_count(), 0);
}

/// Same contract as the convex case, for a ragged `heightfield` grid.
#[test]
fn a_rejected_heightfield_grid_is_reported_once() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            fixed(Shape::Heightfield {
                rows: 1,
                cols: 4,
                heights: vec![0.0; 4],
                scale: [1.0, 1.0, 1.0],
            }),
        )
        .expect("a fresh entity accepts a collider");
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);

    for frame in 0..3 {
        tick(&mut world, &mut schedule, frame);
    }
    let rejections = world
        .resource_mut::<PhysicsWorld>()
        .expect("the claim allocates a world even though the body fails to build")
        .take_rejections();
    assert_eq!(rejections.len(), 1, "{rejections:?}");
    assert_eq!(rejections[0].entity, entity);
    assert_eq!(rejections[0].code, code::COLLIDER_HEIGHTFIELD_GRID);
}

#[test]
fn the_plane_locks_the_off_plane_translation_and_the_in_plane_rotations() {
    for (plane, expected) in [
        (
            Plane::Xz,
            AxisLock {
                translation: [false, true, false],
                rotation: [true, false, true],
            },
        ),
        (
            Plane::Xy,
            AxisLock {
                translation: [false, false, true],
                rotation: [true, true, false],
            },
        ),
    ] {
        let (world, entity) = world_with(
            rigid(Shape::Circle { radius: 0.5 }),
            Some(plane),
            PhysicsSpace::Planar,
        );
        assert_eq!(
            physics(&world).locked_axes(entity),
            Some(expected),
            "{plane:?}"
        );
        assert_eq!(physics(&world).plane(), Some(plane));
    }
}

/// The case the engine's default plane makes ordinary: a world *has* a `Plane`, and a 3D game says
/// so at the install site instead of being silently frozen on the axis gravity points down.
#[test]
fn free_space_ignores_a_plane_the_world_holds() {
    let (world, entity) = world_with(
        rigid(Shape::Sphere { radius: 0.5 }),
        Some(Plane::Xz),
        PhysicsSpace::Free,
    );
    assert_eq!(physics(&world).plane(), None);
    assert_eq!(
        physics(&world).locked_axes(entity),
        Some(AxisLock::default()),
        "Free takes no locks from a plane it was told to ignore"
    );
}

#[test]
fn an_in_plane_box_is_extruded_off_plane_but_a_world_without_a_plane_is_unlocked() {
    let (world, entity) = world_with(
        rigid(Shape::Aabb {
            half_extents: [0.5, 0.25],
        }),
        None,
        PhysicsSpace::Free,
    );
    assert_eq!(physics(&world).shape_kind(entity), Some(ShapeKind::Cuboid));
    assert_eq!(
        physics(&world).locked_axes(entity),
        Some(AxisLock::default())
    );
    assert_eq!(physics(&world).plane(), None);
}

#[test]
fn a_bodys_own_locks_survive_alongside_the_planes() {
    let collider = Collider {
        shape: Shape::Sphere { radius: 0.5 },
        body: Body::Rigid(RigidBody {
            lock: AxisLock {
                translation: [true, false, false],
                rotation: [false, true, false],
            },
            ..RigidBody::default()
        }),
        layer: None,
    };
    let (world, entity) = world_with(collider, Some(Plane::Xz), PhysicsSpace::Planar);
    assert_eq!(
        physics(&world).locked_axes(entity),
        Some(AxisLock {
            translation: [true, true, false],
            rotation: [true, true, true],
        })
    );
}

#[test]
fn a_static_body_never_moves_and_a_rigid_one_falls() {
    let mut world = World::new();
    let ground = world.spawn();
    world
        .insert(ground, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            ground,
            Collider {
                shape: Shape::Box {
                    half_extents: [10.0, 0.5, 10.0],
                },
                body: Body::new_static(),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    let ball = world.spawn();
    world
        .insert(ball, TestPose::at([0.0, 5.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(ball, rigid(Shape::Sphere { radius: 0.5 }))
        .expect("a fresh entity accepts a collider");

    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    for frame in 0..60 {
        tick(&mut world, &mut schedule, frame);
    }

    assert_eq!(pose_of(&world, ground), TestPose::at([0.0, 0.0, 0.0]));
    let fallen = pose_of(&world, ball).translation;
    assert!(fallen[1] < 5.0, "the ball should have fallen: {fallen:?}");
    assert!(fallen[1] > 0.4, "it should rest on the ground: {fallen:?}");
}

#[test]
fn a_kinematic_body_follows_the_ecs_pose() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Sphere { radius: 0.5 },
                body: Body::new_kinematic(),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");

    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    for frame in 0..10 {
        world
            .get_mut::<TestPose>(entity)
            .expect("a live entity")
            .expect("its pose")
            .translation[0] = frame as f32;
        tick(&mut world, &mut schedule, frame);
        // The mirror never writes a kinematic pose back: game code stays the one owner.
        assert_eq!(pose_of(&world, entity).translation[0], frame as f32);
    }
    assert_eq!(physics(&world).body_count(), 1);
}

#[test]
fn a_rigid_body_without_a_velocity_component_simply_does_not_expose_one() {
    let (world, entity) = world_with(
        rigid(Shape::Sphere { radius: 0.5 }),
        None,
        PhysicsSpace::Free,
    );
    assert!(
        world
            .get::<Velocity>(entity)
            .expect("a live entity")
            .is_none()
    );
    assert_eq!(physics(&world).body_count(), 1);
}

#[test]
fn a_body_without_its_own_sleep_ticks_falls_back_to_the_settings_value() {
    let mut world = World::new();
    let default_sleeper = world.spawn();
    world
        .insert(default_sleeper, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            default_sleeper,
            Collider {
                shape: Shape::Sphere { radius: 0.5 },
                body: Body::Rigid(RigidBody {
                    sleep_after_ticks: None,
                    ..RigidBody::default()
                }),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");
    let overridden_sleeper = world.spawn();
    world
        .insert(overridden_sleeper, TestPose::at([5.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            overridden_sleeper,
            Collider {
                shape: Shape::Sphere { radius: 0.5 },
                body: Body::Rigid(RigidBody {
                    sleep_after_ticks: Some(50),
                    ..RigidBody::default()
                }),
                layer: None,
            },
        )
        .expect("a fresh entity accepts a collider");

    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            space: PhysicsSpace::Free,
            gravity: [0.0; 3],
            solver_iterations: 4,
            stabilization_iterations: 1,
            sleep_after_ticks: 2,
            layers: None,
        },
    );
    for frame in 0..10 {
        tick(&mut world, &mut schedule, frame);
    }
    assert_eq!(
        physics(&world).is_sleeping(default_sleeper),
        Some(true),
        "the scene default of 2 ticks should have put it to sleep by now"
    );
    assert_eq!(
        physics(&world).is_sleeping(overridden_sleeper),
        Some(false),
        "its own 50-tick threshold should keep it awake this early"
    );
}

#[test]
fn a_despawned_body_leaves_the_table() {
    let mut world = World::new();
    let kept = world.spawn();
    let dropped = world.spawn();
    for entity in [kept, dropped] {
        world
            .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
            .expect("a fresh entity accepts a pose");
        world
            .insert(entity, rigid(Shape::Sphere { radius: 0.5 }))
            .expect("a fresh entity accepts a collider");
    }
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, PhysicsSpace::Free);
    tick(&mut world, &mut schedule, 0);
    assert_eq!(physics(&world).body_count(), 2);

    world.despawn(dropped).expect("a live entity");
    tick(&mut world, &mut schedule, 1);
    assert_eq!(physics(&world).entities().collect::<Vec<_>>(), vec![kept]);
}

#[test]
fn gravity_is_settable_and_a_top_down_game_turns_it_off() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 5.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, rigid(Shape::Sphere { radius: 0.5 }))
        .expect("a fresh entity accepts a collider");

    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            gravity: [0.0; 3],
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    for frame in 0..30 {
        tick(&mut world, &mut schedule, frame);
    }
    assert_eq!(physics(&world).gravity(), [0.0; 3]);
    assert_eq!(pose_of(&world, entity).translation, [0.0, 5.0, 0.0]);
}

#[test]
fn an_authored_velocity_reaches_the_solver_and_the_mirror_writes_it_back() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at([0.0, 0.0, 0.0]))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, rigid(Shape::Sphere { radius: 0.5 }))
        .expect("a fresh entity accepts a collider");
    world
        .insert(
            entity,
            Velocity {
                linear: [2.0, 0.0, 0.0],
                angular: [0.0; 3],
            },
        )
        .expect("a fresh entity accepts a velocity");

    let mut schedule = Schedule::new();
    install::<TestPose>(
        &mut schedule,
        PhysicsSettings {
            gravity: [0.0; 3],
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    for frame in 0..30 {
        tick(&mut world, &mut schedule, frame);
    }
    let moved = pose_of(&world, entity).translation;
    assert!(
        moved[0] > 0.9,
        "the authored velocity should carry it: {moved:?}"
    );
    let velocity = world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("its velocity");
    assert!((velocity.linear[0] - 2.0).abs() < 1e-3, "{velocity:?}");
}
