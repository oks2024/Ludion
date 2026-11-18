//! **A lamp, a wall and a subject** — a `check` system reads how lit the subject is each tick and
//! records "lit" or "shadow".
//!
//! **The game picks the filter.** `light_exposure` casts a shadow ray only for lights with
//! `cast_shadows: true`, through the filter it is given as written: here the static scenery, never
//! the subject. The wall carries a collider, so it stops the lamp.
//!
//! **Or author it.** The subject also carries `LightExposure: {}`, and the game schedules
//! `light_exposure_update`, which reads every such subject each tick into `LightExposureResults`
//! (default occluders: static colliders only, the subject excluded). `main` asserts the two
//! readings agree.
//!
//! Everything above the `main` divider is copyable into a real game.
//!
//! Run it headless (prints the reading with the wall, then without):
//!
//! ```text
//! cargo run -p ludion --example light_exposure
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Marks the body whose exposure is read.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Subject;

/// What the last tick read: the subject's total light and whether the lamp was shadowed.
#[derive(Default)]
struct Verdict {
    total: f32,
    shadowed: bool,
}

/// Reads the subject's exposure from the static scenery's point of view.
fn check(world: &mut World, _ctx: &SystemCtx) {
    let Some(subject) = world.single::<Subject>() else {
        return;
    };
    let Ok(Some(transform)) = world.get::<Transform>(subject) else {
        return;
    };
    let filter = QueryFilter::new().only_fixed().exclude(subject);
    let query = ExposureQuery {
        points: &[transform.translation],
        facing: Facing::Omni,
        occluders: &filter,
        combine: Combine::Max,
    };
    let Ok(exposure) = light_exposure(world, &query) else {
        return;
    };
    let shadowed = exposure.lights.iter().any(|l| l.occluded);
    world.insert_resource(Verdict {
        total: exposure.total,
        shadowed,
    });
}

/// The game: one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Subject>("Subject")?;
            Ok(())
        })
        .system("check", check)
        .system("light_exposure_update", light_exposure_update)
}

/// A shadow-casting spot over a subject; `{wall}` is a slab between them, or nothing.
fn scene(wall: bool) -> String {
    let wall = if wall {
        "  - name: wall
    components:
      Transform:
        translation:
          - 0.0
          - 2.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 2.0
              - 0.1
              - 2.0
        body: static
"
    } else {
        ""
    };
    format!(
        "format: 1
seed: 1
physics:
  space: free
entities:
  - name: lamp
    components:
      Transform:
        translation:
          - 0.0
          - 5.0
          - 0.0
      SpotLight:
        direction:
          - 0.0
          - -1.0
          - 0.0
        range: 20.0
        cast_shadows: true
  - name: subject
    components:
      Subject:
      LightExposure: {{}}
      Transform: {{}}
{wall}"
    )
}

// --- Headless demo `main` ------------------------------------------------------------------------

/// The function's reading, then the authored component's, both after the same two ticks.
fn run(wall: bool) -> Result<(f32, bool, f32), String> {
    let mut engine = game().build().map_err(|d| d.to_string())?;
    engine.load_inline_scene(&scene(wall)).map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 2,
            hash: false,
        }))
        .map_err(|e| format!("{e:?}"))?;
    let verdict = engine
        .world()
        .resource::<Verdict>()
        .ok_or("check did not run")?;
    let subject = engine.world().single::<Subject>().ok_or("no subject")?;
    let authored = engine
        .world()
        .resource::<LightExposureResults>()
        .and_then(|results| results.reading(subject))
        .ok_or("light_exposure_update did not read the subject")?;
    Ok((verdict.total, verdict.shadowed, authored.total))
}

fn main() -> ExitCode {
    let (Ok((lit, lit_shadowed, lit_authored)), Ok((dark, dark_shadowed, dark_authored))) =
        (run(false), run(true))
    else {
        eprintln!("the scene did not run");
        return ExitCode::FAILURE;
    };
    println!("no wall: total {lit:.3}, shadowed {lit_shadowed}");
    println!("wall:    total {dark:.3}, shadowed {dark_shadowed}");
    if lit_authored != lit || dark_authored != dark {
        eprintln!("the authored component and the function disagree");
        return ExitCode::FAILURE;
    }
    if lit_shadowed || !dark_shadowed || lit <= dark {
        eprintln!("the wall did not shadow the subject");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
