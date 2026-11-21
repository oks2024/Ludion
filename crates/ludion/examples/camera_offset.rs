//! Camera offset — the copyable pattern for a boot intro that pulls the rendered camera in without
//! moving the simulation: a presentation entity carries a [`CameraOffset`], a presentation system
//! eases its `translation[2]` from 4 m back to 0, and the world hash is the one the game has without it.
//!
//! The offset is camera-local (`+z` is back), composed onto the rig's pose at extract, so the sim, the
//! rig, the audio listener and the `camera` debug draw keep the rig's own pose. Screen UI a game
//! anchors with `world_to_pixel` follows the drawn frame only when it asks: pass
//! `CameraOffset::applied_to(&transform, &camera)` instead of the rig's components.
//!
//! Everything above the `main` divider is copyable as-is. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example camera_offset
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Ticks the pull-in lasts.
const INTRO_TICKS: u64 = 60;
/// Metres behind the rig the intro starts from.
const START_BACK: f32 = 4.0;

/// Eases the offset entity's `translation[2]` from [`START_BACK`] to 0 over [`INTRO_TICKS`],
/// spawning it on the first tick. A presentation system: it may only write presentation entities.
fn pull_in(world: &mut World, ctx: &SystemCtx) {
    let existing = world.presentation_entities().next();
    let intro = existing.unwrap_or_else(|| world.spawn_presentation());
    let progress = (ctx.frame.as_u64() as f32 / INTRO_TICKS as f32).min(1.0);
    let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
    let offset = CameraOffset {
        translation: [0.0, 0.0, START_BACK * (1.0 - eased)],
        ..CameraOffset::default()
    };
    world
        .insert(intro, offset)
        .expect("the offset entity is live");
}

/// The game with the intro, or without it (the comparison run).
pub fn game(intro: bool) -> EngineBuilder {
    let builder = Engine::builder();
    if intro {
        builder.presentation_system("pull_in", pull_in)
    } else {
        builder
    }
}

/// A camera 8 m back from a cube, looking down −Z.
const SCENE: &str = r#"format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation: [0.0, 1.0, 8.0]
      Camera: {}
  - name: crate
    components:
      Transform: {}
      Mesh:
        shape: cube
"#;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs the scene with and without the intro, asserts the world hash is identical, and prints the
/// drawn eye as the intro eases in. (A real game's `main` is just `ludion::run(game(true))`.)
fn main() -> ExitCode {
    let mut hashes = Vec::new();
    for intro in [false, true] {
        let mut engine = match game(intro).build() {
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
        for tick in 0..=INTRO_TICKS {
            engine.advance_live();
            if intro && tick % 20 == 0 {
                print_eye(&engine, tick);
            }
        }
        hashes.push(engine.world_hash().expect("the world hashes"));
    }
    assert_eq!(hashes[0], hashes[1], "the intro never moves the world hash");
    println!("world hash with and without the intro: {:?}", hashes[1]);
    ExitCode::SUCCESS
}

/// Prints the eye the frame is drawn from: the rig's pose with the offset applied.
fn print_eye(engine: &Engine, tick: u64) {
    let world = engine.world();
    let Some(camera) = world.entities().find(|&id| world.has::<Camera>(id)) else {
        return;
    };
    let rig = *world.get::<Transform>(camera).unwrap().unwrap();
    let projection = world.get::<Camera>(camera).unwrap().unwrap().clone();
    let drawn = world
        .presentation_entities()
        .find_map(|id| world.get::<CameraOffset>(id).ok().flatten().copied())
        .map_or(rig, |offset| offset.applied_to(&rig, &projection).0);
    println!("tick {tick:>2}: eye {:?}", drawn.translation);
}
