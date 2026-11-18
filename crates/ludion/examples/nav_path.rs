//! **Point-to-point navigation: ask for a path, or give an agent a goal and let it walk the route.**
//!
//! A wall stands between an agent and its goal. `nav_path` plans the route around it on request
//! (A* plus string pulling: a few waypoints, not a node per cell). An agent that also carries a
//! `NavGoal` walks the same kind of route by itself: `nav_steer` plans it, writes it as a `NavRoute`
//! and steers along it, and the game's mover applies the velocity.
//!
//! Run it headless (prints the planned path, then the agent's route status after 240 ticks):
//!
//! ```text
//! cargo run -p ludion --example nav_path
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

fn game() -> EngineBuilder {
    Engine::builder()
        .nav(NavConfig {
            nx: 24,
            nz: 24,
            y_min: -2.0,
            y_max: 8.0,
            ..NavConfig::default()
        })
        .system("nav_update", nav_update)
        .system("nav_steer", nav_steer)
        .system("mover", mover)
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

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor with its top at y = 0, a wall across x = 10.5, and an agent routing to the far side.
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
          - 10.5
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
  - name: walker
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 12.0
      NavAgent:
        speed: 6.0
        neighbor_layers: 0
      NavGoal:
        at:
          - 20.0
          - 0.0
          - 12.0
";

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
    // The first tick creates the physics bodies; the grid is built from them on the second.
    for _ in 0..3 {
        engine.advance_live();
    }
    match nav_path(
        engine.world(),
        &PathRequest::new([2.0, 0.0, 12.0], [20.0, 0.0, 12.0]),
    ) {
        Ok(path) => println!(
            "path: {:?}, {:.1} m, waypoints {:?}",
            path.status, path.cost, path.waypoints
        ),
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    }
    for _ in 0..240 {
        engine.advance_live();
    }
    for (_, (route,)) in engine.world_mut().query_ids::<(&NavRoute,)>() {
        println!(
            "route after 240 ticks: {:?}, waypoint {} of {}",
            route.status,
            route.index,
            route.waypoints.len()
        );
    }
    ExitCode::SUCCESS
}
