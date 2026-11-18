//! The dispatch contract of `ai_tick`, on a bare `World`.

use ludion_ai::{
    AiFaults, Cx, Group, GroupMember, Machine, Mind, Step, active_path, ai_tick, members,
    register_machine, send,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::{Registry, SystemCtx, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Ev {
    ToB,
    Ping(u8),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Log {
    calls: Vec<String>,
    /// Update at the `A1` leaf: index into `ALL`; `100 + i` restarts.
    next: Option<u8>,
    /// `enter(B)` and `enter(A2)` bounce the agent when set.
    bounce: bool,
    outbox: Vec<(EntityId, u8)>,
    received: Vec<(u64, u8)>,
    time_in_root: Vec<u64>,
    group: Vec<Option<EntityId>>,
}

macro_rules! machine {
    ($name:ident, $leaf_first:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        enum $name {
            Root,
            A,
            A1,
            A2,
            B,
        }
        impl Machine for $name {
            type Blackboard = Log;
            type Event = Ev;
            const NAME: &'static str = stringify!($name);
            const ROOT: Self = $name::Root;
            const ALL: &'static [Self] = &[$name::Root, $name::A, $name::A1, $name::A2, $name::B];
            const LEAF_FIRST: bool = $leaf_first;
            fn parent(s: Self) -> Option<Self> {
                match s {
                    $name::Root => None,
                    $name::A | $name::B => Some($name::Root),
                    $name::A1 | $name::A2 => Some($name::A),
                }
            }
            fn initial(s: Self) -> Option<Self> {
                match s {
                    $name::Root => Some($name::A),
                    $name::A => Some($name::A1),
                    _ => None,
                }
            }
            fn enter(cx: &mut Cx<'_, Self>, s: Self) {
                cx.bb.calls.push(format!("enter {s:?}"));
                cx.bb.group.push(cx.group);
                if cx.bb.bounce {
                    match s {
                        $name::B => cx.go($name::A2),
                        $name::A2 => cx.go($name::B),
                        _ => {}
                    }
                }
            }
            fn exit(cx: &mut Cx<'_, Self>, s: Self) {
                cx.bb.calls.push(format!("exit {s:?}"));
            }
            fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
                cx.bb.calls.push(format!("update {s:?}"));
                if s == $name::Root {
                    cx.bb.time_in_root.push(cx.ticks_in($name::Root).unwrap());
                }
                for (to, n) in std::mem::take(&mut cx.bb.outbox) {
                    cx.send(to, Ev::Ping(n));
                }
                if s == $name::A1
                    && let Some(i) = cx.bb.next.take()
                {
                    return match i {
                        100.. => Step::Restart(Self::ALL[usize::from(i - 100)]),
                        i => Step::Go(Self::ALL[usize::from(i)]),
                    };
                }
                Step::Stay
            }
            fn on_event(cx: &mut Cx<'_, Self>, s: Self, e: &Ev) -> Step<Self> {
                cx.bb.calls.push(format!("event {s:?}"));
                match (*e, s) {
                    (Ev::ToB, $name::Root) => Step::Go($name::B),
                    (Ev::ToB, $name::A1) => Step::Go($name::A2),
                    (Ev::Ping(n), $name::Root) => {
                        cx.bb.received.push((cx.ctx.game_time.ticks(), n));
                        Step::Stay
                    }
                    _ => Step::Stay,
                }
            }
        }
    };
}
machine!(Rf, false);
machine!(Lf, true);

fn ctx(frame: u64) -> SystemCtx {
    SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT)
}

fn spawn<M: Machine>(world: &mut World, mind: Mind<M>) -> EntityId {
    let e = world.spawn();
    world.insert(e, mind).unwrap();
    e
}

fn bb(world: &World, e: EntityId) -> &Log {
    world.component::<Log>(e).unwrap()
}

fn take_calls(world: &mut World, e: EntityId) -> Vec<String> {
    std::mem::take(&mut world.component_mut::<Log>(e).unwrap().calls)
}

fn faults(world: &mut World) -> Vec<(EntityId, ludion_core::diagnostics::DiagnosticCode)> {
    world
        .resource_mut::<AiFaults>()
        .map(|f| f.take().into_iter().map(|f| (f.entity, f.code)).collect())
        .unwrap_or_default()
}

#[test]
fn start_enters_root_down_the_initial_chain() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(5));
    let calls = take_calls(&mut world, e);
    assert_eq!(
        calls,
        [
            "enter Root",
            "enter A",
            "enter A1",
            "update Root",
            "update A",
            "update A1"
        ]
    );
    assert_eq!(active_path::<Rf>(&world, e), [Rf::Root, Rf::A, Rf::A1]);
    let mind = world.component::<Mind<Rf>>(e).unwrap();
    assert_eq!((mind.state, mind.entered.clone()), (Rf::A1, vec![5, 5, 5]));
}

