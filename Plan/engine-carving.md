# Plan: Carve `Engine` by lifetime

> Status: **planned, not started.** Source: the 2026-09-25 code audit, finding J1 (the July audit's B2,
> re-measured). Current-code facts were read from the tree on 2026-09-26; file:line pointers are of that
> date, so re-grep before editing.

## Trigger

July's audit predicted that "every new capability adds a field plus a reset-vs-carry decision at three
world-reset sites". The September audit found that bug class in five separate findings: a half-installed
world after a failed rebuild, held input lost on a switch, a diagnostics latch that survived a load, a
swallowed profile warning, and keyframe bookkeeping.

The fix stage already retired the worst of it (below). Build the rest when **any one** of these happens:

- a new `Engine` field needs a reset/carry decision that `commit_world` cannot express as one `match` row;
- a rebuild bug is traced to `from_parts` disagreeing with `commit_world`;
- a sim-visible host input is found missing from replay verification (the class B3/B4 belonged to);
- `engine.rs` passes 5,000 lines.

Until then, the single `commit_world` gives the audit's main benefit.

## What exists (2026-09-26)

- **`Engine`** (`crates/ludion-engine/src/engine.rs:152-379`) has **50 fields**; the file is 4,125 lines.
  `SessionState` (`:403-461`, with `reset`/`carry`/`adopt`) is the only carved group.
- **Prepare/commit shipped.**
  - `PreparedWorld` (`:3486`) is built fallibly and carries its `Rebuild` kind (`Load`, `Restore`,
    `Switch`, `:3497`).
  - `commit_world` (`:3554-3675`) is the one infallible commit, and it is used by `load_scene_document`,
    `install_restored` and the transition switch (`transition.rs:422`).
  - Its body is documented as "the one place a rebuild's reset-or-carry decisions live". It has eight
    hand-written `match` rows: diagnostics latch, carriers, keyframe rewind, game clock, session,
    profile, asset cache, GI re-bake.
- **`from_parts` (`:587-664`) does not go through `commit_world`.** It repeats the `install_*` sequence
  by hand (`:655-662` against `:3595-3605`). This is the last duplicated rebuild path.
- **`Applied { result, mutated }` shipped** (`:3339`), and recording reads `mutated`.
  - Handlers report a partial mutation through a side channel on `Engine` (`applied_before_error`,
    `note_applied_before_error` `:2028`), not through their return value.
  - Two gaps remain unrecorded: a `sim.step{hash:true}` whose hash fails after ticks ran, and a handler
    panic (L4016).
- **Host inputs.**
  - The render extent is in `ReplayHeader.render_extent` and is mirrored onto every rebuilt world.
  - `GameClock` is in the snapshot, but not in the header.
  - `InputState` is carried nowhere. Its carry is a separate, hash-moving fix-stage item, not this plan.
  - `KeyframeCheck` (`crates/ludion-engine/src/bisect.rs:517-560`) compares the world doc, rng,
    allocator and frame. It does not compare the clock, extent, input or doc stores.
- **No resource persistence class exists.** The only mover is the game-registered `carry_resource`
  (`builder.rs:474`), used on switch only.

## Proposed design

### 1. Six field groups, by lifetime

The lifetime is exactly the axis the bugs cut along. The groups, with today's fields:

