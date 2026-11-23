# Plan: Agent behaviour — HFSM, timers, squads (`ludion-ai`)

> **Wave 1 shipped; `ai.describe` shipped in R14; Waves 2–3 not started.** Driven by Neon Shroud request 0003 (an AI behaviour
> framework). Game-facing doc: `ludion docs ai`.

## Design decisions

1. **The sanctioned Rust-side form, not a DSL.** States are a Rust enum, the hierarchy a `parent` fn,
   transitions are Rust fns returning `Step`. Rust is the only Turing-complete layer (CLAUDE.md), so
   nothing here is data-driven behaviour; the request fills the "Rust-side shape is open" slot of
   `Plan/engine-gap-map.md` (see Owner rulings below).
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
7. **Ref-carrying events (W3).** `Mind<M>::inbox` is an `EntityRefCollection` (decision 16), so an event
   variant may carry `#[schemars(with = "Option<EntityRefSchema>")] body: Option<EntityId>`; scene load
   range-checks it and `scene.save`, the carry and the snapshot remap it per inbox element. An event ref
   that is not nullable is refused at registration with `L1513`: a target that despawned would refuse
   the save (`L1018`) or drop the whole `Mind` on a carry (`L3043`). A marked ref inside a blackboard
   list stays `L1392`. (Wave 1 shipped the events ref-free; decision 16 lifted that.)
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
    (a machine has ticked), so a world without ai describes byte-identically. **`ai.describe`** (no
    params, shipped in R14) returns the same machines typed on the wire (`ResponseBody::AiDescribe`,
    `machines: []` before any machine has ticked); the `ai` key stays, built from the same structs
    (`crates/ludion-engine/src/ai.rs::machines`). The new `Command` variant moved `replay_schema_hash`,
    which the corpus re-record absorbed.
15. **The squad scene is an engine test fixture, not a gallery scene.** The gallery is a builtin-only
    sweep (`ludion test` and `committed_scene_spelling.rs` load every `examples/gallery/*.yaml` with a
    plain `Engine::builder()`), so a scene bearing `GuardMind` fails there and a new scene owes pinned
    goldens. It lives at `crates/ludion-engine/tests/fixtures/ai-squad.yaml` beside its committed
    `ai-squad.trace.txt` (per-tick `WorldHash`) and `ai-squad.ai.jsonl` (the `ai` channel).
16. **Ref collections ride `EntityRefCollection<C>`.** One proxy marks a list (`Vec<T>`) or a string-keyed
    map (`BTreeMap<String, V>`) whose elements may hold marked entity refs at any depth. The walker reports
    them as pattern fields whose suffix holds a `/*` step, and every consumer iterates the concrete sites of
    the value in hand (`ComponentInfo::entity_ref_sites`). Per-site rules match the single field: a nullable
    dangling site becomes `null`, a non-nullable one refuses the save (`L1018`) or drops the component on carry
    (`L3043`). Nullable element refs (`Option<EntityRefSchema>`) are the drift-safe form. Any other list, map or
    tuple holding a marked ref stays `L1392`; tuples are never sanctioned.
17. **`MemberLeft` is opt-in through `GroupRoster`.** A `GroupRoster` on a group entity remembers the
    membership it last saw (`members`, in `members()` order), the departures found at the last refresh
    (`left`) and the gameplay tick of that refresh (`seen`). A record on every `Group` would move the
    squad fixture's per-tick `WorldHash`, and naming who left needs refs, not the request's count. `ai_tick`
    refreshes each roster after the held-tick early return and before the agent loop: departures are the
    previous members no longer in the group (a despawn or a retarget; a `None` entry counts), and the
    `seen` stamp makes a second machine's `ai_tick` in the same tick reuse `left`. Delivery follows, in
    ascending group id: for each departure in `left` order, the game's event goes to each current member
    with a `Mind<M>`, in `members` order, so it is handled the same tick. Joins are not reported; a fresh
    roster reports nothing. A group without a roster behaves byte-identically to before.
18. **`Machine::member_left` builds the game's event.** The defaulted trait fn
    `member_left(member: Option<EntityId>) -> Option<Self::Event>` returns `None` to send nothing. A
    `None` member is a departure seen only after a save, carry or restore nulled the dead entity.

## Waves

- **W1 — shipped.** `ludion-ai` (`Machine`, `Step`, `Mind`, `Cx`, `ai_tick`, `register_machine`, `send`,
  `members`, `Timer`, `Cooldown`, `Group`, `GroupMember`, `AiFaults`), the engine fault drain, the
  `ludion::ai` facade module, `docs/ai.md`, `crates/ludion/examples/ai_squad.rs`.
