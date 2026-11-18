//! `ludion-ai` over the real engine: a squad on a toy patrol machine, driven through the schedule,
//! hashed, snapshotted mid-timer, and traced (`ai`, `ai_states`) against committed fixtures.

use ludion_ai::{Cx, Machine, Mind, Step, Timer, ai_tick, register_machine, send};
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AiTransitionRecord, Engine, EngineBuilder, TraceChannel, TraceRecord, trace_with, write_jsonl,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::InputScript;
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

const SCENE: &str = include_str!("fixtures/ai-squad.yaml");

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

const TICKS: usize = 130;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// One `<tick:04> <hash:016x>` line per tick.
fn hash_lines(engine: &mut Engine, ticks: usize) -> Vec<String> {
    trace(engine, ticks)
        .iter()
        .enumerate()
        .map(|(tick, hash)| format!("{tick:04} {hash:016x}"))
        .collect()
}

#[test]
fn the_squad_matches_the_committed_world_hash_trace() {
    let lines = hash_lines(&mut boot(), TICKS);
    let path = fixture("ai-squad.trace.txt");
    if ludion_core::bless::bless_requested() {
        let header = "# Per-tick WorldHash of crates/ludion-engine/tests/fixtures/ai-squad.yaml under the `Guard` machine in
# crates/ludion-engine/tests/ai.rs.
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(ai::the_squad_matches)'
# The M4 must reproduce this file byte-identically - never fabricated by copying the Windows trace.
";
        std::fs::write(
            &path,
            format!(
                "{header}{}
",
                lines.join(
                    "
"
                )
            ),
        )
        .expect("fixtures exist");
        return;
    }
    let committed: Vec<String> = std::fs::read_to_string(&path)
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in lines.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }
}

fn ai_trace(channels: &[TraceChannel]) -> Vec<TraceRecord> {
    trace_with(
        game(),
        &fixture("ai-squad.yaml"),
        &InputScript::new(),
        TICKS as u64,
        channels,
    )
    .expect("the squad traces")
}

fn jsonl(records: &[TraceRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    write_jsonl(records, &mut out).unwrap();
    out
}

#[test]
fn the_ai_channel_matches_the_committed_fixture_and_two_runs_agree() {
    let bytes = jsonl(&ai_trace(&[TraceChannel::Ai]));
    assert_eq!(bytes, jsonl(&ai_trace(&[TraceChannel::Ai])), "two runs");
    let path = fixture("ai-squad.ai.jsonl");
    if ludion_core::bless::bless_requested() {
        std::fs::write(&path, &bytes).expect("fixtures exist");
        return;
    }
    let committed = std::fs::read(&path).expect("the committed ai fixture");
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        String::from_utf8_lossy(&committed),
        "the ai channel moved; re-bless with LUDION_BLESS=1 if intended"
    );
}

#[test]
fn the_ai_lines_land_on_the_expected_ticks_with_their_causes() {
    let lines: Vec<AiTransitionRecord> = ai_trace(&[TraceChannel::Ai])
        .into_iter()
        .map(|r| match r {
            TraceRecord::Ai(t) => t,
            other => panic!("only ai lines were asked for, got {other:?}"),
        })
        .collect();
    let at = |tick: u64| -> Vec<&AiTransitionRecord> {
        lines.iter().filter(|t| t.tick == tick).collect()
    };
    assert_eq!(
        lines.len(),
        12,
        "start, investigate, return, patrol, for three agents"
    );
    for (tick, cause, to) in [
        (0, "start", "Patrol"),
        (10, "event:Noise", "Investigate"),
        (70, "update", "Return"),
        (100, "update", "Patrol"),
    ] {
        let group = at(tick);
        assert_eq!(group.len(), 3, "tick {tick}");
        for line in group {
            assert_eq!(line.cause, cause, "tick {tick}");
            assert_eq!(line.to.last().map(String::as_str), Some(to), "tick {tick}");
            let event = cause
                .starts_with("event:")
                .then(|| serde_json::json!("Noise"));
            assert_eq!(line.event, event, "tick {tick}");
        }
    }
    assert!(at(0).iter().all(|t| t.from.is_empty()));
    assert!(at(10).iter().all(|t| t.from == ["Root", "Patrol"]));
}

#[test]
fn ai_states_prints_one_line_per_agent_per_tick() {
    let records = ai_trace(&[TraceChannel::AiStates]);
    assert_eq!(records.len(), 3 * TICKS);
    for (expected_tick, chunk) in (0u64..).zip(records.chunks(3)) {
        let names: Vec<&str> = chunk
            .iter()
            .map(|r| match r {
                TraceRecord::AiStates(s) => {
                    assert_eq!(s.tick, expected_tick);
                    assert_eq!(s.machine, "Guard");
                    s.entity.as_str()
                }
                other => panic!("only ai_states lines were asked for, got {other:?}"),
            })
            .collect();
        assert_eq!(
            names,
            ["leader", "flank-a", "flank-b"],
            "ascending entity order"
        );
    }
}

#[test]
fn recording_the_default_channels_does_not_move_the_world_hash() {
    let plain = trace(&mut boot(), TICKS);
    let mut engine = boot();
    let mut traced = Vec::new();
    ludion_engine::trace(&mut engine, &InputScript::new(), 0, &TraceChannel::ALL).unwrap();
    for _ in 0..TICKS {
        ludion_engine::trace(&mut engine, &InputScript::new(), 1, &TraceChannel::ALL).unwrap();
        traced.push(engine.world_hash().unwrap().as_u64());
    }
    assert_eq!(plain, traced);
}

#[test]
fn describe_carries_the_machine_and_its_live_agents() {
    let mut engine = boot();
    trace(&mut engine, 40);
    let description = match engine.execute(Command::EngineDescribe).unwrap().body {
        ResponseBody::EngineDescription { description } => description,
        other => panic!("expected a description, got {other:?}"),
    };
    let machine = &description["ai"]["machines"][0];
    assert_eq!(machine["machine"], "Guard");
    assert_eq!(machine["component"], "GuardMind");
    assert_eq!(machine["game_tick"], 39);
    assert_eq!(machine["root"], "Root");
    assert_eq!(machine["states"][0]["initial"], "Patrol");
    let agents = machine["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 3);
    for agent in agents {
        assert_eq!(agent["path"], serde_json::json!(["Root", "Investigate"]));
        assert_eq!(agent["entered"][1], 10);
        assert_eq!(agent["ticks_in"][1], 29, "game tick 39 - entered 10");
    }
    assert_eq!(agents[0]["name"], "leader");
}
