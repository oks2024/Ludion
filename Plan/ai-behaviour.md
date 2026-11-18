# Plan: Agent behaviour — HFSM, timers, squads (`ludion-ai`)

> **Wave 1 shipped; Waves 2–3 not started.** Driven by Neon Shroud request 0003 (an AI behaviour
> framework). Game-facing doc: `ludion docs ai`.

## Design decisions

1. **The sanctioned Rust-side form, not a DSL.** States are a Rust enum, the hierarchy a `parent` fn,
   transitions are Rust fns returning `Step`. Rust is the only Turing-complete layer (CLAUDE.md), so
   nothing here is data-driven behaviour; the request fills the "Rust-side shape is open" slot of
   `Plan/engine-gap-map.md` (see Proposed below).
2. **One `Machine` trait over a game enum; one generic system.** `ai_tick::<M>` runs every `Mind<M>`.
   A game registers one machine per enum with `register_machine::<M>`; nothing is a builtin, so an
   absent machine leaves the component schema snapshot, the protocol schema hash, goldens, replays and
   every `WorldHash` untouched.
3. **A hierarchy with an explicit dispatch contract.** Root-first by default (`LEAF_FIRST` opts out);
   events before update; the first non-`Stay` answer wins and applies before the next event; `Go` to an
   active state is a no-op; `Restart` re-enters; transitions exit leaf-up to the common ancestor and
   enter down, then follow `initial`. Documented as numbered rules in `ludion docs ai` and tested one by one.
4. **At most 8 transitions per agent per tick** (`L4120`): a bouncing `enter` chain stops instead of
   hanging the sim.
5. **Take, don't remove.** The agent's `Mind` and blackboard are `mem::take`n out of storage for the
   callbacks and written back after: no storage churn, no ownership dance. The cost is a documented trap:
   inside a callback they read as defaults through `cx.world`. Sends to the running agent go into the
   taken mind, never the placeholder.
6. **Gameplay time.** Timers and cooldowns store absolute `ctx.game_time` ticks, so they freeze under a
   held game and survive a snapshot restore (`GameClock` rides snapshots). Seconds convert to ticks
   rounding up after absorbing `f32` representation error (`0.3 s` at 60 Hz is 18 ticks, not 19).
7. **Ref-free events, Wave 1.** The inbox is a list inside `Mind<M>`, so a marked entity ref in an event
   fails registration with `L1392`, and an unmarked one fails a game's `assert_entity_refs_marked`;
   existing gates enforce it. References travel in single marked blackboard fields.
8. **Squads are components.** `Group { kind }` and `GroupMember { group, role }` (the group field is a
   marked, optional, drift-safe ref). `members` scans and sorts by `(role, entity)`, so a despawned member
   leaves with no list to go stale. `Cx::broadcast` reaches the other members in that order.
9. **`ai_tick` returns on a held gameplay frame** (`!ctx.game_advanced()`): agents neither think nor time
   out while the game is frozen.
10. **Faults cross a seam.** `ludion-ai` cannot see `Diagnostics`; faults queue in the `AiFaults`
    resource (never hashed, never snapshotted, deduped per `(entity, code)`) and `ludion-engine` drains
    them through `report_faults`, the `ShapeRejection` pattern. Codes `L1510`–`L1512` (registration),
    `L4120`–`L4124` (runtime).
11. **Event delivery order falls out of ascending entity order.** A send to an agent not yet run this tick
    is seen this tick; to one already run, next tick; to oneself, next tick. Inbox capacity is 16 per
    machine (`INBOX_CAPACITY`); overflow drops the oldest and counts it in `Mind.dropped` (`L4121`).
12. **The trace is fed by `ai_tick` into an opt-in `AiTrace`.** The trace driver inserts the resource;
    `ai_tick` appends only when it is present, so the cost with tracing off is one resource lookup per
    call. It is never registered, hashed or snapshotted (the `AiFaults` pattern), and names come from
    serde (`#[serde(rename_all)]` machines trace the names their scenes use), not `Debug`.
