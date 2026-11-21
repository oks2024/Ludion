//! **Dynamic blockers: a door the game opens when the guard waits.**
//!
//! A wall splits the floor, with a door gap sealed by an `interact` `NavBlocker`. The guard's `NavGoal` route
//! leads through the door: `nav_steer` stops it before the door with `NavRoute.status = at_blocker` and
//! `NavRoute.blocker` naming the door. The game decides when to open it (here, after a short wait) by setting
//! `enabled: false`; the guard carries on the next tick. There is no done call, and the engine never moves a body.
//!
//! Run it headless (prints the stop, the opening, then the arrival):
//!
//! ```text
//! cargo run -p ludion --example nav_blocker
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

/// Opens the door: a disabled blocker drops out of the overlay at the next `nav_update`.
fn open(world: &mut World, door: EntityId) {
    if let Ok(Some(blocker)) = world.get_mut::<NavBlocker>(door) {
        blocker.enabled = false;
    }
}

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor, a wall across x = 12 with a 3 m gap at z = 11..14, a door blocker in the gap, and a guard.
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
  - name: wall_low
    components:
      Transform:
        translation:
          - 12.0
          - 1.5
          - 5.5
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 1.5
              - 5.5
        body: static
  - name: wall_high
    components:
      Transform:
        translation:
          - 12.0
          - 1.5
          - 19.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 1.5
              - 5.0
        body: static
  - name: door
    components:
      Transform:
        translation:
          - 12.0
          - 1.0
          - 12.5
      NavBlocker:
        shape:
          box:
            half_extents:
              - 0.6
              - 1.0
              - 1.5
        mode: interact
  - name: guard
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 12.5
      NavAgent:
        speed: 6.0
        neighbor_layers: 0
      NavGoal:
        at:
          - 20.5
          - 0.0
          - 12.5
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
    let mut waited = 0;
    let mut opened = false;
    for tick in 0..600 {
        engine.advance_live();
        let route = engine
            .world_mut()
            .query_ids::<(&NavRoute,)>()
            .next()
            .map(|(_, (route,))| route.clone());
        let Some(route) = route else {
            continue;
        };
        match route.status {
            NavRouteStatus::AtBlocker if !opened => {
                if waited == 0 {
                    println!("tick {tick}: waiting at the door {:?}", route.blocker);
                }
                waited += 1;
                if waited == 30 {
                    let door = route.blocker.expect("at_blocker names the door");
                    open(engine.world_mut(), door);
                    opened = true;
                    println!("tick {tick}: opening the door");
                }
            }
            NavRouteStatus::Arrived => {
                println!("tick {tick}: {:?}", route.status);
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    eprintln!("the guard never arrived");
    ExitCode::FAILURE
}
