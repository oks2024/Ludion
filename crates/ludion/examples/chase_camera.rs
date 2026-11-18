//! A third-person chase camera — the copyable pattern for an orbit camera: a `look` system turns
//! mouse motion into rig look, and the `chase` rig hangs the eye on a boom from a sprung pivot that
//! follows the hero. Yaw and pitch are not sprung, so the view turns 1:1 with the input; the pitch
//! stays inside `pitch_min_deg..pitch_max_deg`. A `follow` rig takes the same shape with a fixed
//! yaw and pitch and ignores look. `collision` keeps the eye out of walls (it needs the scene's
//! `physics:` block), and the game-written `framing` channel peeks the boom over a shoulder.
//!
//! A stick is not a mouse: mouse counts are already a delta, but a stick's deflection is a rate, so
//! scale it by the real frame time (`ctx.dt`) before [`add_look`], or the turn speed follows the
//! tick rate.
//!
//! Everything above the `main` divider is copyable as-is. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example chase_camera
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Mouse motion turns the boom; the camera never reads the stick or mouse itself.
fn look(world: &mut World, _ctx: &SystemCtx) {
    let delta = world.resource::<InputState>().unwrap().mouse().delta();
    let Some(cam) = world.entities().find(|&id| world.has::<CameraRig>(id)) else {
        return;
    };
    let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
    add_look_counts(rig, delta[0], delta[1]);
}

/// A shoulder peek between ticks 10 and 20: the framing weight eases in and back out on its own.
fn peek(world: &mut World, ctx: &SystemCtx) {
    let Some(cam) = world.entities().find(|&id| world.has::<CameraRig>(id)) else {
        return;
    };
    let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
    let peeking = (10..20).contains(&ctx.frame.as_u64());
    rig.framing = Framing {
        weight: if peeking { 1.0 } else { 0.0 },
        pivot_offset: [0.8, 0.2, 0.0],
        ..Framing::default()
    };
}

/// The game: the look and peek systems on a default engine.
pub fn game() -> EngineBuilder {
    Engine::builder().system("look", look).system("peek", peek)
}

/// A chase camera on a hero entity over a floor; the pitch is held between 10 and 40 degrees.
const SCENE: &str = r#"format: 1
seed: 1
physics:
  space: free
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            chase:
              distance: 6.0
              pitch_deg: 20.0
              pitch_min_deg: 10.0
              pitch_max_deg: 40.0
          collision:
            enabled: true
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -1.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 20.0
              - 0.5
              - 20.0
        body: static
"#;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Turns the view left over thirty ticks and prints the boom's yaw and eye. (A real game's `main`
/// is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let motion = |x: f32, y: f32| serde_json::json!({ "kind": "mouse_motion", "delta_bits": [x.to_bits(), y.to_bits()] });
    for frame in 1..=30 {
        engine
            .execute(Command::InputInject(command::InputInjectParams {
                frame: Some(FrameIndex::new(frame)),
                event: motion(-10.0, 100.0),
            }))
            .expect("the event injects");
    }

    let mut yaws = Vec::new();
    for tick in 0..40 {
        engine.advance_live();
        let world = engine.world();
        let cam = world
            .entities()
            .find(|&id| world.has::<CameraRig>(id))
            .expect("the camera");
        let aim = world
            .get::<CameraRig>(cam)
            .unwrap()
            .unwrap()
            .aim()
            .expect("posed");
        yaws.push(aim.yaw_deg);
        if tick % 10 == 9 {
            println!(
                "tick {tick:>2}: yaw {:>6.2}, pitch {:>5.2}, eye {:.2?}",
                aim.yaw_deg, aim.pitch_deg, aim.eye
            );
        }
    }

    let world = engine.world();
    let cam = world
        .entities()
        .find(|&id| world.has::<CameraRig>(id))
        .unwrap();
    let aim = world.get::<CameraRig>(cam).unwrap().unwrap().aim().unwrap();
    assert!(
        yaws.last().is_some_and(|yaw| yaw.abs() > 1.0),
        "the yaw turned"
    );
    assert_eq!(aim.pitch_deg, 40.0, "the pitch stopped at its clamp");
    ExitCode::SUCCESS
}
