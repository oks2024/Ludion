---
description: Agent behaviour in Rust: a hierarchical state machine over your enum, timers, squads, events
---

# AI behaviour (`ludion::ai`)

**Scope:** giving agents (guards, drones, bosses) states, transitions, timers and a squad to talk to.
States are a Rust enum and every callback is Rust; there is no behaviour DSL. For the clock timers
read see `ludion docs game-clock`; for registering components, `ludion docs components`.

## The loop

1. Write an enum that `impl Machine` (the hierarchy is its `parent` fn, the callbacks are plain fns).
2. Register it once: `EngineBuilder::components(|r| ludion::ai::register_machine::<Guard>(r))`, plus
   your blackboard type if it is not `()`. This registers `GuardMind`, `Group` and `GroupMember`.
3. Schedule `ai_tick::<Guard>` once: `.system("guard_ai", ludion::ai::ai_tick::<Guard>)`, after the
   systems that `send` events and before the ones that read the state.
4. Give entities `GuardMind: {}` (starts at `ROOT`) or `GuardMind: { state: Alert }`.

Registration refuses a malformed machine: `L1512` (`ALL` empty, no `ROOT`, repeats), `L1510` (a
parent chain that misses `ROOT`), `L1511` (`initial(s)` not a child of `s`).

## Format

| Piece | Notes |
| --- | --- |
| `Mind<M>` (`{NAME}Mind`) | `state` (authored start, any level; at runtime the active leaf); `entered` ticks root→leaf (runtime); `inbox`; `dropped` |
| `Machine::Blackboard` | per-agent component, `Default`; `()` stores nothing |
| `Machine::Event` | what `send` carries; keep it free of entity refs (see Rules) |
| `Step` | `Stay`, `Go(s)`, `Restart(s)` |
| `Timer { deadline }` | one-shot, gameplay ticks; `start(ctx, secs)`, `expired`, `just_expired`, `remaining` |
| `Cooldown { ready_at }` | reusable gate; `trigger(ctx, secs)`, `ready` |
| `Group { kind }`, `GroupMember { group, role }` | squads; `members(world, group)` sorts by `(role, entity)` |

Consts: `LEAF_FIRST` (default root first), `INBOX_CAPACITY` (16).

## The dispatch contract

`ai_tick` returns at once on a tick the game held. Per agent, in ascending entity order:

1. **Start.** No `entered` stamp: enter `ROOT` down to the authored `state`, then follow `initial`. A
   `state` outside `ALL` is `L4124` and starts at `ROOT`.
2. **Events.** The inbox is taken; each event, in arrival order, is offered to the active path (root
   first, or leaf first with `LEAF_FIRST`) through `on_event`. The first non-`Stay` applies before the
   next event.
3. **Update.** The same walk with `update`.
4. **Transition.** `Go(t)` on an already-active `t` does nothing. Otherwise exit leaf-up to the common
   ancestor, enter down to `t`, follow `initial`. `Restart(t)` exits through `t` and re-enters it
   (a plain `Go` when `t` is inactive). `cx.go(t)` in `enter`/`exit` chains after the transition. More
   than 8 transitions in a tick is `L4120`: the agent holds its state until the next tick.
5. **Sends.** To yourself: next tick. To an agent not yet run this tick: this tick; already run: next
   tick. `broadcast` reaches the other members, in `(role, entity)` order; the free `send::<M>` is for
   systems such as perception. A full inbox drops the oldest (`L4121`); a target with no mind is
   `L4123`; a dead or group-less `GroupMember.group` is `L4122`.

Agents spawned during a tick start on the next one.

## Rules

- **Time is gameplay time.** `Timer`, `Cooldown`, `Cx::time_in` read `ctx.game_time`; they freeze
  while the game holds, and the absolute deadlines survive a snapshot restore.
- **Events are ref-free.** An `EntityRefSchema` field inside the event type is refused at registration
  (`L1392`, the inbox is a list). Keep the reference in one marked blackboard field and send a tag.
- **The take-out trap.** Inside a callback the agent's own `Mind` and blackboard are out of the world
  (they read as defaults through `cx.world`): use `cx.bb`, `cx.send`, `cx.go`. Reading *other* agents
  through `cx.world` is fine.
- Everything sim-side is deterministic: no wall clock, no `HashMap`, no randomness except the world RNG.

## Example

```rust
use ludion::ai::{Cx, Machine, Step, Timer};
use ludion::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
enum Guard { Root, Patrol, Investigate }

#[derive(Default, Serialize, Deserialize, JsonSchema)]
struct Memory { timer: Timer }

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
enum Heard { Noise }

impl Machine for Guard {
    type Blackboard = Memory;
    type Event = Heard;
    const NAME: &'static str = "Guard";
    const ROOT: Self = Guard::Root;
    const ALL: &'static [Self] = &[Guard::Root, Guard::Patrol, Guard::Investigate];
    fn parent(s: Self) -> Option<Self> { (s != Guard::Root).then_some(Guard::Root) }
    fn initial(s: Self) -> Option<Self> { (s == Guard::Root).then_some(Guard::Patrol) }
    fn enter(cx: &mut Cx<'_, Self>, s: Self) {
        if s == Guard::Investigate {
            cx.bb.timer.start(cx.ctx, 2.0);
            if let Some(squad) = cx.group { cx.broadcast(squad, &Heard::Noise); }
        }
    }
    fn on_event(_: &mut Cx<'_, Self>, s: Self, _: &Heard) -> Step<Self> {
        if s == Guard::Patrol { Step::Go(Guard::Investigate) } else { Step::Stay }
    }
    fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
        if s == Guard::Investigate && cx.bb.timer.expired(cx.ctx) { Step::Go(Guard::Patrol) } else { Step::Stay }
    }
}
// builder: .components(|r| { ludion::ai::register_machine::<Guard>(r)?; r.register::<Memory>("Memory") })
//          .system("guard_ai", ludion::ai::ai_tick::<Guard>)
```

## Gotchas

- Register the blackboard yourself; `register_machine` registers only the mind and the squad
  components. An unregistered blackboard works but is not hashed or snapshotted.
- A faulting agent keeps running; faults arrive as diagnostics (`L4120`–`L4124`), once per entity.

## Next

`ludion docs game-clock`, `ludion docs components`, `ludion explain L4120`.
