//! **Projectile patterns** — a ring of bullets fired on a keypress, and a dummy that counts the
//! ones that reach it.
//!
//! A gameplay bullet is not an entity: the entity is the [`Volley`], and bullet *i*'s trajectory is
//! a pure function of `(volley, bullet kind, i, now)`. One volley of 8 costs a few hundred hashed
//! bytes instead of eight entities, a whole-field cancel is a handful of despawns, and — because
//! the hit test the sim runs and the quads the renderer draws call the same [`volley_hits`]
//! arithmetic — what you are hit by is exactly what you saw.
//!
//! **The engine ships the vocabulary; the game owns every rule.** [`play_patterns`] stands volleys
//! up on schedule and [`advance_volleys`] reaps the spent ones — both plain `fn`s this game
//! registers in its own order. Who is hit, what damage means, and what happens to a bullet
//! afterward are `count_hits` below: it calls [`volley_hits`], increments its own counter, and sets
//! the volley's kill bit so one bullet lands one hit.
//!
//! **`started` is a stamp, not a trigger** — [`Animation`]'s rule, verbatim. `fire_pattern` writes
//! the current frame into [`PatternPlayer::started`] on the key's **down edge**; re-stamping every
//! tick would restart the timeline every tick and nothing would ever fire.
//!
//! Everything above the `main` divider is copyable into a real game: `game()`, `HitCounter`,
//! `fire_pattern` and `count_hits` are the whole pattern. The pattern document itself is content —
//! a `kind: pattern` file under the game's content root, hot-reloadable like any other data-doc.
//!
//! Run it headless (fires the ring and reports the hits):
//!
//! ```text
//! cargo run -p ludion --example pattern_player
//! ```

use ludion::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- The game (copyable) -------------------------------------------------------------------------

/// A target that counts the bullets reaching it. `radius` is its own body radius — [`volley_hits`]
/// adds the bullet kind's `hit_radius` to it, so neither side has to know the other's size.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
struct HitCounter {
    hits: u32,
    radius: f32,
}

/// The key that fires the pattern.
const FIRE: Key = Key::Space;

/// The pattern document, by its content-root-relative path — the key a [`DocStore`] holds it under.
const PATTERN: &str = "patterns/ring.yaml";

/// Starts the pattern on the key's **down edge**: names the document and stamps the current
/// gameplay frame — the clock the timeline is walked on.
///
/// `was_pressed` is the load-bearing half. Swap it for `is_held` and `started` moves every tick, so
/// the timeline never reaches its first entry and the turret is silent forever.
fn fire_pattern(world: &mut World, ctx: &SystemCtx) {
    let fired = world
        .resource::<InputState>()
        .is_some_and(|input| input.was_pressed(FIRE));
    if !fired {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    for id in ids {
        if let Ok(Some(player)) = world.get_mut::<PatternPlayer>(id) {
            player.pattern = PATTERN.to_string();
            player.started = ctx.game_frame;
        }
    }
}

/// Tests every live volley against every [`HitCounter`] and lands the overlaps.
///
/// The three-pass shape is the ordinary one for a system that reads a resource and writes
/// components: gather the targets, gather the hits while the `DocStore` borrow is alive, then
/// apply. Setting the kill bit is what makes a hit *count once* — a bullet that stays alive would
/// score again on every one of the ~20 frames it spends inside the circle.
fn count_hits(world: &mut World, ctx: &SystemCtx) {
    let plane = Plane::of(world);
    let axes = plane.axes();
    let ids: Vec<EntityId> = world.entities().collect();

    let targets: Vec<(EntityId, [f32; 2], f32)> = ids
        .iter()
        .filter_map(|&id| {
            let counter = *world.get::<HitCounter>(id).ok().flatten()?;
            let at = world.get::<Transform>(id).ok().flatten()?.translation;
            Some((id, [at[axes[0]], at[axes[1]]], counter.radius))
        })
        .collect();

    let mut landed: Vec<(EntityId, EntityId, u32)> = Vec::new();
    {
        let store = world.resource::<DocStore<PatternDoc>>();
        for &id in &ids {
            let Some(volley) = world.get::<Volley>(id).ok().flatten() else {
                continue;
            };
            let Some(origin) = world.get::<Transform>(id).ok().flatten() else {
                continue;
            };
            let kind = store
                .and_then(|store| store.get(&volley.pattern))
                .and_then(|doc| doc.bullets.get(&volley.bullet));
            let Some(kind) = kind else {
                continue;
            };
            for &(target, centre, radius) in &targets {
                let hits = volley_hits(volley, origin, kind, ctx.game_time, plane, centre, radius);
                landed.extend(hits.map(|bullet| (id, target, bullet)));
            }
        }
    }

    for (volley_id, target, bullet) in landed {
        let landed = match world.get_mut::<Volley>(volley_id) {
            Ok(Some(volley)) if !volley.is_killed(bullet) => {
                volley.kill(bullet);
                true
            }
            _ => false,
        };
        if landed && let Ok(Some(counter)) = world.get_mut::<HitCounter>(target) {
            counter.hits += 1;
        }
    }
}

/// The game: one component, the `pattern` doc kind, and the schedule.
///
/// The order is the whole contract — volleys are stood up, tested, then reaped, so a bullet killed
/// this frame is gone by the next and an emptied volley never survives a tick it cannot be seen in.
/// A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<HitCounter>("HitCounter"))
        .patterns()
        .system("fire_pattern", fire_pattern)
        .system("play_patterns", play_patterns)
        .system("count_hits", count_hits)
        .system("advance_volleys", advance_volleys)
}

