//! **Off-mesh links: a ladder joins two floors, a routed agent walks to it, and you play the climb.**
//!
//! A deck stands 3 m up with no ramp, so the grid alone cannot reach it. A `NavLink` entity adds a ladder
//! edge. The agent's `NavGoal` route crosses it: `nav_steer` stops the walker at the ladder's foot with
//! `NavRoute.status = at_link`; the game plays the climb, moves the body to the ladder's top and calls
//! `nav_link_done`, and the route carries on to the goal. The engine never moves the body.
//!
//! Run it headless (prints the route at the link, the climb, then the arrival):
//!
//! ```text
//! cargo run -p ludion --example nav_link
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

/// The traversal the game owns: move the body to the link's end, then tell the engine it is done.
fn climb(world: &mut World, agent: EntityId, to: [f32; 3]) {
    if let Ok(Some(transform)) = world.get_mut::<Transform>(agent) {
        transform.translation = to;
    }
    nav_link_done(world, agent).expect("the agent is at a link");
}

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor, a deck 3 m up (x from 14 to 24), a ladder from its foot to the deck's edge, and a walker.
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
  - name: deck
    components:
      Transform:
        translation:
          - 19.0
          - 1.5
          - 12.0
      Collider:
        shape:
          box:
            half_extents:
              - 5.0
              - 1.5
              - 12.0
        body: static
  - name: ladder
    components:
      NavLink:
        from:
          - 12.5
          - 0.0
          - 12.5
        to:
          - 15.5
          - 3.0
          - 12.5
        kind: ladder
  - name: walker
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
          - 3.0
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
    let mut climbed = false;
    for tick in 0..600 {
        engine.advance_live();
        let state = engine
            .world_mut()
            .query_ids::<(&NavRoute,)>()
            .next()
            .map(|(id, (route,))| (id, route.clone()));
        let Some((agent, route)) = state else {
            continue;
        };
        match route.status {
            NavRouteStatus::AtLink if !climbed => {
                println!(
                    "tick {tick}: stopped at link {:?}, waypoint {} of {}",
                    route.link,
                    route.index,
                    route.waypoints.len()
                );
                let to = route.waypoints[route.index as usize + 1];
                println!("climbing to {to:?}");
                climb(engine.world_mut(), agent, to);
                climbed = true;
            }
            NavRouteStatus::Arrived => {
                println!("tick {tick}: {:?}", route.status);
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    eprintln!("the walker never arrived");
    ExitCode::FAILURE
}
