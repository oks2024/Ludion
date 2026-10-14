//! `--render-out --frames N` ticks the **game's** schedule, not the engine's empty one.
//!
//! The proof runs against `ludion_engine::engine_at_frame` — the CPU half of
//! `render_scene_with_builder`, which is what `--render-out` and `GoldenTest::with_builder` both
//! drive their ticks through — so it needs no GPU: the ticks land before a renderer is ever asked
//! for. Frame N of a game must be the world N ticks of *its* systems produce; frame N of the
//! built-in schedule is frame 0 with nothing but the sim clock moved, and the second assertion pins
//! that difference.

use ludion::prelude::*;

/// One unit of eastward drift per tick: a hashed component moving is what "the game ran" means here.
fn drift(world: &mut World, _ctx: &SystemCtx) {
    for (xform,) in world.query::<(&mut Transform,)>() {
        xform.translation[0] += 1.0;
    }
}

fn game() -> EngineBuilder {
    Engine::builder().system("drift", drift)
}

const SCENE: &str = concat!(
    "format: 1\n",
    "entities:\n",
    "  - name: drifter\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
);

/// Writes the scene to its own temp directory and returns the file path.
fn scene_file(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-render-out-schedule-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.yaml");
    std::fs::write(&path, SCENE).unwrap();
    path
}

/// The x the `drifter` entity sits at after the ticks.
fn drifter_x(engine: &mut ludion_engine::Engine) -> f32 {
    engine
        .world_mut()
        .query::<(&Transform,)>()
        .next()
        .expect("the scene authors one entity with a Transform")
        .0
        .translation[0]
}

#[test]
fn the_builders_schedule_runs_once_per_frame() {
    let scene = scene_file("game");
    let mut engine = ludion_engine::engine_at_frame(game(), &scene, 7).unwrap();
    assert_eq!(
        drifter_x(&mut engine),
        7.0,
        "seven ticks of the game's own schedule must move the world seven times"
    );
}

#[test]
fn the_builtin_schedule_moves_nothing() {
    let scene = scene_file("builtin");
    let mut engine = ludion_engine::engine_at_frame(Engine::builder(), &scene, 7).unwrap();
    assert_eq!(
        drifter_x(&mut engine),
        0.0,
        "a builder with no systems is the old `--render-out`: frame 7 is frame 0 with the clock moved"
    );
}
