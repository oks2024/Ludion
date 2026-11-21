//! [`ai_tick`]: the one generic system that runs every agent of a machine.

use std::any::TypeId;

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};

use crate::faults::fault;
use crate::group::{self, Group, GroupMember};
use crate::machine::{Cx, Machine, Step};
use crate::mind::{self, Mind, active_path, overflow_fault, path_to};
use crate::trace::{AiCause, AiRecord, AiState, AiTrace, AiTransition, path_names, roster_upsert};

/// The most transitions one agent makes in one tick (its start excluded). A machine that flips
/// between states on entry stops here and raises `L4120`.
pub const MAX_TRANSITIONS_PER_TICK: u32 = 8;

/// Runs every `Mind<M>` on each tick gameplay time moves (under a scale, integrate `ctx.game_dt`). Register it once per machine:
/// `schedule.add_system("guard_ai", |w, ctx| ai_tick::<Guard>(w, ctx))`, after the systems that
/// send events and before the ones that read the state.
///
/// Returns at once on a tick the game held, so a frozen game's agents neither think nor time out.
/// The dispatch contract, per agent in ascending entity order:
///
/// 1. **Start** — a mind with no `entered` stamp enters from `ROOT` down to its authored `state`,
///    then follows `initial`. A `state` outside `ALL` raises `L4124` and starts from `ROOT`.
/// 2. **Events** — the inbox is taken and each event is offered to the active path, root first
///    (leaf first with `LEAF_FIRST`) through `on_event`. The first non-`Stay` answer applies before
///    the next event.
/// 3. **Update** — the same walk with `update`; the first non-`Stay` answer applies.
/// 4. **Transition** — `Go(t)` with `t` already active does nothing; otherwise the machine exits
///    leaf-up to the common ancestor, enters down to `t`, then follows `initial`. `Restart(t)` exits
///    through `t` and enters it again. A [`Cx::go`] left pending chains after the transition. More
///    than [`MAX_TRANSITIONS_PER_TICK`] raises `L4120` and the agent holds its state for the tick.
/// 5. **Sends** — a send to the running agent lands in its own inbox for the next tick; a send to an
///    agent that has not run yet this tick is seen this tick, one that has run, the next.
///
/// Agents spawned during the tick start on the next one. Inside a callback the agent's own `Mind`
/// and blackboard are out of the world: use [`Cx::bb`](crate::Cx::bb).
pub fn ai_tick<M: Machine>(world: &mut World, ctx: &SystemCtx) {
    roster_upsert::<M>(world, ctx);
    if !ctx.game_advanced() {
        return;
    }
    deliver_member_left::<M>(world, ctx.game_time.ticks());
    let (transitions, states) = world
        .resource::<AiTrace>()
        .map_or((false, false), |t| (t.transitions, t.states));
    let agents: Vec<EntityId> = world.query_ids::<(&Mind<M>,)>().map(|(id, _)| id).collect();
    for agent in agents {
        run_agent::<M>(world, ctx, agent, transitions);
        if states {
            record_state::<M>(world, ctx, agent);
        }
    }
}

/// Refreshes the rosters, then sends each group's departures, in departure order, to every remaining
/// member that runs machine `M`.
fn deliver_member_left<M: Machine>(world: &mut World, now: u64) {
    group::refresh_rosters(world, now);
    for (remaining, left) in group::departures(world, now) {
        for departed in left {
            let Some(event) = M::member_left(departed) else {
                continue;
            };
            for &member in &remaining {
                if world.component::<Mind<M>>(member).is_some() {
                    mind::send::<M>(world, member, event.clone());
                }
            }
        }
    }
}

fn record_state<M: Machine>(world: &mut World, ctx: &SystemCtx, agent: EntityId) {
    if !world.is_alive(agent) || world.component::<Mind<M>>(agent).is_none() {
        return;
    }
    let path = path_names(active_path::<M>(world, agent));
    if let Some(trace) = world.resource_mut::<AiTrace>() {
        trace.push(AiRecord::State(AiState {
            tick: ctx.frame.as_u64(),
            entity: agent,
            machine: M::NAME,
            path,
        }));
    }
}

