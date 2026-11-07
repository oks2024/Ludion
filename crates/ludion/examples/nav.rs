//! **A walkability grid built from static colliders, and the flow toward a target on a deck.**
//!
//! `.nav(..)` opts in; `nav_update` samples the grid from the static physics colliders (a floor, a
//! 2 m deck and a ramp joining them) and keeps it current when a static changes. A game asks the
//! [`Nav`] resource's cache for the flow field toward a target, then follows the direction at an
//! agent's position.
//!
//! Run it headless (prints the node count and the flow at the bottom of the ramp):
//!
//! ```text
//! cargo run -p ludion --example nav
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
}

/// The flow direction `(dx, dz)` toward `target` at `from`, from the shared field of the target's cell.
fn flow_toward(world: &mut World, target: [f32; 3], from: [f32; 3]) -> Option<[f32; 2]> {
    let (grid, cache) = world.resource_mut::<Nav>()?.parts();
    let grid = grid?;
    let node = grid.node_at(target)?;
    cache.field(grid, node).flow_at(grid, from)
}

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor with its top at y = 0, a deck at y = 2 from x = 14, and a 30-degree ramp between.
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
              - 0.5
              - 12.0
        body: static
  - name: ramp
    components:
      Transform:
        translation:
          - 12.0
          - 0.913
          - 12.0
        rotation:
          - 0.0
          - 0.0
          - 30.0
      Collider:
        shape:
          box:
            half_extents:
              - 2.0
              - 0.1
              - 2.0
        body: static
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
    let nodes = engine
        .world()
        .resource::<Nav>()
        .and_then(Nav::grid)
        .map(|grid| grid.node_count());
    println!("walkable nodes: {nodes:?}");
    let flow = flow_toward(engine.world_mut(), [20.5, 2.0, 12.5], [8.5, 0.0, 12.5]);
    println!("flow at the foot of the ramp toward the deck: {flow:?}");
    ExitCode::SUCCESS
}
