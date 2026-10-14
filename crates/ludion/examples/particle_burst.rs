//! **Particles** — an ambient plume that is always there, and a burst that fires on a keypress.
//!
//! A [`ParticleEmitter`] is a *description*, not a machine: nothing is spawned, pooled or stepped.
//! Every frame, the render boundary computes the live set analytically from the component and the
//! current sim time, so a particle is never an entity, never a world-hash term and never a replay
//! record. Two consequences the demo leans on: an endless emitter (`duration: 0`) is in **steady
//! state at frame 0** — the brazier's plume is already full on the frame the scene loads, with no
//! warm-up — and a paused engine renders the same frame twice.
//!
//! **The one rule worth learning: `started` is a stamp, not a trigger.** A burst releases its
//! particles at `started` and is spent one lifetime later, so firing an effect is a single write of
//! the *current gameplay frame* (`ctx.game_frame`, never `ctx.frame`) into that field. Write it
//! when the **choice** happens (the key went down, the shot landed), never every frame —
//! re-stamping `started` on every tick restarts the burst on every tick, and the effect freezes at
//! age zero forever. That is [`Animation`]'s rule verbatim, for exactly the same reason.
//!
//! Everything above the `main` divider is copyable into a real game: `game()`, `Detonator`, and
//! `fire_burst` are the whole pattern.
//!
//! Run it headless (scripts a keypress and reports both emitters):
//!
//! ```text
//! cargo run -p ludion --example particle_burst
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- The game (copyable) -------------------------------------------------------------------------

/// Marks the emitter a keypress fires, so `fire_burst` stamps that one and leaves the ambient plume
/// alone. A marker component, the `Residue`/`GiDynamic` shape: no fields, nothing to get wrong.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct Detonator {}

/// The key that fires the burst.
const FIRE: Key = Key::Space;

/// Fires every marked emitter by stamping it with the frame the key went **down** on.
///
/// `was_pressed` is the load-bearing half: it is an edge, so holding the key stamps once instead of
/// once per tick. Swap it for `is_held` and the burst never ages past zero — which is the failure
/// mode this system exists to demonstrate the shape of avoiding.
fn fire_burst(world: &mut World, ctx: &SystemCtx) {
    let fired = world
        .resource::<InputState>()
        .is_some_and(|input| input.was_pressed(FIRE));
    if !fired {
        return;
    }
    let marked: Vec<EntityId> = world
        .entities()
        .filter(|&e| world.get::<Detonator>(e).unwrap().is_some())
        .collect();
    for entity in marked {
        if let Some(emitter) = world.get_mut::<ParticleEmitter>(entity).unwrap() {
            emitter.started = ctx.game_frame;
        }
    }
}

/// The game: one marker component and one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Detonator>("Detonator"))
        .system("fire_burst", fire_burst)
}

/// A camera, the always-on brazier plume, and the burst waiting for the key.
///
/// The brazier is endless (`duration` left at its default `0`) and rises (`gravity: -1.2`) with a
/// little drag, softly, additively — an ember bed. The spark burst is `rate: 0` (no continuous
/// emission at all), 60 particles thrown over the whole sphere and pulled down hard, clipped at the
/// ground by `floor` so nothing sinks through it.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
  - name: brazier
    components:
      Transform:
        translation:
          - -1.5
          - 0.5
          - 0.0
      ParticleEmitter:
        shape: disc
        extent:
          - 0.3
          - 0.0
          - 0.0
        rate: 35.0
        life:
          - 1.2
          - 2.0
        speed:
          - 0.4
          - 0.9
        spread_deg: 20.0
        gravity: -1.2
        drag: 0.6
        start_size: 0.18
        end_size: 0.02
        start_color:
          - 3.0
          - 1.4
          - 0.4
          - 1.0
        end_color:
          - 0.6
          - 0.1
          - 0.0
          - 0.0
        blend: additive
        softness: 0.8
        seed: 101
  - name: sparks
    components:
      Detonator: {}
      Transform:
        translation:
          - 1.5
          - 0.4
          - 0.0
      ParticleEmitter:
        shape: sphere
        extent:
          - 0.1
          - 0.1
          - 0.1
        rate: 0.0
        burst: 60
        life:
          - 0.4
          - 0.9
        speed:
          - 3.0
          - 7.0
        spread_deg: 180.0
        gravity: 14.0
        drag: 1.5
        floor: 0.0
        start_size: 0.09
        end_size: 0.0
        start_color:
          - 4.0
          - 3.0
          - 1.6
          - 1.0
        end_color:
          - 1.0
          - 0.3
          - 0.0
          - 0.0
        blend: additive
        softness: 0.2
        seed: 202
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs ten quiet ticks, taps the fire key, runs ten more, and reports what each emitter is doing.
/// Deterministic, no window. (A real game's `main` is just `ludion::run(game())`.)
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

    for _ in 0..10 {
        engine.execute(step()).expect("the session drives");
    }
    let before = burst_started(&mut engine);

    engine.execute(key_down(FIRE)).expect("the key goes down");
    engine.execute(step()).expect("the session drives");
    let stamped = burst_started(&mut engine);

    // Ten more ticks with the key still held: `was_pressed` is an edge, so the stamp does not move
    // and the burst is allowed to age.
    for _ in 0..10 {
        engine.execute(step()).expect("the session drives");
    }
    let held = burst_started(&mut engine);

    println!("brazier      endless, in steady state since frame 0 — no warm-up, no trigger");
    println!("sparks       started {before} before the key, {stamped} after it fired");
    println!("             still {held} ten ticks later, because the key press is an edge");
    if stamped == before || held != stamped {
        eprintln!("the burst did not stamp exactly once");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The burst emitter's current `started` frame.
fn burst_started(engine: &mut Engine) -> u64 {
    engine
        .world_mut()
        .query::<(&ParticleEmitter, &Detonator)>()
        .next()
        .map(|(emitter, _)| emitter.started.as_u64())
        .expect("the scene has a marked emitter")
}

fn key_down(key: Key) -> Command {
    Command::InputInject(command::InputInjectParams {
        frame: None,
        event: json!({ "kind": "key_down", "key": key }),
    })
}

fn step() -> Command {
    Command::SimStep(command::SimStepParams {
        ticks: 1,
        hash: false,
    })
}
