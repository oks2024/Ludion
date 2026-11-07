//! First-person recoil — the copyable pattern for a shot that kicks the view without moving the
//! aim: a `look` system turns mouse motion into rig look, and a left click adds a recoil kick with
//! [`add_kick`]. The kick shows on the camera `Transform` and decays back to centre on its own;
//! `CameraRig::aim()` never carries it, so a hitscan fired from `aim()` goes where the crosshair was.
//!
//! The kick, roll and field-of-view channels are sim state (hashed, snapshotted, replayed). For an
//! effect that is presentation only, use `CameraOffset` instead.
//!
//! Everything above the `main` divider is copyable as-is. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example first_person_kick
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Degrees of look per raw mouse count.
const DEG_PER_COUNT: f32 = 0.1;
/// Degrees of pitch kick per shot; recoil up is a negative pitch.
const RECOIL_PITCH_DEG: f32 = -2.0;

/// Mouse motion turns the view; a left click kicks it.
fn look_and_fire(world: &mut World, _ctx: &SystemCtx) {
    let (delta, fired) = {
        let mouse = world.resource::<InputState>().unwrap().mouse();
        (mouse.delta(), mouse.was_pressed(MouseButton::Left))
    };
    let Some(cam) = world.entities().find(|&id| world.has::<CameraRig>(id)) else {
        return;
    };
    let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
    add_look(rig, -delta[0] * DEG_PER_COUNT, delta[1] * DEG_PER_COUNT);
    if fired {
        add_kick(rig, 0.0, RECOIL_PITCH_DEG, 0.0);
    }
}

/// The game: the look-and-fire system on a default engine.
pub fn game() -> EngineBuilder {
    Engine::builder().system("look_and_fire", look_and_fire)
}

/// A first-person camera riding a hero entity.
const SCENE: &str = r#"format: 1
seed: 1
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
            first_person: {}
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
"#;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Looks down 5 degrees, fires at tick 5, and prints the aim pitch beside the camera's pitch while
/// the kick decays. (A real game's `main` is just `ludion::run(game())`.)
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
    let inject = |engine: &mut Engine, frame: u64, event: serde_json::Value| {
        engine
            .execute(Command::InputInject(command::InputInjectParams {
                frame: Some(FrameIndex::new(frame)),
                event,
            }))
            .expect("the event injects");
    };
    inject(&mut engine, 1, motion(0.0, 50.0));
    inject(
        &mut engine,
        5,
        serde_json::json!({ "kind": "mouse_down", "button": "left" }),
    );

    let mut aim_pitches = Vec::new();
    let mut kicked_ticks = 0;
    for tick in 0..90 {
        engine.advance_live();
        let world = engine.world();
        let cam = world
            .entities()
            .find(|&id| world.has::<CameraRig>(id))
            .expect("the camera");
        let rig = world.get::<CameraRig>(cam).unwrap().unwrap();
        let aim = rig.aim().expect("posed");
        let camera_rotation = world.get::<Transform>(cam).unwrap().unwrap().rotation;
        if camera_rotation != aim.rotation_deg() {
            kicked_ticks += 1;
        }
        if tick >= 2 {
            aim_pitches.push(aim.pitch_deg);
        }
        if tick % 10 == 4 {
            println!(
                "tick {tick:>2}: aim pitch {:.3}, camera rotation x {:.3}, kick {:.3}",
                aim.pitch_deg, camera_rotation[0], rig.kick_deg[1]
            );
        }
    }

    let aim_never_moved = aim_pitches
        .windows(2)
        .all(|w| w[0].to_bits() == w[1].to_bits());
    assert!(aim_never_moved, "the kick never moves aim()");
    assert!(kicked_ticks > 0, "the camera shows the kick");
    let world = engine.world();
    let cam = world
        .entities()
        .find(|&id| world.has::<CameraRig>(id))
        .unwrap();
    let rig = world.get::<CameraRig>(cam).unwrap().unwrap();
    let rest = world.get::<Transform>(cam).unwrap().unwrap().rotation;
    assert_eq!(rig.kick_deg, [0.0; 3], "the kick has rested");
    assert_eq!(
        rest,
        rig.aim().unwrap().rotation_deg(),
        "the camera is back on the aim"
    );
    println!(
        "kicked for {kicked_ticks} ticks; aim pitch held at {:.3}",
        aim_pitches[0]
    );
    ExitCode::SUCCESS
}
