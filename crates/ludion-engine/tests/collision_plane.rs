//! The game-chosen collision [`Plane`]: a world carrying `Plane::Xy` runs every 2D query on
//! `Transform.translation` x/y instead of the default x/z.
//!
//! These live in `ludion-engine` rather than `ludion-spatial` because they assert on the one
//! `GroundPosition` impl the engine owns — `Transform` — which sits above the spatial crate.
//! Headless: `spatial::rebuild` and the queries are pure sim-side CPU work, so no renderer is built.

use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, Transform};
use ludion_spatial::{Body, Collider, Plane, Shape, spatial};

fn ctx() -> SystemCtx {
    SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
}

/// Spawns a unit-circle collider at `translation`.
fn spawn(world: &mut World, translation: [f32; 3], body: Body) -> EntityId {
    let e = world.spawn();
    world
        .insert(
            e,
            Collider {
                shape: Shape::Circle { radius: 1.0 },
                body,
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            e,
            Transform {
                translation,
                ..Transform::default()
            },
        )
        .unwrap();
    e
}

/// Two colliders separated along world `y`: on the ground plane they share the same `(x, z)` and
/// overlap; on `Plane::Xy` the 5-unit gap separates them.
#[test]
fn the_xy_plane_separates_colliders_the_ground_plane_stacks() {
    let mut ground = World::new();
    let near = spawn(&mut ground, [0.0, 0.0, 0.0], Body::new_static());
    let above = spawn(&mut ground, [0.0, 5.0, 0.0], Body::new_static());
    spatial::rebuild::<Transform>(&mut ground, &ctx());
    let stacked = spatial::overlaps(&ground, Shape::Circle { radius: 1.0 }, [0.0, 0.0], None);
    assert_eq!(stacked, vec![near, above], "x/z ignores the y separation");

    let mut sprites = World::new();
    sprites.insert_resource(Plane::Xy);
    let near = spawn(&mut sprites, [0.0, 0.0, 0.0], Body::new_static());
    let _above = spawn(&mut sprites, [0.0, 5.0, 0.0], Body::new_static());
    spatial::rebuild::<Transform>(&mut sprites, &ctx());
    let hits = spatial::overlaps(&sprites, Shape::Circle { radius: 1.0 }, [0.0, 0.0], None);
    assert_eq!(hits, vec![near], "x/y reads the y separation");
}

/// A mover pushed diagonally into a wall standing along world `x` slides along it: on `Plane::Xy`
/// the cancelled axis is `y`, so the `x` component survives.
#[test]
fn move_and_slide_slides_along_y_on_the_xy_plane() {
    let mut world = World::new();
    world.insert_resource(Plane::Xy);
    let mover = spawn(&mut world, [0.0, 0.0, 0.0], Body::new_kinematic());
    // A wall two units up in world y — blocking the mover's y motion, not its x motion.
    let wall = world.spawn();
    world
        .insert(
            wall,
            Collider {
                shape: Shape::Aabb {
                    half_extents: [10.0, 0.5],
                },
                body: Body::new_static(),
                layer: None,
                sensor: false,
            },
        )
        .unwrap();
    world
        .insert(
            wall,
            Transform {
                translation: [0.0, 2.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    spatial::rebuild::<Transform>(&mut world, &ctx());

    let resolved = spatial::move_and_slide::<Transform>(&world, mover, [0.5, 1.0]);
    assert_eq!(
        resolved,
        [0.5, 0.0],
        "the y push cancels, the x push survives"
    );
}

/// The builder's choice reaches the world it builds, and survives the resource wipe a `scene.load`
/// performs.
#[test]
fn the_builder_installs_the_plane_resource_on_every_world() {
    let mut engine = Engine::builder().plane(Plane::Xy).build().unwrap();
    assert_eq!(engine.plane(), Plane::Xy);
    assert_eq!(Plane::of(engine.world()), Plane::Xy);
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    assert_eq!(Plane::of(engine.world()), Plane::Xy);

    let default = Engine::builder().build().unwrap();
    assert_eq!(Plane::of(default.world()), Plane::Xz);
}
