//! Facade smoke for the spatial module (Phase 12 Wave 5): everything a game needs — `Collider`
//! authoring in a scene, `spatial::rebuild::<Transform>` registered as an ordinary system through
//! `Engine::builder().system(...)`, and each query + `move_and_slide` — reachable through
//! `use ludion::prelude::*` alone.

use ludion::prelude::*;

/// The game: the rebuild system at an explicit schedule position, exactly as the guide shows.
fn game() -> EngineBuilder {
    Engine::builder().system("spatial_rebuild", spatial::rebuild::<Transform>)
}

/// Two colliders authored in a scene: a static wall AABB ahead of a dynamic circle mover.
const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: mover\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      Collider:\n",
    "        shape:\n",
    "          circle:\n",
    "            radius: 0.5\n",
    "        body: kinematic\n",
    "  - name: wall\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 3.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      Collider:\n",
    "        shape:\n",
    "          aabb:\n",
    "            half_extents:\n",
    "              - 0.5\n",
    "              - 4.0\n",
    "        body: static\n",
);

#[test]
fn spatial_module_works_end_to_end_through_the_facade() {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("a scene authoring Collider validates and loads");

    // One tick runs the registered rebuild system, populating the index.
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("sim steps");

    let world = engine.world_mut();
    let mover = world
        .entities()
        .find(|&e| {
            world
                .get::<Collider>(e)
                .is_ok_and(|c| c.is_some_and(|c| matches!(c.body, Body::Kinematic(_))))
        })
        .expect("the mover is alive and carries its Collider");
    let wall = world
        .entities()
        .find(|&e| {
            world
                .get::<Collider>(e)
                .is_ok_and(|c| c.is_some_and(|c| matches!(c.body, Body::Static(_))))
        })
        .expect("the wall is alive and carries its Collider");

    // overlaps: a probe over the wall finds it (and only it, filtered to statics).
    let hits = spatial::overlaps(
        world,
        Shape::Circle { radius: 1.0 },
        [3.0, 0.0],
        Some(Body::new_static()),
    );
    assert_eq!(hits, vec![wall]);

    // k_nearest from the origin sees both, nearest first (mover at 0, wall at 3).
    let nearest = spatial::k_nearest(world, [0.0, 0.0], 2, 10.0, None);
    assert_eq!(nearest, vec![mover, wall]);

    // raycast along +x from just past the mover hits the wall's near face (x = 2.5).
    let hit = spatial::raycast(world, [1.0, 0.0], [1.0, 0.0], Some(Body::new_static()))
        .expect("the ray hits the wall");
    assert_eq!(hit.entity, wall);
    assert!((hit.t - 1.5).abs() < 1e-5, "t was {}", hit.t);

    // move_and_slide: the mover heading diagonally into the wall keeps only the tangential axis.
    let resolved = spatial::move_and_slide::<Transform>(world, mover, [3.0, 1.0]);
    assert_eq!(resolved, [0.0, 1.0], "x blocked by the wall, z preserved");
}
