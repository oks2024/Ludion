//! Facade smoke for the `spatial3d` module: a scene-authored `SpatialVolume`,
//! `spatial3d::rebuild::<Transform, TransformHierarchy>` as an ordinary system, and each query,
//! all reachable through `use ludion::prelude::*` alone.

use ludion::prelude::*;
use spatial3d::{Filter, Handle, Index3d, Neighbor};

fn game() -> EngineBuilder {
    Engine::builder().system(
        "spatial3d_rebuild",
        spatial3d::rebuild::<Transform, TransformHierarchy>,
    )
}

/// A sphere at x = 4 (layer 1), a box at x = 8 (layer 2) and a sphere at x = 4, z = 6 (layer 1).
const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: ball\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 4.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      SpatialVolume:\n",
    "        shape:\n",
    "          sphere:\n",
    "            radius: 1.0\n",
    "        layers: 1\n",
    "  - name: crate\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 8.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      SpatialVolume:\n",
    "        shape:\n",
    "          aabb:\n",
    "            half_extents:\n",
    "              - 1.0\n",
    "              - 1.0\n",
    "              - 1.0\n",
    "        layers: 2\n",
    "  - name: far_ball\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 4.0\n",
    "          - 0.0\n",
    "          - 6.0\n",
    "      SpatialVolume:\n",
    "        shape:\n",
    "          sphere:\n",
    "            radius: 1.0\n",
    "        layers: 1\n",
);

#[test]
fn spatial3d_works_end_to_end_through_the_facade() {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("a scene authoring SpatialVolume validates and loads");
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("sim steps");

    let world = engine.world();
    let by_x = |x: f32, z: f32| {
        world
            .entities()
            .find(|&e| {
                world.get::<Transform>(e).is_ok_and(|t| {
                    t.is_some_and(|t| t.translation[0] == x && t.translation[2] == z)
                })
            })
            .expect("entity at that spot")
    };
    let (ball, crate_, far_ball) = (by_x(4.0, 0.0), by_x(8.0, 0.0), by_x(4.0, 6.0));
    let index = world.resource::<Index3d>().expect("rebuilt");
    assert_eq!(index.len(), 3);

    let mut handles = Vec::new();
    index.overlap_sphere([4.0, 0.0, 0.5], 0.25, &Filter::new(1), &mut handles);
    assert_eq!(handles, [Handle::entity(ball)]);
    index.overlap_aabb(
        [7.5, -0.5, -0.5],
        [9.0, 0.5, 0.5],
        &Filter::new(2),
        &mut handles,
    );
    assert_eq!(handles, [Handle::entity(crate_)]);

    let mut near: Vec<Neighbor> = Vec::new();
    index.k_nearest([0.0; 3], 1, 100.0, &Filter::new(3), &mut near);
    assert_eq!(near[0].handle, Handle::entity(ball));
    assert_eq!(near[0].distance_sq, 16.0);
    index.k_nearest([0.0; 3], 3, 100.0, &Filter::new(1), &mut near);
    assert_eq!(
        near.iter().map(|n| n.handle).collect::<Vec<_>>(),
        [Handle::entity(ball), Handle::entity(far_ball)]
    );

    let hit = index
        .cast([0.0; 3], [10.0, 0.0, 0.0], 0.0, &Filter::new(3))
        .expect("the segment hits the ball");
    assert_eq!(hit.handle, Handle::entity(ball));
    assert_eq!(hit.t, 0.3);
    assert_eq!(hit.normal, [-1.0, 0.0, 0.0]);
    // Excluding the ball lets the segment reach the crate's face at x = 7.
    let hit = index
        .cast(
            [0.0; 3],
            [10.0, 0.0, 0.0],
            0.0,
            &Filter::new(3).excluding(Handle::entity(ball)),
        )
        .expect("the segment hits the crate");
    assert_eq!(hit.handle, Handle::entity(crate_));
    assert_eq!(hit.t, 0.7);
}