#[test]
fn transitions_exit_to_the_common_ancestor_and_enter_down() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    world.component_mut::<Log>(e).unwrap().next = Some(4); // A1 -> B
    ai_tick::<Rf>(&mut world, &ctx(1));
    let calls = take_calls(&mut world, e);
    assert_eq!(
        calls,
        [
            "update Root",
            "update A",
            "update A1",
            "exit A1",
            "exit A",
            "enter B"
        ]
    );
    world.component_mut::<Log>(e).unwrap().next = None;
    // B -> A2: only B is left, A and A2 are entered.
    world
        .component_mut::<Mind<Rf>>(e)
        .unwrap()
        .inbox
        .push(Ev::ToB);
    ai_tick::<Rf>(&mut world, &ctx(2));
    assert_eq!(active_path::<Rf>(&world, e), [Rf::Root, Rf::B]);
}

#[test]
fn siblings_share_their_parent() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    world.component_mut::<Log>(e).unwrap().next = Some(3); // A1 -> A2
    ai_tick::<Rf>(&mut world, &ctx(1));
    let calls = take_calls(&mut world, e);
    assert_eq!(&calls[3..], ["exit A1", "enter A2"]);
    let mind = world.component::<Mind<Rf>>(e).unwrap();
    assert_eq!(mind.entered, [0, 0, 1], "A kept its entry tick, A2 is new");
}

#[test]
fn events_run_before_update_and_root_first_picks_the_root() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    assert!(send::<Rf>(&mut world, e, Ev::ToB));
    ai_tick::<Rf>(&mut world, &ctx(1));
    let calls = take_calls(&mut world, e);
    assert_eq!(calls[0], "event Root", "root first: Root answers ToB");
    assert_eq!(&calls[1..4], ["exit A1", "exit A", "enter B"]);
    assert!(calls[4..].iter().all(|c| c.starts_with("update")));
    assert_eq!(active_path::<Rf>(&world, e), [Rf::Root, Rf::B]);
}

#[test]
fn leaf_first_picks_the_leaf() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Lf>::default());
    ai_tick::<Lf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    send::<Lf>(&mut world, e, Ev::ToB);
    ai_tick::<Lf>(&mut world, &ctx(1));
    let calls = take_calls(&mut world, e);
    assert_eq!(
        calls[0], "event A1",
        "leaf first: A1 answers before Root is asked"
    );
    assert_eq!(active_path::<Lf>(&world, e), [Lf::Root, Lf::A, Lf::A2]);
}

#[test]
fn go_to_an_active_state_is_a_no_op() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    for target in [1u8, 2, 0] {
        world.component_mut::<Log>(e).unwrap().next = Some(target);
        ai_tick::<Rf>(&mut world, &ctx(1));
        let calls = take_calls(&mut world, e);
        assert!(calls.iter().all(|c| c.starts_with("update")), "{calls:?}");
    }
    assert_eq!(world.component::<Mind<Rf>>(e).unwrap().entered, [0, 0, 0]);
}

#[test]
fn restart_exits_through_the_state_and_reenters_it() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    take_calls(&mut world, e);
    world.component_mut::<Log>(e).unwrap().next = Some(101); // Restart(A)
    ai_tick::<Rf>(&mut world, &ctx(4));
    let calls = take_calls(&mut world, e);
    assert_eq!(&calls[3..], ["exit A1", "exit A", "enter A", "enter A1"]);
    assert_eq!(world.component::<Mind<Rf>>(e).unwrap().entered, [0, 4, 4]);
    // Restart of an inactive state is a plain Go.
    world.component_mut::<Log>(e).unwrap().next = Some(104); // Restart(B)
    ai_tick::<Rf>(&mut world, &ctx(5));
    assert_eq!(active_path::<Rf>(&world, e), [Rf::Root, Rf::B]);
}

