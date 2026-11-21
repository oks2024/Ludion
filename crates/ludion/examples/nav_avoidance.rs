//! **Avoidance: two crowds cross a floor without walking through each other.**
//!
//! Six walkers head east and six head west along the same lanes. Every `NavAgent` carries `avoidance`, so
//! `nav_steer` replaces the separation push with an ORCA velocity computed from the neighbours' previous
//! steering. The output is still only `NavSteering.velocity`; the game's mover applies it.
//!
//! A forklift (a `NavObstacle`) drives north through the middle of the crossing. It is no agent, so its motion
//! is read from its pose: the walkers yield to it exactly as they do to each other.
//!
//! Run it headless (prints the closest approach between the crowds and to the forklift, then the arrivals):
//!
//! ```text
//! cargo run -p ludion --example nav_avoidance
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

fn game() -> EngineBuilder {
    Engine::builder()
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
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
        .system("forklift", forklift)
}

/// Drives every `NavObstacle` north at a steady pace; it runs after `nav_steer`, so its state lags one tick.
fn forklift(world: &mut World, ctx: &SystemCtx) {
    let ids: Vec<_> = world
        .query_ids::<(&NavObstacle,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if let Ok(Some(transform)) = world.get_mut::<Transform>(id) {
            transform.translation[2] += FORKLIFT_SPEED * ctx.dt;
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

// --- Harness (not part of the game) ---------------------------------------------------------------

const RADIUS: f32 = 0.5;
const FORKLIFT_RADIUS: f32 = 1.0;
const FORKLIFT_SPEED: f32 = 3.0;

/// A floor and twelve avoiding walkers: even entities head east, odd ones head west.
fn scene() -> String {
    let mut out = "\
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
"
    .to_string();
    for n in 0..6 {
        let z = 8.0 + 1.6 * n as f32;
        for (side, from, to, dz) in [("east", 3.0, 21.0, 0.0), ("west", 21.0, 3.0, 0.8)] {
            out += &format!(
                "  - name: {side}{n}
    components:
      Transform:
        translation:
          - {from:?}
          - 0.0
          - {:?}
      SpatialVolume:
        layers: 2
      NavAgent:
        speed: 5.0
        neighbor_layers: 2
        target:
          - {to:?}
          - 0.0
          - {:?}
        avoidance:
          radius: {RADIUS:?}
",
                z + dz,
                z + dz
            );
        }
    }
    out += &format!(
        "  - name: forklift
    components:
      Transform:
        translation:
          - 12.0
          - 0.0
          - 2.0
      NavObstacle:
        radius: {FORKLIFT_RADIUS:?}
"
    );
    out
}

fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(&scene()) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let mut closest = f32::MAX;
    let mut closest_forklift = f32::MAX;
    for _ in 0..400 {
        engine.advance_live();
        let mut at: Vec<(EntityId, [f32; 3])> = engine
            .world_mut()
            .query_ids::<(&Transform, &NavAgent)>()
            .map(|(id, (t, _))| (id, t.translation))
            .collect();
        at.sort_by_key(|(id, _)| *id);
        for (_, obstacle) in engine
            .world_mut()
            .query_ids::<(&NavObstacleState,)>()
            .map(|(id, (state,))| (id, state.position))
            .collect::<Vec<_>>()
        {
            for (_, a) in &at {
                closest_forklift = closest_forklift
                    .min(ludion::math::hypot(a[0] - obstacle[0], a[2] - obstacle[2]));
            }
        }
        for (i, (_, a)) in at.iter().enumerate() {
            for (_, b) in &at[i + 1..] {
                closest = closest.min(ludion::math::hypot(a[0] - b[0], a[2] - b[2]));
            }
        }
    }
    let arrived = engine
        .world_mut()
        .query_ids::<(&Transform, &NavAgent)>()
        .filter(|(_, (t, agent))| {
            ludion::math::hypot(
                t.translation[0] - agent.target[0],
                t.translation[2] - agent.target[2],
            ) < 1.5
        })
        .count();
    println!(
        "closest approach {closest:.2} m (radius sum {:.2}), to the forklift {closest_forklift:.2} m (radius sum {:.2}); {arrived} of 12 arrived",
        2.0 * RADIUS,
        RADIUS + FORKLIFT_RADIUS
    );
    if closest >= 2.0 * RADIUS - 0.1
        && closest_forklift >= RADIUS + FORKLIFT_RADIUS - 0.1
        && arrived == 12
    {
        ExitCode::SUCCESS
    } else {
        eprintln!("the crowds overlapped or did not arrive");
        ExitCode::FAILURE
    }
}
