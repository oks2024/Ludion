//! **A squad of three guards on a patrol → investigate → return machine.**
//!
//! `ai::register_machine` registers the `GuardMind` component (and `Group`/`GroupMember`); `ai_tick`
//! runs every guard once per gameplay tick. A perception system tells the leader it heard something
//! on frame 10; the leader's `enter(Investigate)` broadcasts to the squad, and a `Timer` brings
//! everyone back. Each agent's active path is printed whenever it changes.
//!
//! Run it headless:
//!
//! ```text
//! cargo run -p ludion --example ai_squad
//! ```

use ludion::ai::{Cx, Machine, Mind, Step, Timer};
use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Guard {
    Root,
    Patrol,
    Investigate,
    Return,
}

#[derive(Default, Serialize, Deserialize, JsonSchema)]
struct Memory {
    timer: Timer,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
enum Heard {
    Noise,
}

impl Machine for Guard {
    type Blackboard = Memory;
    type Event = Heard;
    const NAME: &'static str = "Guard";
    const ROOT: Self = Guard::Root;
    const ALL: &'static [Self] = &[
        Guard::Root,
        Guard::Patrol,
        Guard::Investigate,
        Guard::Return,
    ];

    fn parent(s: Self) -> Option<Self> {
        (s != Guard::Root).then_some(Guard::Root)
    }
    fn initial(s: Self) -> Option<Self> {
        (s == Guard::Root).then_some(Guard::Patrol)
    }
    fn enter(cx: &mut Cx<'_, Self>, s: Self) {
        match s {
            Guard::Investigate => {
                cx.bb.timer.start(cx.ctx, 1.0);
                if let Some(squad) = cx.group {
                    cx.broadcast(squad, &Heard::Noise);
                }
            }
            Guard::Return => cx.bb.timer.start(cx.ctx, 0.5),
            _ => {}
        }
    }
    fn on_event(_: &mut Cx<'_, Self>, s: Self, _: &Heard) -> Step<Self> {
        if s == Guard::Patrol {
            Step::Go(Guard::Investigate)
        } else {
            Step::Stay
        }
    }
    fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
        match s {
            Guard::Investigate if cx.bb.timer.expired(cx.ctx) => Step::Go(Guard::Return),
            Guard::Return if cx.bb.timer.expired(cx.ctx) => Step::Go(Guard::Patrol),
            _ => Step::Stay,
        }
    }
}

/// Perception stand-in: on frame 10 the leader (the first guard) hears a noise.
fn hear(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 10 {
        return;
    }
    let leader = world
        .entities()
        .find(|&id| world.component::<Mind<Guard>>(id).is_some());
    if let Some(leader) = leader {
        ludion::ai::send::<Guard>(world, leader, Heard::Noise);
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|registry| {
            ludion::ai::register_machine::<Guard>(registry)?;
            registry.register::<Memory>("Memory")
        })
        .system("hear", hear)
        .system("guard_ai", ludion::ai::ai_tick::<Guard>)
}

// --- Harness (not part of the game) ---------------------------------------------------------------

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: squad
    components:
      Group:
        kind: patrol
  - name: leader
    components:
      GuardMind: {}
      GroupMember:
        group:
          index: 0
          generation: 0
        role: 0
  - name: flank-a
    components:
      GuardMind: {}
      GroupMember:
        group:
          index: 0
          generation: 0
        role: 1
  - name: flank-b
    components:
      GuardMind: {}
      GroupMember:
        group:
          index: 0
          generation: 0
        role: 2
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
    let mut last: Vec<Vec<Guard>> = Vec::new();
    for _ in 0..110 {
        engine.advance_live();
        let world = engine.world();
        let paths: Vec<Vec<Guard>> = world
            .entities()
            .filter(|&id| world.component::<Mind<Guard>>(id).is_some())
            .map(|id| ludion::ai::active_path::<Guard>(world, id))
            .collect();
        if paths != last {
            println!("frame {:>3}: {paths:?}", engine.frame().as_u64() - 1);
            last = paths;
        }
    }
    ExitCode::SUCCESS
}