#[test]
fn authored_state_enters_from_root_and_follows_initial() {
    let mut world = World::new();
    let a = spawn(&mut world, Mind::starting_in(Rf::A));
    let a2 = spawn(&mut world, Mind::starting_in(Rf::A2));
    ai_tick::<Rf>(&mut world, &ctx(0));
    assert_eq!(
        take_calls(&mut world, a)[..3],
        ["enter Root", "enter A", "enter A1"]
    );
    assert_eq!(
        take_calls(&mut world, a2)[..3],
        ["enter Root", "enter A", "enter A2"]
    );
    assert_eq!(active_path::<Rf>(&world, a2), [Rf::Root, Rf::A, Rf::A2]);
}

#[test]
fn a_bouncing_chain_is_capped_at_eight_transitions() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    world
        .insert(
            e,
            Log {
                bounce: true,
                next: Some(4),
                ..Log::default()
            },
        )
        .unwrap();
    ai_tick::<Rf>(&mut world, &ctx(0));
    let calls = take_calls(&mut world, e);
    let transitions = calls
        .iter()
        .filter(|c| matches!(c.as_str(), "enter B" | "enter A2"))
        .count();
    assert_eq!(transitions, 8, "{calls:?}");
    assert_eq!(faults(&mut world), [(e, code::AI_TRANSITION_CHAIN_CAPPED)]);
    // The agent held where the cap stopped it, and the next tick is quiet.
    assert_eq!(active_path::<Rf>(&world, e), [Rf::Root, Rf::A, Rf::A2]);
    ai_tick::<Rf>(&mut world, &ctx(1));
    assert!(faults(&mut world).is_empty());
}

#[test]
fn unlisted_authored_state_warns_and_starts_from_root() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    enum Partial {
        Root,
        Ghost,
    }
    impl Machine for Partial {
        type Blackboard = ();
        type Event = ();
        const NAME: &'static str = "Partial";
        const ROOT: Self = Partial::Root;
        const ALL: &'static [Self] = &[Partial::Root];
        fn parent(s: Self) -> Option<Self> {
            (s != Partial::Root).then_some(Partial::Root)
        }
    }
    let mut world = World::new();
    let e = spawn(&mut world, Mind::starting_in(Partial::Ghost));
    ai_tick::<Partial>(&mut world, &ctx(0));
    assert_eq!(faults(&mut world), [(e, code::AI_STATE_NOT_LISTED)]);
    assert_eq!(active_path::<Partial>(&world, e), [Partial::Root]);
}

#[test]
fn sends_to_a_processed_agent_and_to_self_arrive_next_tick_in_order() {
    let mut world = World::new();
    let a = spawn(&mut world, Mind::<Rf>::default());
    let b = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0)); // start both
    world.component_mut::<Log>(a).unwrap().outbox = vec![(b, 1)];
    world.component_mut::<Log>(b).unwrap().outbox = vec![(a, 2), (b, 3), (b, 4)];
    ai_tick::<Rf>(&mut world, &ctx(1));
    ai_tick::<Rf>(&mut world, &ctx(2));
    // a ran first: its send to b is seen in tick 1; b's send to a (already run) waits for tick 2.
    assert_eq!(bb(&world, b).received, [(1, 1), (2, 3), (2, 4)]);
    assert_eq!(bb(&world, a).received, [(2, 2)]);
}

#[test]
fn a_full_inbox_drops_the_oldest_and_faults() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    for n in 0..18 {
        assert!(send::<Rf>(&mut world, e, Ev::Ping(n)));
    }
    let mind = world.component::<Mind<Rf>>(e).unwrap();
    assert_eq!((mind.inbox.len(), mind.dropped), (16, 2));
    assert_eq!(mind.inbox[0], Ev::Ping(2));
    assert_eq!(faults(&mut world), [(e, code::AI_INBOX_OVERFLOW)]);
}

#[test]
fn a_self_send_overflow_is_counted_too() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    world.component_mut::<Log>(e).unwrap().outbox = (0..17).map(|n| (e, n)).collect();
    ai_tick::<Rf>(&mut world, &ctx(1));
    assert_eq!(world.component::<Mind<Rf>>(e).unwrap().dropped, 1);
    assert_eq!(faults(&mut world), [(e, code::AI_INBOX_OVERFLOW)]);
}

