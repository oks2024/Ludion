//! **Slow motion** — the game scales gameplay time to a quarter for a window of frames, and a
//! [`PathFollower`] platform covers a quarter of the distance per tick while it lasts.
//!
//! **The game asks, the clock folds.** `slow_window` calls `GameClock::scale(0.25)` on every frame it
//! wants slowed, exactly as a pause calls `hold()`. The engine's `path_follow` integrates
//! `ctx.game_dt`, which the clock shrinks to `dt × 0.25`, so the platform needs no change.
//! Physics, the motor and the closed-form evaluators do not follow the scale yet
//! (`ludion docs game-clock`).
//!
//! Everything above the `main` divider is copyable into a real game.
//!
//! Run it headless (prints how far the platform moved each tick):
//!
//! ```text
//! cargo run -p ludion --example slow_motion
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The path document, by its content-root-relative path.
const PATH: &str = "paths/straight.path.yaml";

/// The frames the game runs slowed, and the scale it asks for.
const SLOW_FRAMES: std::ops::RangeInclusive<u64> = 4..=7;
const SLOW_SCALE: f32 = 0.25;

/// The platform's poses, one per tick, in the order `rider` saw them.
#[derive(Default)]
struct Ride(Vec<[f32; 3]>);

/// Asks for slow motion on the frames in [`SLOW_FRAMES`]. The request applies to the next tick.
fn slow_window(world: &mut World, ctx: &SystemCtx) {
    if SLOW_FRAMES.contains(&ctx.frame.as_u64())
        && let Some(clock) = world.resource_mut::<GameClock>()
    {
        clock.scale(SLOW_SCALE);
    }
}

/// Records the platform's pose. Registered like any game system; `path_follow` has already run.
fn rider(world: &mut World, _ctx: &SystemCtx) {
    let Some(platform) = world.single::<PathFollower>() else {
        return;
    };
    let Ok(Some(transform)) = world.get::<Transform>(platform) else {
        return;
    };
    let pose = transform.translation;
    if world.resource::<Ride>().is_none() {
        world.insert_resource(Ride::default());
    }
    if let Some(ride) = world.resource_mut::<Ride>() {
        ride.0.push(pose);
    }
}

/// The game: two systems. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .system("slow_window", slow_window)
        .system("rider", rider)
}

/// A forty-metre straight, so the platform never turns a corner during the demo.
const STRAIGHT: &str = "\
format: 1
kind: path
name: straight
interpolation: linear
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 40.0
    - 0.0
    - 0.0
";

/// A camera and a platform that rides the path at 60 metres a second: one metre per tick.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 20.0
          - 8.0
          - 8.0
      Camera: {}
  - name: platform
    components:
      Transform: {}
      PathFollower:
        path: paths/straight.path.yaml
        speed: 60.0
        mode: once
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs twelve ticks and prints how far the platform moved each tick. Deterministic, no window.
fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_path_doc(&root) {
        eprintln!("could not write the demo path document: {error}");
        return ExitCode::FAILURE;
    }
    let mut engine = match game().project_root(&root).build() {
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
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 12,
            hash: false,
        }))
        .expect("the sim steps");

    let ride = engine.world().resource::<Ride>().expect("rider ran");
    let steps: Vec<f32> = ride.0.windows(2).map(|w| w[1][0] - w[0][0]).collect();
    for (tick, step) in steps.iter().enumerate() {
        println!("tick {:>2}  platform moved {step:.2} m", tick + 2);
    }
    let slowed = steps.iter().filter(|s| (**s - 0.25).abs() < 1e-3).count();
    let full = steps.iter().filter(|s| (**s - 1.0).abs() < 1e-3).count();
    if slowed != SLOW_FRAMES.count() || slowed + full != steps.len() {
        eprintln!(
            "expected {} quarter-rate ticks, got {slowed}",
            SLOW_FRAMES.count()
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// A fresh, process-unique temp directory for the demo's path document (removed first, so a re-run
/// starts clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-slow-motion-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the path document under `root/paths/`.
fn write_path_doc(root: &Path) -> std::io::Result<()> {
    let paths = root.join("paths");
    std::fs::create_dir_all(&paths)?;
    std::fs::write(root.join(PATH), STRAIGHT)
}