13. **Cause vocabulary: `start`, `event:<Name>`, `update`, `chain`.** `<Name>` is the event's variant
    name (the request's spelling); the payload rides beside it in the `event` key. The per-tick channel
    is `ai_states`, not the request's `ai:states`: trace channels have no sub-channel syntax and an
    alias would be a second way (Pillar 6). The first
    transition of an event or update walk takes that cause; every later transition of the same settle,
    and one settled right after the start, is `chain`. A no-op `Go` and a capped transition record
    nothing. A tick the game held records nothing, so a held frame prints no ai lines.
14. **Live introspection rides `engine.describe`'s `ai` key**, present only when an `AiRoster` exists
    (a machine has ticked), so a world without ai describes byte-identically. There is no `ai.describe`
    command: a new `Command` variant moves `replay_schema_hash` and would refuse every committed `.lrp`
    (`L3010`), a corpus re-record.
15. **The squad scene is an engine test fixture, not a gallery scene.** The gallery is a builtin-only
    sweep (`ludion test` and `committed_scene_spelling.rs` load every `examples/gallery/*.yaml` with a
    plain `Engine::builder()`), so a scene bearing `GuardMind` fails there and a new scene owes pinned
    goldens. It lives at `crates/ludion-engine/tests/fixtures/ai-squad.yaml` beside its committed
    `ai-squad.trace.txt` (per-tick `WorldHash`) and `ai-squad.ai.jsonl` (the `ai` channel).

## Waves

- **W1 — shipped.** `ludion-ai` (`Machine`, `Step`, `Mind`, `Cx`, `ai_tick`, `register_machine`, `send`,
  `members`, `Timer`, `Cooldown`, `Group`, `GroupMember`, `AiFaults`), the engine fault drain, the
  `ludion::ai` facade module, `docs/ai.md`, `crates/ludion/examples/ai_squad.rs`.
- **W2 — shipped** (decisions 12–15). The `ai` and `ai_states` trace channels
  (`crates/ludion-ai/src/trace.rs`, `crates/ludion-engine/src/trace.rs`), the `engine.describe` `ai`
  section (`crates/ludion-engine/src/ai.rs`), the squad fixture and its committed traces, and the
  fault messages naming the entity once.
- **W3 — not started.** A `MemberLeft` event and ref-carrying events, after the entity-ref-collection
  subtask below.

## Out of scope

- **Behaviour trees.** Trigger: a game whose HFSM visibly repeats task sequencing a tree would express.
- **Utility scoring.** Trigger: a game with agents choosing among many weighted actions.
- **Parallel evaluate/apply.** Trigger: a measured `ai_tick` cost that a profile attributes to callbacks,
  at a count a single thread cannot hold.
- **Game gameplay-time scale** (request 0011). Timers follow whatever representation 0011 gives gameplay
  time; this plan couples to it by reading `ctx.game_time` and nothing else.
- Perception (request 0001) and navigation (request 0004) are separate plans.
- **Blackboard timers via `Describe`.** Trigger: a tool needs timers without reading the blackboard
  component through `entity.get`.

## Proposed — owner ruling owed

- **The sanctioned form vs `Plan/voxel-animation.md` decision 6 and `Plan/engine-gap-map.md`.** A
  data-driven state machine was rejected there as a pillar violation; this plan ships the Rust-side shape
  those documents left open. The ruling confirms that reading.
- **No behaviour trees** (above) is a position, not a measurement.
- **Inbox capacity 16** is a default chosen without a game's numbers.
- **An entity-ref collection type.** Sanctioning a list of entity refs touches the ref walker
  (`crates/ludion-ecs/src/entity_ref.rs`) and about ten consumer sites in `ludion-scene` (`expand.rs`,
  `validate.rs`, `instantiate.rs`) and `ludion-engine` (`collapse.rs`, `transition.rs`, `prefab_ops.rs`,
  `commands/entity.rs`): a cross-cutting subtask of its own, with request 0001's memory as the natural
  trigger. Not built here.
- **An `ai.describe` command.** It rides the next replay-corpus re-record, because a `Command` variant
  moves `replay_schema_hash`; until then `engine.describe`'s `ai` key serves.
- **Gallery placement of the squad scene.** A gallery scene needs a game-registered machine in the
  plain sweep, plus pinned goldens.
- **`broadcast` excludes the sender** (`Cx::broadcast`); the free `members` lists everyone. A game that
  wants the sender included sends to itself explicitly.

## Status

- **W1 shipped** (Neon Shroud request 0003): the crate and its unit tests (dispatch contract, timers to
  the exact tick, groups, registration refusals), the engine drain, `docs/ai.md`, an engine test
  (`crates/ludion-engine/tests/ai.rs`: squad on a toy machine, expected states on expected ticks,
  two-run `WorldHash` identity, snapshot/restore mid-timer). No pinned artifact moved.
- **W2 shipped** (Neon Shroud request 0003): `AiTrace`/`AiRoster` in `ludion-ai`, the `ai`/`ai_states`
  channels in `ludion trace` and a game's `--trace`, the `engine.describe` `ai` key, the squad fixture
  (`crates/ludion-engine/tests/fixtures/ai-squad.{yaml,trace.txt,ai.jsonl}`; blessed on the pinned
  Windows box, the M4 must reproduce both byte-identically), `ludion-ai` trace tests, `docs/ai.md`
  Tracing and describe. Deviations from the request: no `ai.describe` command and no gallery scene
  (decisions 14, 15; both Proposed). No pinned artifact moved.