#[test]
fn a_send_to_a_mindless_entity_faults() {
    let mut world = World::new();
    let nobody = world.spawn();
    assert!(!send::<Rf>(&mut world, nobody, Ev::ToB));
    assert_eq!(faults(&mut world), [(nobody, code::AI_SEND_NO_MIND)]);
}

#[test]
fn a_held_tick_runs_nothing() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    let mut clock = GameClock::default();
    clock.hold();
    let advanced = clock.advance();
    let held = ctx(3).with_clock(&clock, advanced);
    assert!(!held.game_advanced());
    ai_tick::<Rf>(&mut world, &held);
    assert!(
        world.component::<Log>(e).is_none(),
        "nothing ran, nothing was stored"
    );
    assert!(world.component::<Mind<Rf>>(e).unwrap().entered.is_empty());
}

#[test]
fn members_sort_by_role_then_entity_and_a_despawned_member_leaves() {
    let mut world = World::new();
    let squad = world.spawn();
    world
        .insert(
            squad,
            Group {
                kind: "patrol".into(),
            },
        )
        .unwrap();
    let mk = |world: &mut World, role| {
        let e = world.spawn();
        world
            .insert(
                e,
                GroupMember {
                    group: Some(squad),
                    role,
                },
            )
            .unwrap();
        e
    };
    let (a, b, c) = (mk(&mut world, 1), mk(&mut world, 0), mk(&mut world, 1));
    let elsewhere = world.spawn();
    world
        .insert(
            elsewhere,
            GroupMember {
                group: None,
                role: 0,
            },
        )
        .unwrap();
    assert_eq!(members(&mut world, squad), [b, a, c]);
    world.despawn(a).unwrap();
    assert_eq!(members(&mut world, squad), [b, c]);
}

#[test]
fn broadcast_reaches_the_other_members_in_order() {
    #[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
    struct Shout {
        heard: Vec<u8>,
        fire: bool,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    enum Sq {
        Root,
    }
    impl Machine for Sq {
        type Blackboard = Shout;
        type Event = u8;
        const NAME: &'static str = "Sq";
        const ROOT: Self = Sq::Root;
        const ALL: &'static [Self] = &[Sq::Root];
        fn parent(_: Self) -> Option<Self> {
            None
        }
        fn update(cx: &mut Cx<'_, Self>, _: Self) -> Step<Self> {
            if std::mem::take(&mut cx.bb.fire)
                && let Some(g) = cx.group
            {
                cx.broadcast(g, &9);
            }
            Step::Stay
        }
        fn on_event(cx: &mut Cx<'_, Self>, _: Self, e: &u8) -> Step<Self> {
            cx.bb.heard.push(*e);
            Step::Stay
        }
    }
    let mut world = World::new();
    let squad = world.spawn();
    world.insert(squad, Group::default()).unwrap();
    let mut agents = Vec::new();
    for role in [0u8, 1, 2] {
        let e = spawn(&mut world, Mind::<Sq>::default());
        world
            .insert(
                e,
                GroupMember {
                    group: Some(squad),
                    role,
                },
            )
            .unwrap();
        agents.push(e);
    }
    ai_tick::<Sq>(&mut world, &ctx(0));
    world.component_mut::<Shout>(agents[1]).unwrap().fire = true;
    ai_tick::<Sq>(&mut world, &ctx(1)); // leader ran before the shouter: hears next tick
    ai_tick::<Sq>(&mut world, &ctx(2));
    let heard: Vec<_> = agents
        .iter()
        .map(|&e| world.component::<Shout>(e).unwrap().heard.clone())
        .collect();
    assert_eq!(heard, [vec![9], vec![], vec![9]], "the sender is excluded");
}

#[test]
fn a_missing_group_faults_and_cx_group_is_none() {
    let mut world = World::new();
    let gone = world.spawn();
    world.despawn(gone).unwrap();
    let e = spawn(&mut world, Mind::<Rf>::default());
    world
        .insert(
            e,
            GroupMember {
                group: Some(gone),
                role: 0,
            },
        )
        .unwrap();
    ai_tick::<Rf>(&mut world, &ctx(0));
    assert_eq!(faults(&mut world), [(e, code::AI_GROUP_MISSING)]);
    assert!(bb(&world, e).group.iter().all(Option::is_none));
}

#[test]
fn time_in_counts_gameplay_ticks_since_entry() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    for f in 10..13 {
        ai_tick::<Rf>(&mut world, &ctx(f));
    }
    assert_eq!(bb(&world, e).time_in_root, [0, 1, 2]);
}

