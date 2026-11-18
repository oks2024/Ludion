//! `AiTrace` and `AiRoster`: what `ai_tick` records, and that recording moves nothing.

use ludion_ai::{
    AiCause, AiFaults, AiRecord, AiRoster, AiState, AiTrace, AiTransition, Cx, Machine, Mind, Step,
    ai_tick, send,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::{SystemCtx, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Ev {
    ToB,
    Ping(u8),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Log {
    /// Update at the `A1` leaf: index into `ALL`; `100 + i` restarts.
    next: Option<u8>,
    /// `enter(B)` bounces the agent to `A2`, `enter(A2)` back to `B`.
    bounce: bool,
    /// `enter(A2)` chains to `B` once.
    chain_once: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Tm {
    Root,
    A,
    A1,
    A2,
    B,
}

impl Machine for Tm {
    type Blackboard = Log;
    type Event = Ev;
    const NAME: &'static str = "Tm";
    const ROOT: Self = Tm::Root;
    const ALL: &'static [Self] = &[Tm::Root, Tm::A, Tm::A1, Tm::A2, Tm::B];
    fn parent(s: Self) -> Option<Self> {
        match s {
            Tm::Root => None,
            Tm::A | Tm::B => Some(Tm::Root),
            Tm::A1 | Tm::A2 => Some(Tm::A),
        }
    }
    fn initial(s: Self) -> Option<Self> {
        match s {
            Tm::Root => Some(Tm::A),
            Tm::A => Some(Tm::A1),
            _ => None,
        }
    }
    fn enter(cx: &mut Cx<'_, Self>, s: Self) {
        if cx.bb.bounce {
            match s {
                Tm::B => cx.go(Tm::A2),
                Tm::A2 => cx.go(Tm::B),
                _ => {}
            }
        }
        if s == Tm::A2 && std::mem::take(&mut cx.bb.chain_once) {
            cx.go(Tm::B);
        }
    }
    fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
        if s == Tm::A1
            && let Some(i) = cx.bb.next.take()
        {
            return match i {
                100.. => Step::Restart(Self::ALL[usize::from(i - 100)]),
                i => Step::Go(Self::ALL[usize::from(i)]),
            };
        }
        Step::Stay
    }
    fn on_event(_: &mut Cx<'_, Self>, s: Self, e: &Ev) -> Step<Self> {
        match (*e, s) {
            (Ev::ToB, Tm::Root) => Step::Go(Tm::B),
            _ => Step::Stay,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Sn {
    Root,
    LookAround,
}

impl Machine for Sn {
    type Blackboard = ();
    type Event = ();
    const NAME: &'static str = "Sn";
    const ROOT: Self = Sn::Root;
    const ALL: &'static [Self] = &[Sn::Root, Sn::LookAround];
    fn parent(s: Self) -> Option<Self> {
        (s != Sn::Root).then_some(Sn::Root)
    }
    fn initial(s: Self) -> Option<Self> {
        (s == Sn::Root).then_some(Sn::LookAround)
    }
}

fn ctx(frame: u64) -> SystemCtx {
    SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT)
}

fn held_ctx() -> SystemCtx {
    let mut clock = GameClock::default();
    clock.hold();
    let advanced = clock.advance();
    ctx(0).with_clock(&clock, advanced)
}

fn spawn<M: Machine>(world: &mut World, mind: Mind<M>) -> EntityId {
    let e = world.spawn();
    world.insert(e, mind).unwrap();
    e
}

fn traced(transitions: bool, states: bool) -> World {
    let mut world = World::new();
    world.insert_resource(AiTrace::new(transitions, states));
    world
}

fn take(world: &mut World) -> Vec<AiRecord> {
    world.resource_mut::<AiTrace>().unwrap().take()
}

fn transitions(records: Vec<AiRecord>) -> Vec<AiTransition> {
    records
        .into_iter()
        .filter_map(|r| match r {
            AiRecord::Transition(t) => Some(t),
            AiRecord::State(_) => None,
        })
        .collect()
}

fn state_lines(records: Vec<AiRecord>) -> Vec<AiState> {
    records
        .into_iter()
        .filter_map(|r| match r {
            AiRecord::State(s) => Some(s),
            AiRecord::Transition(_) => None,
        })
        .collect()
}

fn names(path: &[&str]) -> Vec<String> {
    path.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn start_event_update_and_chain_carry_their_causes() {
    let mut world = traced(true, false);
    let e = spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    let start = transitions(take(&mut world));
    assert_eq!(start.len(), 1);
    assert_eq!(start[0].cause, AiCause::Start);
    assert_eq!(start[0].from, Vec::<String>::new());
    assert_eq!(start[0].to, names(&["Root", "A", "A1"]));
    assert_eq!(
        (start[0].tick, start[0].entity, start[0].machine),
        (0, e, "Tm")
    );

    assert!(send::<Tm>(&mut world, e, Ev::ToB));
    ai_tick::<Tm>(&mut world, &ctx(1));
    let event = transitions(take(&mut world));
    assert_eq!(event.len(), 1);
    assert_eq!(event[0].cause, AiCause::Event(serde_json::json!("ToB")));
    assert_eq!(event[0].from, names(&["Root", "A", "A1"]));
    assert_eq!(event[0].to, names(&["Root", "B"]));
    assert_eq!(event[0].tick, 1);

    let mut world = traced(true, false);
    let e = spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    take(&mut world);
    world.component_mut::<Log>(e).unwrap().next = Some(4);
    ai_tick::<Tm>(&mut world, &ctx(1));
    let update = transitions(take(&mut world));
    assert_eq!(update.len(), 1);
    assert_eq!(update[0].cause, AiCause::Update);
    assert_eq!(update[0].to, names(&["Root", "B"]));

    // `enter(A2)` chains to B: the update's transition is `Update`, the bounce is `Chain`.
    let mut world = traced(true, false);
    let e = spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    take(&mut world);
    {
        let bb = world.component_mut::<Log>(e).unwrap();
        bb.next = Some(3); // A2
        bb.chain_once = true;
    }
    ai_tick::<Tm>(&mut world, &ctx(1));
    let chain = transitions(take(&mut world));
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].cause, AiCause::Update);
    assert_eq!(chain[0].to, names(&["Root", "A", "A2"]));
    assert_eq!(chain[1].cause, AiCause::Chain);
    assert_eq!(chain[1].from, names(&["Root", "A", "A2"]));
    assert_eq!(chain[1].to, names(&["Root", "B"]));
}

#[test]
fn a_transition_settled_right_after_the_start_is_a_chain() {
    let mut world = traced(true, false);
    let e = world.spawn();
    world.insert(e, Mind::<Tm>::starting_in(Tm::B)).unwrap();
    world
        .insert(
            e,
            Log {
                bounce: true,
                ..Log::default()
            },
        )
        .unwrap();
    ai_tick::<Tm>(&mut world, &ctx(0));
    let lines = transitions(take(&mut world));
    assert_eq!(lines[0].cause, AiCause::Start);
    assert!(lines.len() >= 2);
    assert!(lines[1..].iter().all(|t| t.cause == AiCause::Chain));
}

#[test]
fn restart_records_from_equal_to_and_a_go_to_an_active_state_records_nothing() {
    let mut world = traced(true, false);
    let e = spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    take(&mut world);

    world.component_mut::<Log>(e).unwrap().next = Some(102); // Restart(A1)
    ai_tick::<Tm>(&mut world, &ctx(1));
    let restart = transitions(take(&mut world));
    assert_eq!(restart.len(), 1);
    assert_eq!(restart[0].from, restart[0].to);

    world.component_mut::<Log>(e).unwrap().next = Some(1); // Go(A), already active
    ai_tick::<Tm>(&mut world, &ctx(2));
    assert!(take(&mut world).is_empty());
}

#[test]
fn a_capped_chain_records_only_the_transitions_it_made() {
    let mut world = traced(true, false);
    let e = world.spawn();
    world.insert(e, Mind::<Tm>::starting_in(Tm::B)).unwrap();
    world
        .insert(
            e,
            Log {
                bounce: true,
                ..Log::default()
            },
        )
        .unwrap();
    ai_tick::<Tm>(&mut world, &ctx(0));
    let lines = transitions(take(&mut world));
    // The start, then exactly the capped number of chained transitions: the refused one is silent.
    assert_eq!(
        lines.len(),
        1 + ludion_ai::MAX_TRANSITIONS_PER_TICK as usize
    );
    let faults = world.resource_mut::<AiFaults>().unwrap().take();
    assert!(
        faults
            .iter()
            .any(|f| f.code == code::AI_TRANSITION_CHAIN_CAPPED)
    );
}

#[test]
fn a_restored_mind_records_no_start() {
    let mut world = traced(true, false);
    let mind = Mind::<Tm> {
        state: Tm::A1,
        entered: vec![0, 0, 0],
        ..Mind::default()
    };
    spawn(&mut world, mind);
    ai_tick::<Tm>(&mut world, &ctx(5));
    assert!(take(&mut world).is_empty());
}

#[test]
fn a_held_clock_records_nothing() {
    let mut world = traced(true, true);
    spawn(&mut world, Mind::<Tm>::default());
    let held = held_ctx();
    ai_tick::<Tm>(&mut world, &held);
    assert!(take(&mut world).is_empty());
}

#[test]
fn ai_states_gives_one_line_per_agent_per_tick_ascending() {
    let mut world = traced(false, true);
    let a = spawn(&mut world, Mind::<Tm>::default());
    let b = spawn(&mut world, Mind::<Tm>::default());
    for frame in 0..3 {
        ai_tick::<Tm>(&mut world, &ctx(frame));
    }
    let lines = state_lines(take(&mut world));
    let keys: Vec<(u64, EntityId)> = lines.iter().map(|l| (l.tick, l.entity)).collect();
    assert_eq!(keys, [(0, a), (0, b), (1, a), (1, b), (2, a), (2, b)]);
    assert!(lines.iter().all(|l| l.path == names(&["Root", "A", "A1"])));
}

#[test]
fn transitions_false_records_no_transitions() {
    let mut world = traced(false, true);
    spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    let records = take(&mut world);
    assert!(transitions(records.clone()).is_empty());
    assert_eq!(state_lines(records).len(), 1);
}

#[test]
fn tracing_moves_nothing() {
    let run = |trace: bool| {
        let mut world = World::new();
        if trace {
            world.insert_resource(AiTrace::new(true, true));
        }
        let e = spawn(&mut world, Mind::<Tm>::default());
        ai_tick::<Tm>(&mut world, &ctx(0));
        assert!(send::<Tm>(&mut world, e, Ev::Ping(1)));
        world.component_mut::<Log>(e).unwrap().next = Some(4);
        for frame in 1..4 {
            ai_tick::<Tm>(&mut world, &ctx(frame));
        }
        (
            world.component::<Mind<Tm>>(e).unwrap().clone().state,
            world.component::<Mind<Tm>>(e).unwrap().entered.clone(),
            world.component::<Log>(e).unwrap().clone(),
        )
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn the_roster_lists_the_hierarchy_agents_and_game_tick_without_duplicating() {
    let mut world = World::new();
    assert!(world.resource::<AiRoster>().is_none());
    let a = spawn(&mut world, Mind::<Tm>::default());
    let b = spawn(&mut world, Mind::<Tm>::default());
    ai_tick::<Tm>(&mut world, &ctx(0));
    ai_tick::<Tm>(&mut world, &ctx(7));
    let roster = world.resource::<AiRoster>().unwrap();
    assert_eq!(roster.machines.len(), 1, "an upsert, not an append");
    let machine = &roster.machines[0];
    assert_eq!(machine.machine, "Tm");
    assert_eq!(machine.component, "TmMind");
    assert_eq!(machine.root, "Root");
    assert!(!machine.leaf_first);
    assert_eq!(machine.inbox_capacity, 16);
    assert_eq!(machine.game_tick, 7);
    let a1 = machine.states.iter().find(|s| s.state == "A1").unwrap();
    assert_eq!(a1.parent.as_deref(), Some("A"));
    assert_eq!(a1.initial, None);
    let root = &machine.states[0];
    assert_eq!(
        (root.parent.clone(), root.initial.as_deref()),
        (None, Some("A"))
    );
    let agents = machine.agents(&world);
    assert_eq!(agents.iter().map(|g| g.entity).collect::<Vec<_>>(), [a, b]);
    assert_eq!(agents[0].path, names(&["Root", "A", "A1"]));
    assert_eq!(agents[0].entered, [0, 0, 0]);
    assert_eq!((agents[0].inbox, agents[0].dropped), (0, 0));
}

#[test]
fn the_roster_lists_a_machine_even_on_a_held_tick() {
    let mut world = World::new();
    spawn(&mut world, Mind::<Tm>::default());
    let held = held_ctx();
    ai_tick::<Tm>(&mut world, &held);
    assert_eq!(world.resource::<AiRoster>().unwrap().machines.len(), 1);
}

#[test]
fn a_serde_renamed_machine_traces_its_snake_case_names() {
    let mut world = traced(true, true);
    spawn(&mut world, Mind::<Sn>::default());
    ai_tick::<Sn>(&mut world, &ctx(0));
    let records = take(&mut world);
    assert_eq!(
        transitions(records.clone())[0].to,
        names(&["root", "look_around"])
    );
    assert_eq!(
        state_lines(records)[0].path,
        names(&["root", "look_around"])
    );
    let roster = world.resource::<AiRoster>().unwrap();
    assert_eq!(roster.machines[0].states[1].state, "look_around");
}

#[test]
fn a_send_fault_does_not_name_the_target() {
    let mut world = World::new();
    let ghost = world.spawn();
    assert!(!send::<Tm>(&mut world, ghost, Ev::ToB));
    let faults = world.resource_mut::<AiFaults>().unwrap().take();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].code, code::AI_SEND_NO_MIND);
    assert_eq!(faults[0].entity, ghost);
    assert!(
        !faults[0].message.contains(&ghost.to_string()),
        "the drain prefixes the entity: {}",
        faults[0].message
    );
}
