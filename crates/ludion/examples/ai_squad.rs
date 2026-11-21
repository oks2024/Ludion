//! **A squad of three guards on a patrol → investigate → return machine.**
//!
//! `ai::register_machine` registers the `GuardMind` component (and `Group`/`GroupMember`); `ai_tick`
//! runs every guard once per gameplay tick. A perception system tells the leader it heard something
//! on frame 10; the leader's `enter(Investigate)` broadcasts to the squad, and a `Timer` brings
//! everyone back. Each agent's active path is printed whenever it changes. The squad also carries a
//! `GroupRoster`: when a flanker is despawned, `Machine::member_left` builds a `Heard::Down` that names
//! it (a nullable entity reference in the event) and the remaining guards receive it the same tick.
//!
//! Run it headless:
//!
//! ```text
//! cargo run -p ludion --example ai_squad
//! ```

use ludion::ai::{Cx, GroupRoster, Machine, Mind, Step, Timer};
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
    /// Who the squad lost, as the `Heard::Down` events named them.
    #[serde(default)]
    #[schemars(with = "EntityRefCollection<Vec<Option<EntityRefSchema>>>")]
    lost: Vec<Option<EntityId>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
enum Heard {
    Noise,
    /// A squadmate left; `None` when the member was despawned across a save before it was seen.
    Down {
        #[schemars(with = "Option<EntityRefSchema>")]
        who: Option<EntityId>,
    },
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
    fn on_event(cx: &mut Cx<'_, Self>, s: Self, e: &Heard) -> Step<Self> {
        match e {
            Heard::Noise if s == Guard::Patrol => Step::Go(Guard::Investigate),
            // Offered at every active level; record it once, at the root.
            Heard::Down { who } if s == Guard::Root => {
                cx.bb.lost.push(*who);
                Step::Stay
            }
            Heard::Noise | Heard::Down { .. } => Step::Stay,
        }
    }
    fn member_left(member: Option<EntityId>) -> Option<Heard> {
        Some(Heard::Down { who: member })
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
      GroupRoster: {}
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
    let rosters = engine
        .world()
        .entities()
        .filter(|&id| engine.world().component::<GroupRoster>(id).is_some())
        .count();
    println!("squads with a roster: {rosters}");
    let mut last: Vec<Vec<Guard>> = Vec::new();
    let mut gone = None;
    for tick in 0..110 {
        if tick == 50 {
            // The last flanker is despawned; the roster notices on the next guard tick.
            let flank = engine
                .world()
                .entities()
                .filter(|&id| engine.world().component::<Mind<Guard>>(id).is_some())
                .last();
            if let Some(flank) = flank {
                let _ = engine.world_mut().despawn(flank);
                gone = Some(flank);
            }
        }
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
    let world = engine.world();
    let heard: Vec<Vec<Option<EntityId>>> = world
        .entities()
        .filter_map(|id| world.component::<Memory>(id).map(|m| m.lost.clone()))
        .collect();
    println!("member left: {gone:?} -> each remaining guard heard {heard:?}");
    if gone.is_none() || heard.len() != 2 || heard.iter().any(|lost| *lost != [gone]) {
        eprintln!(
            "the remaining guards did not each receive one MemberLeft for the despawned flanker"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