/// Three rings of eight, twenty frames apart, each bullet living two seconds. Bullet 0 of every
/// ring launches along `+x` at 4 units a second, so it crosses the dummy three units out three
/// quarters of a second later; the other seven miss it by a wide margin.
///
/// **Block style throughout.** A flow-style mapping or sequence (`{ a: 1 }`, `[0.3, 0.3]`) in a
/// content document is skipped by the content walk, and the pattern would silently never load.
const RING_DOC: &str = "\
format: 1
kind: pattern
name: ring
bullets:
  pellet:
    size:
      - 0.2
      - 0.2
    color:
      - 1.0
      - 0.6
      - 0.2
      - 1.0
    blend: additive
    hit_radius: 0.2
    life: 120
timeline:
  - at: 0
    repeat: 3
    every: 20
    shape: ring
    bullet: pellet
    count: 8
    speed: 4.0
    angle_deg: 0.0
";

/// A camera, a turret whose [`PatternPlayer`] names nothing until the key is pressed, and a dummy
/// three units along `+x` counting what reaches it.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 8.0
          - 8.0
      Camera: {}
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer: {}
  - name: dummy
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      HitCounter:
        hits: 0
        radius: 0.5
";

/// One hit per ring — bullet 0 is the only one aimed at the dummy, and its kill bit stops it
/// scoring twice.
const EXPECTED_HITS: u32 = 3;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs five quiet ticks, taps the fire key, runs 120 more, and reports the dummy's tally.
/// Deterministic, no window. (A real game's `main` is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_pattern_doc(&root) {
        eprintln!("could not write the demo pattern document: {error}");
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

    for _ in 0..5 {
        step(&mut engine);
    }
    println!(
        "quiet       {} volleys, {} hits — a player naming no pattern fires nothing",
        volleys(&engine),
        hits(&engine)
    );

    engine.execute(key_down(FIRE)).expect("the key goes down");
    step(&mut engine);
    println!(
        "fired       {} volley on the frame the key went down",
        volleys(&engine)
    );

    for _ in 0..120 {
        step(&mut engine);
    }
    let tally = hits(&engine);
    println!(
        "settled     {tally} hits, {} volleys left — every ring landed its one aimed bullet",
        volleys(&engine)
    );

    if tally != EXPECTED_HITS {
        eprintln!("expected {EXPECTED_HITS} hits, counted {tally}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// A fresh, process-unique temp directory for the demo's pattern document (removed first, so a
/// re-run starts clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-pattern-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the pattern document under `root/patterns/`.
fn write_pattern_doc(root: &Path) -> std::io::Result<()> {
    let patterns = root.join("patterns");
    std::fs::create_dir_all(&patterns)?;
    std::fs::write(patterns.join("ring.yaml"), RING_DOC)
}

/// Advances one fixed tick, running the whole schedule.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

fn key_down(key: Key) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "key_down", "key": key }),
    })
}

/// How many volleys are on the field.
fn volleys(engine: &Engine) -> usize {
    let world = engine.world();
    world
        .entities()
        .filter(|&id| world.has::<Volley>(id))
        .count()
}

/// The dummy's tally.
fn hits(engine: &Engine) -> u32 {
    engine
        .world()
        .entities()
        .find_map(|id| engine.world().get::<HitCounter>(id).ok().flatten())
        .map(|counter| counter.hits)
        .expect("the scene has a HitCounter")
}