fn record_transition<M: Machine>(cx: &mut Cx<'_, M>, from: Vec<String>, cause: AiCause) {
    let record = AiRecord::Transition(AiTransition {
        tick: cx.ctx.frame.as_u64(),
        entity: cx.agent,
        machine: M::NAME,
        from,
        to: path_names(cx.active.iter().map(|(s, _)| *s)),
        cause,
    });
    if let Some(trace) = cx.world.resource_mut::<AiTrace>() {
        trace.push(record);
    }
}

fn has_blackboard<M: Machine>() -> bool {
    TypeId::of::<M::Blackboard>() != TypeId::of::<()>()
}

fn run_agent<M: Machine>(world: &mut World, ctx: &SystemCtx, agent: EntityId, tracing: bool) {
    if !world.is_alive(agent) {
        return;
    }
    let Some(mut mind) = world.component_mut::<Mind<M>>(agent).map(std::mem::take) else {
        return;
    };
    let mut bb = M::Blackboard::default();
    if has_blackboard::<M>() {
        if world.component::<M::Blackboard>(agent).is_none() {
            let _ = world.insert(agent, M::Blackboard::default());
        }
        if let Some(stored) = world.component_mut::<M::Blackboard>(agent) {
            bb = std::mem::take(stored);
        }
    }
    let group = resolve_group(world, agent, M::NAME);

    let mut cx = Cx::<M> {
        world,
        ctx,
        agent,
        bb: &mut bb,
        group,
        active: Vec::new(),
        pending: None,
        own_inbox: Vec::new(),
        transitions: 0,
        capped: false,
        tracing,
        cause: None,
    };
    drive(&mut cx, &mut mind);

    let Cx {
        active, own_inbox, ..
    } = cx;
    mind.state = active.last().map_or(mind.state, |(s, _)| *s);
    mind.entered = active.iter().map(|(_, tick)| *tick).collect();
    let mut overflowed = false;
    for event in own_inbox {
        overflowed |= mind.push(event);
    }
    if has_blackboard::<M>()
        && let Some(stored) = world.component_mut::<M::Blackboard>(agent)
    {
        *stored = bb;
    }
    if let Some(slot) = world.component_mut::<Mind<M>>(agent) {
        *slot = mind;
        if overflowed {
            overflow_fault::<M>(world, agent);
        }
    }
}

fn resolve_group(world: &mut World, agent: EntityId, name: &str) -> Option<EntityId> {
    let group = world.component::<GroupMember>(agent)?.group?;
    if world.is_alive(group) && world.has::<Group>(group) {
        return Some(group);
    }
    fault(
        world,
        agent,
        code::AI_GROUP_MISSING,
        format!(
            "its `{name}Mind` is a member of {group}, which is gone or has no `Group`; \
             it runs without a squad"
        ),
    );
    None
}

fn drive<M: Machine>(cx: &mut Cx<'_, M>, mind: &mut Mind<M>) {
    start(cx, mind);
    settle(cx);

    let events = std::mem::take(&mut mind.inbox);
    for event in &events {
        if !cx.world.is_alive(cx.agent) {
            return;
        }
        let step = walk(cx, |cx, s| M::on_event(cx, s, event));
        if cx.tracing {
            cx.cause = Some(AiCause::Event(
                serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
            ));
        }
        apply(cx, step);
    }
    if !cx.world.is_alive(cx.agent) {
        return;
    }
    let step = walk(cx, |cx, s| M::update(cx, s));
    if cx.tracing {
        cx.cause = Some(AiCause::Update);
    }
    apply(cx, step);
}

