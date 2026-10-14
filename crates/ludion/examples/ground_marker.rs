//! Ground telegraph — the copyable pattern for a charge-line [`GroundMarker`] that fills in as its
//! warmup counts down.
//!
//! **The one rule worth learning: animate the shape field, never `Transform.scale`.** A
//! [`GroundMarkerShape::Segment`]'s `length` and `width` both multiply by the entity's *same*
//! `Transform.scale.x` (the marker's only scale axis — the other two are ignored), so scaling a
//! charge lane down to represent a partial fill shrinks its `width` right along with its `length`:
//! a lane authored 8 × 2 world units reads a quarter as *wide* as authored at `scale.x = 0.25`, not
//! merely shorter. The correct idiom writes the shape field that should change (`length: reach *
//! fill`) and leaves `Transform.scale` at its default `1.0` — the width the player sees stays the
//! width that was authored, at every fill level.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, `Charge`, and
//! `charge_telegraph` are the whole pattern.
//!
//! Run it headless (steps the warmup and prints the marker's resolved footprint):
//!
//! ```text
//! cargo run -p ludion --example ground_marker
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// Tags the entity whose segment marker is a charge telegraph, and stamps when the charge began.
/// `started` is a stamp, not a trigger — [`Animation`]'s rule verbatim: write it once, when the
/// charge is chosen, never every tick.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Charge {
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    started: FrameIndex,
}

impl Default for Charge {
    fn default() -> Self {
        Charge {
            started: FrameIndex::ZERO,
        }
    }
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

/// How many frames the telegraph takes to fill.
const CHARGE_FRAMES: u64 = 20;
/// The lane's full length once charged — the shape field that animates.
const REACH: f32 = 8.0;
/// The lane's width, held fixed for the whole charge and kept below `REACH` so the rounded-box SDF
/// never degenerates past a disc (`GroundMarkerShape::Segment`'s doc).
const WIDTH: f32 = 2.0;

/// Fills every [`Charge`]d segment marker's `length` linearly toward `REACH`, clamped at full once
/// the warmup elapses. `width` is never touched — it is not part of the animated dimension — and
/// `Transform.scale` is never written at all, which is the whole point of the example.
fn charge_telegraph(world: &mut World, ctx: &SystemCtx) {
    for (charge, marker) in world.query::<(&Charge, &mut GroundMarker)>() {
        let elapsed = ctx
            .game_frame
            .as_u64()
            .saturating_sub(charge.started.as_u64());
        let fill = (elapsed as f32 / CHARGE_FRAMES as f32).min(1.0);
        marker.shape = GroundMarkerShape::Segment {
            length: REACH * fill,
            width: WIDTH,
        };
    }
}

/// The game: one marker component and one system. A real game's `main` is `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Charge>("Charge"))
        .system("charge_telegraph", charge_telegraph)
}

/// A camera looking down a lane, and one segment marker charging from the origin along local +X.
/// `length` starts small rather than zero — `L2106` rejects a non-positive shape dimension at load,
/// and the system overwrites it on the very first tick regardless.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 10.0
          - 12.0
        rotation:
          - -35.0
          - 0.0
          - 0.0
      Camera: {}
  - name: lane
    components:
      Charge: {}
      GroundMarker:
        shape:
          type: segment
          length: 0.01
          width: 2.0
        color:
          - 2.2
          - 0.5
          - 0.1
          - 0.85
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Steps the warmup to completion, printing the marker's resolved `length`/`width` at even
/// checkpoints. No window, deterministic. (A real game's `main` is just `ludion::run(game())`.)
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

    println!("frame  length  width   fill");
    let mut last_length = -1.0;
    for frame in 0..=CHARGE_FRAMES {
        step(&mut engine);
        let (length, width) = lane_footprint(&engine);
        println!(
            "{frame:>5}  {length:>6.2}  {width:>5.2}   {:>4.0}%",
            (length / REACH) * 100.0
        );
        last_length = length;
    }

    if last_length != REACH {
        eprintln!("the charge did not reach its full length");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Advances one fixed tick, running the charge system.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// The lane's current `(length, width)`, read back off its `GroundMarker`.
fn lane_footprint(engine: &Engine) -> (f32, f32) {
    let world = engine.world();
    for entity in world.entities() {
        if world.get::<Charge>(entity).unwrap().is_none() {
            continue;
        }
        let marker = world
            .get::<GroundMarker>(entity)
            .unwrap()
            .expect("the lane has a GroundMarker");
        let GroundMarkerShape::Segment { length, width } = marker.shape else {
            panic!("the lane is a segment");
        };
        return (length, width);
    }
    panic!("the scene has a charging lane");
}
