//! **Guards on patrol** — one walks a loop around a wall, one stands at a post and looks around.
//!
//! A patrol is a `kind: patrol` file: ordered stops with an optional wait, facing, look-around
//! sequence and tag. A guard carries a `NavAgent` and a `PatrolFollower { route }`; the game schedules
//! `patrol_update` between `nav_update` and `nav_steer`, and the engine writes the guard's `NavGoal` for
//! each stop, so every leg is a nav path around obstacles. **The engine never turns the body**: the
//! facing the game should turn to is `look_deg` in `patrol_motion`, and the game's own mover applies it.
//!
//! Everything above the `main` divider is copyable into a real game. The patrols themselves are
//! content under the game's content root.
//!
//! Run it headless (prints each stop a guard began, with its tag and facing):
//!
//! ```text
//! cargo run -p ludion --example patrol
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// One stop a guard began: `(tick, guard, point, tag, look_deg)`.
type Stop = (u64, String, u32, Option<String>, Option<f32>);

/// The stops the guards began, in order.
#[derive(Default)]
struct Stops(Vec<Stop>);

/// Records each stop the tick it begins. Registered after `patrol_update`, like any game system.
fn watch(world: &mut World, ctx: &SystemCtx) {
    let guards: Vec<EntityId> = world
        .query_ids::<(&PatrolFollower,)>()
        .map(|(id, _)| id)
        .collect();
    for id in guards {
        let Some(motion) = patrol_motion(world, id).filter(|m| m.arrived) else {
            continue;
        };
        let name = world
            .get::<Name>(id)
            .ok()
            .flatten()
            .map_or_else(String::new, |name| name.0.clone());
        if world.resource::<Stops>().is_none() {
            world.insert_resource(Stops::default());
        }
        if let Some(stops) = world.resource_mut::<Stops>() {
            stops.0.push((
                ctx.frame.as_u64(),
                name,
                motion.point,
                motion.tag,
                motion.look_deg,
            ));
        }
    }
}

/// Applies each agent's steering velocity to its position (a real game moves a motor instead).
fn mover(world: &mut World, ctx: &SystemCtx) {
    let moves: Vec<_> = world
        .query_ids::<(&NavSteering,)>()
        .map(|(id, (steering,))| (id, steering.velocity))
        .collect();
    for (id, v) in moves {
        if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
            transform.translation[0] += v[0] * ctx.dt;
            transform.translation[2] += v[2] * ctx.dt;
        }
    }
}

/// The game: the nav grid, `patrol_update` between `nav_update` and `nav_steer`, a mover, a watcher.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .nav(NavConfig {
            nx: 24,
            nz: 24,
            y_min: -2.0,
            y_max: 8.0,
            ..NavConfig::default()
        })
        .system("nav_update", nav_update)
        .system("patrol_update", patrol_update)
        .system("nav_steer", nav_steer)
        .system("mover", mover)
        .system("watch", watch)
}

/// A square round the wall: wait a moment at the second corner, tagged for the game's door logic.
const LOOP: &str = "\
format: 1
kind: patrol
name: yard
mode: loop
points:
  - at:
      - 4.0
      - 0.0
      - 4.0
  - at:
      - 16.0
      - 0.0
      - 4.0
    wait: 0.5
    tag: door
  - at:
      - 16.0
      - 0.0
      - 20.0
  - at:
      - 4.0
      - 0.0
      - 20.0
";

/// A post: one stop, facing north, scanning left and right.
const POST: &str = "\
format: 1
kind: patrol
name: gate
mode: loop
points:
  - at:
      - 20.0
      - 0.0
      - 12.0
    face_deg: 180.0
    wait: 2.0
    look:
      - yaw_deg: -45.0
        hold: 0.5
      - yaw_deg: 45.0
        hold: 0.5
";

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor, a wall in the middle, and two guards.
const SCENE: &str = "\
format: 1
seed: 1
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 12.0
          - -0.5
          - 12.0
      Collider:
        shape:
          box:
            half_extents:
              - 12.0
              - 0.5
              - 12.0
        body: static
  - name: wall
    components:
      Transform:
        translation:
          - 10.0
          - 1.5
          - 12.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 1.5
              - 4.0
        body: static
  - name: runner
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 4.0
      NavAgent:
        speed: 6.0
        neighbor_layers: 0
      PatrolFollower:
        route: patrols/yard.patrol.yaml
  - name: sentry
    components:
      Transform:
        translation:
          - 20.0
          - 0.0
          - 12.0
      NavAgent:
        speed: 6.0
        neighbor_layers: 0
      PatrolFollower:
        route: patrols/gate.patrol.yaml
";

fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_patrols(&root) {
        eprintln!("could not write the demo patrols: {error}");
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
    for _ in 0..900 {
        engine.advance_live();
    }
    let Some(stops) = engine.world().resource::<Stops>() else {
        eprintln!("no guard began a stop");
        return ExitCode::FAILURE;
    };
    for (tick, guard, point, tag, look) in &stops.0 {
        println!("tick {tick:>3}  {guard:<7} stop {point}  tag {tag:?}  facing {look:?}");
    }
    let runner_stops = stops.0.iter().filter(|s| s.1 == "runner").count();
    let sentry_looked = stops
        .0
        .iter()
        .any(|s| s.1 == "sentry" && s.4 == Some(135.0));
    if runner_stops < 4 || !sentry_looked {
        eprintln!("the guards did not patrol as authored");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// A fresh, process-unique temp directory for the demo's patrols (removed first, so a re-run starts
/// clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-patrol-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes both patrol documents under `root/patrols/`.
fn write_patrols(root: &Path) -> std::io::Result<()> {
    let dir = root.join("patrols");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("yard.patrol.yaml"), LOOP)?;
    std::fs::write(dir.join("gate.patrol.yaml"), POST)
}
