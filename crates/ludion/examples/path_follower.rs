//! **A moving platform on a path** — a closed square route, one [`PathFollower`] riding it, and a
//! `rider` system that reads the platform's pose the tick it moves.
//!
//! **The engine moves the platform; the game only reads it.** `path_follow` is installed by the
//! engine, before every game system, because the scene holds a follower — the game registers nothing
//! for it. `rider` therefore sees *this* tick's pose, which is what a character standing on the
//! platform needs to move with it.
//!
//! Everything above the `main` divider is copyable into a real game. The path itself is content — a
//! `kind: path` file under the game's content root.
//!
//! Run it headless (prints the platform's pose for a few ticks):
//!
//! ```text
//! cargo run -p ludion --example path_follower
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The path document, by its content-root-relative path.
const PATH: &str = "paths/square.path.yaml";

/// The platform's poses, one per tick, in the order `rider` saw them.
#[derive(Default)]
struct Ride(Vec<[f32; 3]>);

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

/// The game: one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder().system("rider", rider)
}

/// A four-metre square in the XZ plane, closed and linear, so the platform turns corners exactly.
const SQUARE: &str = "\
format: 1
kind: path
name: square
interpolation: linear
closed: true
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 4.0
  - - 0.0
    - 0.0
    - 4.0
";

/// A camera and a platform that rides the square at 60 metres a second: one metre per tick.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 2.0
          - 8.0
          - 8.0
      Camera: {}
  - name: platform
    components:
      Transform: {}
      PathFollower:
        path: paths/square.path.yaml
        speed: 60.0
        mode: loop
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs ten ticks and prints where the platform was each time `rider` looked. Deterministic, no
/// window.
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
            ticks: 10,
            hash: false,
        }))
        .expect("the sim steps");

    let ride = engine.world().resource::<Ride>().expect("rider ran");
    for (tick, pose) in ride.0.iter().enumerate() {
        println!(
            "tick {:>2}  platform at ({:.2}, {:.2}, {:.2})",
            tick + 1,
            pose[0],
            pose[1],
            pose[2]
        );
    }
    // One metre a tick from the origin: four ticks to the first corner, then along +z.
    let corner = ride.0[3];
    let past = ride.0[5];
    if (corner[0] - 4.0).abs() > 1e-3 || (past[2] - 2.0).abs() > 1e-3 {
        eprintln!("the platform did not ride the square as authored");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// A fresh, process-unique temp directory for the demo's path document (removed first, so a re-run
/// starts clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-path-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the path document under `root/paths/`.
fn write_path_doc(root: &Path) -> std::io::Result<()> {
    let paths = root.join("paths");
    std::fs::create_dir_all(&paths)?;
    std::fs::write(root.join(PATH), SQUARE)
}
