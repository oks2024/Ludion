//! `assert_no_boundary_crossings` over an engine whose presentation system discards a refused write.

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, Transform};
use ludion_testkit::{assert_no_boundary_crossings, loaded_engine, step};

const SCENE: &str = "\
format: 1
entities:
  - name: ball
    components:
      Transform: {}
";

fn trespass(world: &mut World, _ctx: &SystemCtx) {
    let Some((ball, _)) = world.query_ids::<(&Transform,)>().next() else {
        return;
    };
    let _ = world.get_mut::<Transform>(ball);
}

fn quiet(_: &mut World, _ctx: &SystemCtx) {}

#[test]
#[should_panic(expected = "L0028")]
fn a_discarded_crossing_fails_the_assert() {
    let mut engine = loaded_engine(
        Engine::builder().presentation_system("trespass", trespass),
        SCENE,
    );
    step(&mut engine, 3);
    assert_no_boundary_crossings(engine.world());
}

#[test]
fn a_clean_run_passes_the_assert() {
    let mut engine = loaded_engine(Engine::builder().presentation_system("quiet", quiet), SCENE);
    step(&mut engine, 3);
    assert_no_boundary_crossings(engine.world());
}
