//! **Agent classes: one world, two agent sizes.**
//!
//! A wall with a 0.7 m gap crosses the floor and stops short of the far edge. `NavConfig.classes`
//! builds one grid per class: the `walker` (radius 0.3) fits the gap, the `forklift` (radius 1.0)
//! does not and walks around the wall's end. Each query names its class.
//!
//! Run it headless (prints one distance per class and a raycast):
//!
//! ```text
//! cargo run -p ludion --example nav_class
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
            classes: vec![
                NavClass {
                    name: "walker".into(),
                    radius: 0.3,
                    ..NavClass::default()
                },
                NavClass {
                    name: "forklift".into(),
                    radius: 1.0,
                    ..NavClass::default()
                },
            ],
            ..NavConfig::default()
        })
        .system("nav_update", nav_update)
}

// --- Harness (not part of the game) ---------------------------------------------------------------

/// A floor with its top at y = 0 and a wall across x = 10.5 with a gap centred on z = 12.5.
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
  - name: wall_south
    components:
      Transform:
        translation:
          - 10.5
          - 1.5
          - 6.075
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 1.5
              - 6.075
        body: static
  - name: wall_north
    components:
      Transform:
        translation:
          - 10.5
          - 1.5
          - 16.425
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 1.5
              - 3.575
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
    // The first tick creates the physics bodies; the grids are built from them on the second.
    for _ in 0..3 {
        engine.advance_live();
    }
    let (from, to) = ([2.0, 0.0, 12.5], [20.0, 0.0, 12.5]);
    for class in ["walker", "forklift"] {
        match nav_distance(engine.world(), from, to, class) {
            Ok(Some(metres)) => println!("{class}: {metres:.1} m"),
            Ok(None) => println!("{class}: unreachable"),
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
        }
    }
    match nav_raycast(engine.world(), [4.5, 0.0, 6.5], [18.5, 0.0, 6.5], "walker") {
        Ok(hit) => println!("a straight walk at z = 6.5 stops at {hit:?}"),
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