/// Rebuilds the active stack from the mind and, for an unstarted one, enters it.
fn start<M: Machine>(cx: &mut Cx<'_, M>, mind: &Mind<M>) {
    let target = if M::ALL.contains(&mind.state) {
        mind.state
    } else {
        fault(
            cx.world,
            cx.agent,
            code::AI_STATE_NOT_LISTED,
            format!(
                "its `{}Mind` starts in {:?}, which `{}::ALL` does not list; it starts from {:?}",
                M::NAME,
                mind.state,
                M::NAME,
                M::ROOT
            ),
        );
        M::ROOT
    };
    let path = path_to(target);
    if mind.entered.len() == path.len() && M::ALL.contains(&mind.state) {
        cx.active = path.into_iter().zip(mind.entered.iter().copied()).collect();
        return;
    }
    let now = cx.ctx.game_time.ticks();
    for s in path {
        cx.active.push((s, now));
        M::enter(cx, s);
    }
    follow_initial(cx);
    if cx.tracing {
        record_transition(cx, Vec::new(), AiCause::Start);
    }
}

/// Enters the `initial` chain below the active leaf.
fn follow_initial<M: Machine>(cx: &mut Cx<'_, M>) {
    let now = cx.ctx.game_time.ticks();
    for _ in 0..M::ALL.len() {
        let Some(&(leaf, _)) = cx.active.last() else {
            return;
        };
        let Some(child) = M::initial(leaf) else {
            return;
        };
        cx.active.push((child, now));
        M::enter(cx, child);
    }
}

/// Offers the active path to `f`, in dispatch order; the first non-`Stay` answer wins.
fn walk<M: Machine>(
    cx: &mut Cx<'_, M>,
    mut f: impl FnMut(&mut Cx<'_, M>, M) -> Step<M>,
) -> Step<M> {
    let mut states: Vec<M> = cx.active.iter().map(|(s, _)| *s).collect();
    if M::LEAF_FIRST {
        states.reverse();
    }
    for s in states {
        let step = f(cx, s);
        if step != Step::Stay {
            return step;
        }
    }
    Step::Stay
}

fn apply<M: Machine>(cx: &mut Cx<'_, M>, step: Step<M>) {
    match step {
        Step::Stay => {}
        Step::Go(t) => cx.pending = Some((t, false)),
        Step::Restart(t) => cx.pending = Some((t, true)),
    }
    settle(cx);
    cx.cause = None;
}

/// Applies pending requests until none is left (or the per-tick cap ends the chain).
fn settle<M: Machine>(cx: &mut Cx<'_, M>) {
    while let Some((target, restart)) = cx.pending.take() {
        transition(cx, target, restart);
    }
}

fn transition<M: Machine>(cx: &mut Cx<'_, M>, target: M, restart: bool) {
    if cx.capped {
        return;
    }
    let active: Vec<M> = cx.active.iter().map(|(s, _)| *s).collect();
    let on_path = active.iter().position(|s| *s == target);
    if on_path.is_some() && !restart {
        return;
    }
    if cx.transitions >= MAX_TRANSITIONS_PER_TICK {
        cx.capped = true;
        fault(
            cx.world,
            cx.agent,
            code::AI_TRANSITION_CHAIN_CAPPED,
            format!(
                "its `{}Mind` made more than {MAX_TRANSITIONS_PER_TICK} transitions in one tick \
                 (stopped at {:?}, refused {target:?}); its `enter`/`exit` callbacks likely send it \
                 back and forth",
                M::NAME,
                active.last()
            ),
        );
        return;
    }
    cx.transitions += 1;
    let traced = cx.tracing.then(|| {
        let cause = cx.cause.take().unwrap_or(AiCause::Chain);
        (path_names(active.iter().copied()), cause)
    });

    let target_path = path_to(target);
    let keep = on_path.unwrap_or_else(|| {
        active
            .iter()
            .zip(&target_path)
            .take_while(|(a, t)| a == t)
            .count()
    });
    while cx.active.len() > keep {
        let (s, _) = cx.active[cx.active.len() - 1];
        M::exit(cx, s);
        cx.active.pop();
    }
    let now = cx.ctx.game_time.ticks();
    for &s in &target_path[keep..] {
        cx.active.push((s, now));
        M::enter(cx, s);
    }
    follow_initial(cx);
    if let Some((from, cause)) = traced {
        record_transition(cx, from, cause);
    }
}
