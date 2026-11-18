//! `ludion-ai` over the real engine: a squad on a toy patrol machine, driven through the schedule,
//! hashed, and snapshotted mid-timer.

use ludion_ai::{Cx, Machine, Mind, Step, Timer, ai_tick, register_machine, send};
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Guard {
    Root,
    Patrol,
    Investigate,
    Return,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Memory {
    timer: Timer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
        match s {
            Guard::Investigate if cx.bb.timer.expired(cx.ctx) => Step::Go(Guard::Return),
            Guard::Return if cx.bb.timer.expired(cx.ctx) => Step::Go(Guard::Patrol),
            _ => Step::Stay,
        }
    }
    fn on_event(_: &mut Cx<'_, Self>, s: Self, _: &Heard) -> Step<Self> {
        if s == Guard::Patrol {
            Step::Go(Guard::Investigate)
        } else {
            Step::Stay
        }
    }
}

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

/// Frame 10: perception hears something and tells the leader.
fn hear(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 10 {
        return;
    }
    let leader = world
        .entities()
        .find(|&id| world.component::<Mind<Guard>>(id).is_some())
        .expect("a guard exists");
    assert!(send::<Guard>(world, leader, Heard::Noise));
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            register_machine::<Guard>(reg)?;
            reg.register::<Memory>("Memory")?;
            Ok(())
        })
        .system("hear", hear)
        .system("guard_ai", ai_tick::<Guard>)
}

fn boot() -> Engine {
    let mut engine = game().build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn states(engine: &Engine) -> Vec<Guard> {
    let world = engine.world();
    let ids: Vec<EntityId> = world.entities().collect();
    ids.into_iter()
        .filter_map(|id| world.component::<Mind<Guard>>(id).map(|m| m.state))
        .collect()
}

/// Advances to just after frame `frame` was simulated.
fn run_through(engine: &mut Engine, frame: u64) {
    while engine.frame().as_u64() <= frame {
        engine.advance_live();
    }
}

#[test]
fn the_squad_reacts_on_the_expected_ticks() {
    let mut engine = boot();
    run_through(&mut engine, 9);
    assert_eq!(states(&engine), [Guard::Patrol; 3]);
    run_through(&mut engine, 10);
    assert_eq!(
        states(&engine),
        [Guard::Investigate; 3],
        "the shout spreads the same tick"
    );
    run_through(&mut engine, 69);
    assert_eq!(states(&engine), [Guard::Investigate; 3]);
    run_through(&mut engine, 70);
    assert_eq!(
        states(&engine),
        [Guard::Return; 3],
        "1 s of gameplay time later"
    );
    run_through(&mut engine, 99);
    assert_eq!(states(&engine), [Guard::Return; 3]);
    run_through(&mut engine, 100);
    assert_eq!(states(&engine), [Guard::Patrol; 3]);
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn two_runs_hash_identically() {
    let (a, b) = (trace(&mut boot(), 130), trace(&mut boot(), 130));
    assert_eq!(a, b);
    assert!(
        a.windows(2).any(|w| w[0] != w[1]),
        "the hash moves while the squad does"
    );
}

#[test]
fn snapshot_restore_mid_timer_continues_bit_identically() {
    let mut original = boot();
    trace(&mut original, 40); // inside Investigate, timer running
    let snapshot = match original.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let reference = trace(&mut original, 100);

    let mut restored = game().build().expect("boots");
    match restored
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(states(&restored), [Guard::Investigate; 3]);
    assert_eq!(trace(&mut restored, 100), reference);
    assert_eq!(states(&restored), states(&original));
}