| Group | Fields (today's names) | Touched by a rebuild |
|---|---|---|
| `GameConfig` | registry, systems, doc_kinds, doc_validators, game_doc_kinds, resource_carriers, profile_kind, profile_rel_path, save_root, category_gains, plane, boot_warnings, profile_persistence | never (immutable after build) |
| `SceneIdentity` | seed, scene, scene_name, scene_physics (+ the driver it names) | replaced atomically with the driver |
| `SessionState` | today's struct + doc_validation_warnings, doc_skip_*, profile_migration, boot_profile_pending, audible_emitters | reset/adopt/carry, as now |
| `RenderHost` | renderer, asset_cache, render_size, view_override, render_scale_override, quality, gi_*, shader_reload, stream_ring, present | only through one `forget_world(kind)` |
| `RecordingState` | recorder, pending_doc_reload, applied_before_error, replay_scene_text, query_recording, query_observations | owns "record what was applied" |
| `HostWiring` | subscriptions, event_outbox, shutdown_*, audio_sender, perf | never |

`project_root` is the one field without a clean home: it moves under `load_scene_file`. Decide in W1
whether it belongs to `SceneIdentity` or stays config with an explicit re-root.

### 2. Each group owns its row of the reset/carry table

`commit_world` becomes a sequence of per-group calls, for example `self.render.forget_world(kind)` and
`self.session.on_rebuild(kind)`. Each group's method holds its own `match kind`.

- The table stays readable in one place: the method bodies sit next to their struct.
- A new field is a compile-time question. It must go into a group, and the group's `match` is
  exhaustive over `Rebuild`.

`from_parts` builds a `PreparedWorld` with a new `Rebuild::Boot` kind and commits through the same
function, so no hand-written install list remains.

### 3. The outcome becomes the return value

Handlers return `Applied` directly instead of stashing a partial command in `applied_before_error`.
This closes the two unrecorded gaps above (the `sim.step` hash failure, and the panic path, which then
drops nothing).

### 4. A declared list of sim-visible host inputs

`HostInputs` would be one struct naming every value the sim reads that the host supplies: the render
extent, `InputState`, `GameClock`, and the doc stores.

- The replay header and the snapshot both serialize it.
- Keyframe verification compares it.
- Each group's rebuild row says whether it is carried or reset.

It generalizes into a persistence class on resources (world content / session / scratch). Adopt the
class only if a second resource like vigil's `GateRequest` latch turns up; until then the list is the
cheaper form.

## Waves

- **W1 — carve, no behaviour change.**
  - Move fields into the six groups.
  - Route `from_parts` through `commit_world` (`Rebuild::Boot`).
  - Per-group rebuild methods.
  - No hash, schema, golden or replay moves; the whole suite passing untouched is the gate.
  - Likely 2–3 commits, one crate.
- **W2 — `Applied` as the return value.**
  - Handlers in `crates/ludion-engine/src/commands/` return it.
  - Delete `applied_before_error`.
  - Record the two known gaps, with a regression test each (the replay of a failed-after-mutating
    `sim.step` matches live).
- **W3 — `HostInputs`.**
  - Header and snapshot fields; keyframe comparison.
  - Hash-moving: re-record the `.lrp` corpus last, after the schema snapshot.
  - Needs the held-input carry fix to have shipped first, so `InputState` is serializable.

## Risks

- **A merge-conflict magnet.** `engine.rs` is the most-edited file in the engine, and W1 touches every
  `self.<field>` in it. Schedule W1 when no other arc has uncommitted engine work. Land it as one
  mechanical rename commit before any logic moves.
- **Borrow splitting.** Methods that read one group and write another will need explicit
  `let Engine { render, session, .. } = self;` splits. That friction is the point — it makes a
  cross-lifetime write visible — but it can tempt a `&mut self` helper that re-merges the groups.
- **W3 moves every replay's header.** It is batched with the next hash-moving change if one is queued.

## Exit gate

- `node scripts/check.mjs` green after each wave; `--full` (the 1/2/8-thread identity) after W3.
- W1: `Engine`'s direct fields are the six groups plus the driver; `from_parts` calls `commit_world`.
- W2: no `applied_before_error`; both gap tests pass.
- W3: `KeyframeCheck` compares `HostInputs`; a replay whose header extent or clock is altered fails
  verification with a named field.

## Non-goals

- A typed system-parameter API (the July B3/B4 deferral). It is its own plan.
- Moving any group out of `ludion-engine`. That is `Plan/engine-crate-split.md`'s concern.

## Status

Planned, not started (2026-09-26). The prerequisites shipped in the 2026-09 fix stage: `PreparedWorld`
and `commit_world`, `Applied` recording, and the render extent in the replay header.
