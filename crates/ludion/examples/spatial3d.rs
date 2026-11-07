//! **A 3D hitscan with the broadphase** — a shooter, three enemies on one layer, and a `shoot`
//! system that casts a swept sphere down the +x axis and asks for the nearest enemy.
//!
//! **The game schedules the rebuild.** `spatial3d::rebuild` files every [`SpatialVolume`] carrier
//! into the `Index3d` resource at the position the game gives it; `shoot` is registered after it, so
//! it sees this tick's poses. The shooter carries a volume too, so the cast excludes it by handle —
//! otherwise the probe would start inside it and hit at `t = 0`.
//!
//! Everything above the `main` divider is copyable into a real game.
//!
//! Run it headless (prints the first hit and the nearest enemies):
//!
//! ```text
//! cargo run -p ludion --example spatial3d
//! ```

use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spatial3d::{Filter, Handle, Index3d, Neighbor};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Marks the entity that fires.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Shooter;

/// The layer bit enemies and the shooter live on (`SpatialVolume.layers`).
const ACTORS: u32 = 1;

/// What the last tick's shot found.
#[derive(Default)]
struct Shot {
    /// The first hit: handle, fraction of the 20 m cast, surface normal.
    hit: Option<(Handle, f32, [f32; 3])>,
    /// The two actors nearest the shooter, as `(handle, distance²)`.
    nearest: Vec<(Handle, f32)>,
}

/// Casts a 0.25 m swept sphere 20 m down +x from the shooter, then lists its two nearest actors.
/// Registered after `spatial3d_rebuild`, so the index holds this tick's poses.
fn shoot(world: &mut World, _ctx: &SystemCtx) {
    let Some(shooter) = world.single::<Shooter>() else {
        return;
    };
    let Ok(Some(transform)) = world.get::<Transform>(shooter) else {
        return;
    };
    let origin = transform.translation;
    let Some(index) = world.resource::<Index3d>() else {
        return;
    };
    let filter = Filter::new(ACTORS).excluding(Handle::entity(shooter));
    let hit = index
        .cast(origin, [20.0, 0.0, 0.0], 0.25, &filter)
        .map(|hit| (hit.handle, hit.t, hit.normal));
    let mut near: Vec<Neighbor> = Vec::new();
    index.k_nearest(origin, 2, 50.0, &filter, &mut near);
    let nearest = near.iter().map(|n| (n.handle, n.distance_sq)).collect();
    world.insert_resource(Shot { hit, nearest });
}

/// The game: rebuild, then shoot. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Shooter>("Shooter")?;
            Ok(())
        })
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("shoot", shoot)
}

/// A shooter at the origin, enemies 5 m and 10 m down the line and one 3 m off it.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: shooter
    components:
      Shooter:
      Transform: {}
      SpatialVolume:
        layers: 1
  - name: near_enemy
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
      SpatialVolume:
        layers: 1
  - name: far_enemy
    components:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
      SpatialVolume:
        layers: 1
  - name: flanker
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 3.0
      SpatialVolume:
        shape:
          aabb:
            half_extents:
              - 0.5
              - 1.0
              - 0.5
        layers: 1
";

// --- Headless demo `main` ------------------------------------------------------------------------

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
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");

    let shot = engine.world().resource::<Shot>().expect("shoot ran");
    let Some((handle, t, normal)) = shot.hit else {
        eprintln!("the shot hit nothing");
        return ExitCode::FAILURE;
    };
    println!("hit {handle:?} at t = {t:.4}, normal {normal:?}");
    for (handle, distance_sq) in &shot.nearest {
        println!("near {handle:?} at {:.2} m", distance_sq.sqrt());
    }
    // The sphere's centre stops 0.75 m short of the near enemy's centre: 4.25 m of a 20 m cast.
    if (t - 0.2125).abs() > 1e-4 || normal != [-1.0, 0.0, 0.0] || shot.nearest.len() != 2 {
        eprintln!("the shot did not behave as authored");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
