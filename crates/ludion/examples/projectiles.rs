//! **A turret that fires two projectiles** — a piercing shot and a despawning shot down a row of
//! three enemies, and a `score` system that prints the hits after each step.
//!
//! **The game schedules the step.** `projectiles_step` runs after `spatial3d_rebuild`, so the shots
//! sweep against this tick's enemy volumes; `score` runs after the step and reads its events.
//! `fire` spawns through `spawn_projectile`, which reports a refusal (`L4026`) instead of panicking.
//!
//! Everything above the `main` divider is copyable into a real game.
//!
//! Run it headless (prints the hits):
//!
//! ```text
//! cargo run -p ludion --example projectiles
//! ```

use ludion::prelude::*;
use projectiles::{HitOutcome, ProjectileDesc, Response};
use spatial3d::Handle;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The hits `score` has seen so far.
#[derive(Default)]
struct Score {
    hits: Vec<(Handle, HitOutcome, u32)>,
}

/// Tick 0: a shot that pierces one enemy (tag 1) and a shot that stops at the first (tag 2).
fn fire(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    let mut piercing = ProjectileDesc::new([0.0, 0.0, 0.0], [60.0, 0.0, 0.0]);
    piercing.response = Response::Pierce { max: 1 };
    piercing.tag = 1;
    let mut plain = ProjectileDesc::new([0.0, 0.0, 3.0], [60.0, 0.0, 0.0]);
    plain.tag = 2;
    spawn_projectile(world, piercing);
    spawn_projectile(world, plain);
}

/// Collects the tick's hits. A real game would apply damage here.
fn score(world: &mut World, _ctx: &SystemCtx) {
    let hits: Vec<_> = world
        .resource::<Projectiles>()
        .map(|p| {
            p.events()
                .hits
                .iter()
                .map(|h| (h.handle, h.outcome, h.tag))
                .collect()
        })
        .unwrap_or_default();
    let tally = world.resource_mut::<Score>().expect("inserted by main");
    tally.hits.extend(hits);
}

/// How each tag draws: tag 1 a stretched orange tracer, tag 2 a small cube; any other tag falls
/// back to the default white billboard. Used only by a windowed or rendered run.
fn visuals() -> ProjectileVisuals {
    let mut visuals = ProjectileVisuals::default();
    visuals.tags.insert(
        1,
        ProjectileVisual {
            look: ProjectileLook::Billboard { softness: 0.6 },
            size: 0.08,
            stretch: 0.02,
            color: [1.0, 0.6, 0.2, 1.0],
            ..ProjectileVisual::default()
        },
    );
    visuals.tags.insert(
        2,
        ProjectileVisual {
            look: ProjectileLook::Cube,
            size: 0.1,
            color: [0.2, 0.8, 1.0, 1.0],
            emissive: 2.0,
            ..ProjectileVisual::default()
        },
    );
    visuals
}

/// The game: rebuild the target index, fire, step the pool, score.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .projectiles(ProjectileConfig::default())
        .projectile_visuals(visuals())
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire", fire)
        .system("projectiles_step", projectiles_step)
        .system("score", score)
}

/// Enemies at x = 5, 8 and 11 on the first shot's line; one more on the second shot's line.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: enemy_a
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
      SpatialVolume: {}
  - name: enemy_b
    components:
      Transform:
        translation:
          - 8.0
          - 0.0
          - 0.0
      SpatialVolume: {}
  - name: enemy_c
    components:
      Transform:
        translation:
          - 11.0
          - 0.0
          - 0.0
      SpatialVolume: {}
  - name: enemy_d
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 3.0
      SpatialVolume: {}
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
    engine.world_mut().insert_resource(Score::default());
    // 60 m/s is 1 m a tick: the last enemy is reached well inside 20 ticks.
    for _ in 0..20 {
        engine.advance_live();
    }

    let tally = engine.world().resource::<Score>().expect("scored");
    for (handle, outcome, tag) in &tally.hits {
        println!("shot {tag} hit {handle:?}: {outcome:?}");
    }
    // The piercing shot pierces enemy_a and despawns on enemy_b; the plain shot despawns on enemy_d.
    let by_tag = |tag| {
        tally
            .hits
            .iter()
            .filter(|h| h.2 == tag)
            .map(|h| h.1)
            .collect::<Vec<_>>()
    };
    if by_tag(1) != [HitOutcome::Pierced, HitOutcome::Despawned]
        || by_tag(2) != [HitOutcome::Despawned]
    {
        eprintln!("the shots did not behave as authored");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