- **W2 — shipped** (decisions 12–15). The `ai` and `ai_states` trace channels
  (`crates/ludion-ai/src/trace.rs`, `crates/ludion-engine/src/trace.rs`), the `engine.describe` `ai`
  section (`crates/ludion-engine/src/ai.rs`), the squad fixture and its committed traces, and the
  fault messages naming the entity once.
- **W3 — shipped** (decisions 7, 17, 18). Ref-carrying events with `L1513`, `GroupRoster` and
  `Machine::member_left`.

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

## Owner rulings (2026-11-19)

- Accepted. **The sanctioned form vs `Plan/voxel-animation.md` decision 6 and `Plan/engine-gap-map.md`.** A
  data-driven state machine was rejected there as a pillar violation; this plan ships the Rust-side shape
  those documents left open. The ruling confirms that reading.
- Accepted. **No behaviour trees** (above) is a position, not a measurement.
- Accepted. **Inbox capacity 16** is a default chosen without a game's numbers.
- Accepted, built (decision 16). **An entity-ref collection type.** Sanctioning a list of entity refs touches the ref walker
  (`crates/ludion-ecs/src/entity_ref.rs`) and about ten consumer sites in `ludion-scene` (`expand.rs`,
  `validate.rs`, `instantiate.rs`) and `ludion-engine` (`collapse.rs`, `transition.rs`, `prefab_ops.rs`,
  `commands/entity.rs`): a cross-cutting subtask of its own, with request 0001's memory as the natural
  trigger. Not built here.
- Accepted, shipped in R14 (T10-C2a). **An `ai.describe` command.** It moved `replay_schema_hash`, which the
  R14 corpus re-record absorbs; `engine.describe`'s `ai` key stays beside it.
- Proposed — owner ruling owed: `MemberLeft` via an opt-in `GroupRoster` that names the member, instead of
  the request's previous-membership count on `Group`.
- Accepted. **Gallery placement of the squad scene.** A gallery scene needs a game-registered machine in the
  plain sweep, plus pinned goldens.
- Accepted. **`broadcast` excludes the sender** (`Cx::broadcast`); the free `members` lists everyone. A game that
  wants the sender included sends to itself explicitly.

## Status

- **W1 shipped** (Neon Shroud request 0003): the crate and its unit tests (dispatch contract, timers to
  the exact tick, groups, registration refusals), the engine drain, `docs/ai.md`, an engine test
  (`crates/ludion-engine/tests/ai.rs`: squad on a toy machine, expected states on expected ticks,
  two-run `WorldHash` identity, snapshot/restore mid-timer). No pinned artifact moved.
- **Entity-ref collection shipped** (decision 16; Neon Shroud request 0003): `EntityRefCollection<C>`, pattern
  fields with `/*`, `ComponentInfo::entity_ref_sites` in the scene range check, save remap, carry remap, prefab
  ops and snapshot. No builtin adopts it yet; no pinned artifact moved.
- **W2 shipped** (Neon Shroud request 0003): `AiTrace`/`AiRoster` in `ludion-ai`, the `ai`/`ai_states`
  channels in `ludion trace` and a game's `--trace`, the `engine.describe` `ai` key, the squad fixture
  (`crates/ludion-engine/tests/fixtures/ai-squad.{yaml,trace.txt,ai.jsonl}`; blessed on the pinned
  Windows box, the M4 must reproduce both byte-identically), `ludion-ai` trace tests, `docs/ai.md`
  Tracing and describe. Deviations from the request: no `ai.describe` command and no gallery scene
  (decisions 14, 15; both ruled by the owner 2026-11-19). No pinned artifact moved.
- **W3 shipped** (Neon Shroud request 0003): `Mind<M>::inbox` adopts `EntityRefCollection`; `L1513` for a
  non-nullable event ref; `GroupRoster` with the refresh and delivery in `ai_tick`; `Machine::member_left`;
  tests in `crates/ludion-ai/tests/machine.rs` and `crates/ludion-engine/tests/ai.rs` (save/reload,
  snapshot/restore); `docs/ai.md`; the `ai_squad` example. Deviation from the request: a roster naming the
  member, opt-in, instead of a count on `Group` (decision 17). The ai-squad fixture and every pinned
  artifact are unmoved. A roster stamped at tick 0 is not refreshed until tick 1 (`seen` defaults to 0).
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): the ai-squad fixture matches its committed `WorldHash` trace on the M4.
