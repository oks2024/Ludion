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

#[test]
fn ai_describe_is_the_describe_ai_key_and_empty_without_ai() {
    let mut engine = boot();
    trace(&mut engine, 40);
    let description = match engine.execute(Command::EngineDescribe).unwrap().body {
        ResponseBody::EngineDescription { description } => description,
        other => panic!("expected a description, got {other:?}"),
    };
    let machines = match engine.execute(Command::AiDescribe).unwrap().body {
        ResponseBody::AiDescribe { machines } => machines,
        other => panic!("expected ai_describe, got {other:?}"),
    };
    assert_eq!(
        serde_json::to_value(&machines).unwrap(),
        description["ai"]["machines"],
        "one source: the typed wire list serializes to the describe key byte for byte"
    );
    assert_eq!(machines[0].machine, "Guard");
    assert_eq!(machines[0].agents.len(), 3);

    let mut bare = Engine::new();
    bare.load_inline_scene(
        "format: 1
entities: []
",
    )
    .unwrap();
    match bare.execute(Command::AiDescribe).unwrap().body {
        ResponseBody::AiDescribe { machines } => assert!(machines.is_empty()),
        other => panic!("expected ai_describe, got {other:?}"),
    }
}

// ---- ref-carrying events and the opt-in group roster ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
enum Sight {
    Saw {
        #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
        body: Option<EntityId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Scout {
    Root,
}

impl Machine for Scout {
    type Blackboard = ();
    type Event = Sight;
    const NAME: &'static str = "Scout";
    const ROOT: Self = Scout::Root;
    const ALL: &'static [Self] = &[Scout::Root];
    fn parent(_: Self) -> Option<Self> {
        None
    }
}

/// `squad` lists both scouts in its roster; `scout-a` has seen `scout-b`.
const SCOUTS: &str = "format: 1
seed: 1
entities:
  - name: squad
    components:
      Group: {}
      GroupRoster:
        members:
          - index: 1
            generation: 0
          - index: 2
            generation: 0
  - name: scout-a
    components:
      ScoutMind:
        inbox:
          - Saw:
              body:
                index: 2
                generation: 0
      GroupMember:
        group:
          index: 0
          generation: 0
        role: 0
  - name: scout-b
    components:
      ScoutMind: {}
      GroupMember:
        group:
          index: 0
          generation: 0
        role: 1
";

fn scouts(root: &std::path::Path) -> Engine {
    let mut engine = Engine::builder()
        .project_root(root)
        .components(register_machine::<Scout>)
        .build()
        .expect("boots");
    engine.load_inline_scene(SCOUTS).unwrap();
    engine
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-ai-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// (squad, scout-a, scout-b) by their components.
fn trio(world: &World) -> (EntityId, EntityId, EntityId) {
    let find = |role: u8| {
        world
            .entities()
            .find(|&e| {
                world
                    .component::<ludion_ai::GroupMember>(e)
                    .is_some_and(|m| m.role == role)
            })
            .expect("a scout")
    };
    let squad = world
        .entities()
        .find(|&e| world.component::<ludion_ai::Group>(e).is_some())
        .expect("a squad");
    (squad, find(0), find(1))
}

fn save(engine: &mut Engine, path: &str) {
    ludion_engine::dispatch(
        engine,
        Command::SceneSave(ludion_protocol::command::SceneSaveParams {
            path: path.to_string(),
            force: true,
        }),
    )
    .expect("saves");
}

fn load(engine: &mut Engine, path: &str) {
    ludion_engine::dispatch(
        engine,
        Command::SceneLoad(ludion_protocol::command::SceneLoadParams {
            path: path.to_string(),
        }),
    )
    .expect("loads");
}

#[test]
fn an_inbox_ref_follows_its_entity_through_save_and_reload_and_nulls_when_it_is_gone() {
    let root = scratch("inbox");
    let mut engine = scouts(&root);
    save(&mut engine, "scouts.yaml");
    load(&mut engine, "scouts.yaml");
    let (_, a, b) = trio(engine.world());
    let inbox = engine
        .world()
        .component::<Mind<Scout>>(a)
        .unwrap()
        .inbox
        .clone();
    assert_eq!(
        inbox,
        [Sight::Saw { body: Some(b) }],
        "the ref follows scout-b"
    );

    engine.world_mut().despawn(b).unwrap();
    save(&mut engine, "gone.yaml");
    load(&mut engine, "gone.yaml");
    let a = engine
        .world()
        .entities()
        .find(|&e| engine.world().component::<Mind<Scout>>(e).is_some())
        .unwrap();
    let inbox = engine
        .world()
        .component::<Mind<Scout>>(a)
        .unwrap()
        .inbox
        .clone();
    assert_eq!(
        inbox,
        [Sight::Saw { body: None }],
        "a dead target saves as null"
    );
}

#[test]
fn a_roster_round_trips_through_save_with_a_dead_member_nulled() {
    let root = scratch("roster");
    let mut engine = scouts(&root);
    let (squad, a, b) = trio(engine.world());
    assert_eq!(
        engine
            .world()
            .component::<ludion_ai::GroupRoster>(squad)
            .unwrap()
            .members,
        [Some(a), Some(b)]
    );
    engine.world_mut().despawn(b).unwrap();
    save(&mut engine, "roster.yaml");
    load(&mut engine, "roster.yaml");
    let (squad, a, _) = trio_after_loss(engine.world());
    assert_eq!(
        engine
            .world()
            .component::<ludion_ai::GroupRoster>(squad)
            .unwrap()
            .members,
        [Some(a), None]
    );
}

/// `trio` for a world where scout-b is gone: the squad and scout-a only.
fn trio_after_loss(world: &World) -> (EntityId, EntityId, EntityId) {
    let squad = world
        .entities()
        .find(|&e| world.component::<ludion_ai::Group>(e).is_some())
        .unwrap();
    let a = world
        .entities()
        .find(|&e| world.component::<Mind<Scout>>(e).is_some())
        .unwrap();
    (squad, a, a)
}

#[test]
fn snapshot_restore_keeps_the_inbox_ref_and_the_roster() {
    let root = scratch("snapshot");
    let mut engine = scouts(&root);
    let (squad, a, b) = trio(engine.world());
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut restored = Engine::builder()
        .project_root(&root)
        .components(register_machine::<Scout>)
        .build()
        .expect("boots");
    restored
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let world = restored.world();
    assert_eq!(
        world.component::<Mind<Scout>>(a).unwrap().inbox,
        [Sight::Saw { body: Some(b) }]
    );
    assert_eq!(
        world
            .component::<ludion_ai::GroupRoster>(squad)
            .unwrap()
            .members,
        [Some(a), Some(b)]
    );
}