#[test]
fn agents_removed_mid_tick_are_skipped_and_mind_survives_a_restart_of_state() {
    let mut world = World::new();
    let e = spawn(&mut world, Mind::<Rf>::default());
    ai_tick::<Rf>(&mut world, &ctx(0));
    world.despawn(e).unwrap();
    ai_tick::<Rf>(&mut world, &ctx(1)); // nothing to run, nothing panics
}

// --- registration -------------------------------------------------------------------------

macro_rules! bad_machine {
    ($name:ident, root: $root:expr, all: $all:expr, parent: $parent:expr, initial: $initial:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        enum $name {
            R,
            X,
            Y,
        }
        impl Machine for $name {
            type Blackboard = ();
            type Event = ();
            const NAME: &'static str = stringify!($name);
            const ROOT: Self = $root;
            const ALL: &'static [Self] = $all;
            fn parent(s: Self) -> Option<Self> {
                $parent(s)
            }
            fn initial(s: Self) -> Option<Self> {
                $initial(s)
            }
        }
    };
}

bad_machine!(NoRootListed, root: NoRootListed::R, all: &[NoRootListed::X],
    parent: |_| None, initial: |_| None);
bad_machine!(Duplicate, root: Duplicate::R, all: &[Duplicate::R, Duplicate::R],
    parent: |_| None, initial: |_| None);
bad_machine!(RootHasParent, root: RootHasParent::R, all: &[RootHasParent::R, RootHasParent::X],
    parent: |_| Some(RootHasParent::X), initial: |_| None);
bad_machine!(Orphan, root: Orphan::R, all: &[Orphan::R, Orphan::X],
    parent: |_| None, initial: |_| None);
bad_machine!(Cycle, root: Cycle::R, all: &[Cycle::R, Cycle::X, Cycle::Y],
    parent: |s| match s { Cycle::R => None, Cycle::X => Some(Cycle::Y), _ => Some(Cycle::X) },
    initial: |_| None);
bad_machine!(BadInitial, root: BadInitial::R, all: &[BadInitial::R, BadInitial::X, BadInitial::Y],
    parent: |s| (s != BadInitial::R).then_some(BadInitial::R),
    initial: |s| (s == BadInitial::X).then_some(BadInitial::Y));

fn refused<M: Machine>() -> ludion_core::diagnostics::DiagnosticCode {
    register_machine::<M>(&mut Registry::new())
        .unwrap_err()
        .code
}

#[test]
fn register_machine_refuses_malformed_tables() {
    assert_eq!(refused::<NoRootListed>(), code::AI_MACHINE_VARIANTS);
    assert_eq!(refused::<Duplicate>(), code::AI_MACHINE_VARIANTS);
    assert_eq!(refused::<RootHasParent>(), code::AI_MACHINE_HIERARCHY);
    assert_eq!(refused::<Orphan>(), code::AI_MACHINE_HIERARCHY);
    assert_eq!(refused::<Cycle>(), code::AI_MACHINE_HIERARCHY);
    assert_eq!(refused::<BadInitial>(), code::AI_MACHINE_INITIAL);
}

#[test]
fn register_machine_accepts_a_good_table_and_shares_group_components() {
    let mut registry = Registry::new();
    register_machine::<Rf>(&mut registry).unwrap();
    register_machine::<Lf>(&mut registry).unwrap();
    for name in ["RfMind", "LfMind", "Group", "GroupMember"] {
        assert!(registry.get(name).is_some(), "{name}");
    }
}

#[test]
fn an_event_holding_a_marked_entity_ref_is_refused() {
    #[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
    struct Sighting {
        #[schemars(with = "ludion_ecs::EntityRefSchema")]
        who: EntityId,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    enum Watcher {
        Root,
    }
    impl Machine for Watcher {
        type Blackboard = ();
        type Event = Sighting;
        const NAME: &'static str = "Watcher";
        const ROOT: Self = Watcher::Root;
        const ALL: &'static [Self] = &[Watcher::Root];
        fn parent(_: Self) -> Option<Self> {
            None
        }
    }
    assert_eq!(refused::<Watcher>().as_str(), "L1392");
}
