# Sector state persistence — scene residue

> **Status:** Waves 3–4 COMPLETE (2026-08-20, Lore rev 617 engine half + the CLI/docs commit after
> it) — **the plan is mechanized-complete; only Wave 4's human gate remains.** Wave 3: `SceneResidue`
> gained `last_visit` (the switch frame, `FrameIndex`); `RESIDUE_MAX_SCENES = 32` /
> `RESIDUE_MAX_RECORDS_PER_SCENE = 4096` (facade exports); eviction lives in `perform` step (f) after
> the departing scene is inserted — the least-recently-visited *other* scene goes first, ties to the
> lower scene key (the `StableMap`'s order, no extra comparison), an oversized capture keeps its first
> 4096 records — both reported `L3028` (`RESIDUE_EVICTED`, a warning riding the switch's existing
> fold; the switch still happens). The snapshot's `SceneResidueDoc` carries `last_visit` additively
> (`skip_serializing_if` zero — a pre-field document restores every scene at frame 0, so eviction falls
> to the key tie-break; `SNAPSHOT_FORMAT` still 1, residue-free snapshots byte-identical). **The
> `map-check` lint deviated from the text:** duplicate explicit marker names are already the hard error
> `L1025`, so the residue-relevant hazard is an *unnamed* `Residue`-marked marker whose index-derived
> `Name` re-binds on the next map edit — shipped as **`L1032`** (`MAP_RESIDUE_MARKER_UNNAMED`,
> warning, `name_lints` beside `placement_lints`, fixture `lint-residue-unnamed.map.yaml`).
> Wave 4: `ResidueStore` (`crates/ludion-engine/src/residue_store.rs`, prelude export) is the
> decision-10 mirror — `scenes()`, `last_visit`, `record_count`, `get::<T>(scene, name)` with the
> registry's `(TypeId, name)` pairs snapshotted into the store so `get` keeps the decided signature
> without registry access; installed by `install_residue_store` at every world (re)build **after** that
> site's session mutation (reset / adopt / the switch's commit) so it is never a step stale; `clear()`
> is drained by `drain_residue_clear`, third at the frame boundary and **before** `drain_scene_switch`
> (a clear + switch in one frame forgets the old run and keeps the scene being left), on the one path
> live and replay share. Docs: `scene-transitions.md` "What stays behind" (12,001 B of 12 KB),
> `engine-content-kinds.md` residue paragraph (11,159 B), `map-authoring.md` lint list. Tests:
> `transition_residue.rs` 13 → 19 (33-scene eviction deterministic + reported + fresh return, 4097-record
> cap, `last_visit` across snapshot, cross-scene read equal live vs replay, store shape, clear at the
> boundary, clear+switch same frame); the reference game's transition, replay and golden suites unchanged (it marks nothing —
> the invisibility proof). Verified on the laptop: fmt, clippy 0 warnings, engine/core/scene/facade/
> reference-game non-GPU lanes green; GPU suites contend with a concurrent instance here (owed on the reference
> box with the standing queue). Open from the risks list: the record count as a `profile.capture`
> series stays a question, not a task. **Human gate (Wave 4):** clear a sector's persistent content, leave through
> two gates, come back — rides the first consumer's readout (a private plan).
>
> Wave 2 COMPLETE (2026-08-19) — reproducibility completeness. The additive `residue`
> field on `SnapshotDoc` (`SceneResidueDoc`/`ResidueRecordDoc` under the `doc_reloads` precedent:
> `#[serde(default, skip_serializing_if)]`, emitted in ascending scene-key order for free off the
> sorted session map, anonymous records omitting their `name`); `SessionState::adopt(reloads,
> residue)` grown from `adopt_reloads` — `restore` now **adopts** the snapshot's residue instead of
> clearing it. The Wave-2 text's "`Engine::session_residue()` accessor in the `session_doc_reloads()`
> mold" was read as that mold's actual visibility, **`pub(crate)`** — `snapshot::capture` lives
> in-crate exactly like the doc-reload capture, so Wave 1's accessor pair already satisfied it and no
> public accessor exists. All five exit criteria are tests (`transition_residue.rs`, 8 → 13): a
> recorded A→B→A round trip replays bit-identically with the record-kind set asserted unmoved by an
> exhaustive match (`REPLAY_FORMAT_VERSION` still 1); **the decisive seek is arranged by keyframe
> cadence** — the return fires on frame 310, past the default cadence 300, so the recording's
> frame-300 keyframe lands in district B holding district A's memory and the re-driven deltas must
> cross the return (the door comes home open only if the snapshot carried it); a dev-shaped
> snapshot→restart→restore taken away from home preserves residue across the return, with the
> snapshot document's shape asserted directly (scene key, record `name`, mutated bag value, `Name`
> excluded from the bag); a residue-free snapshot omits the field and round-trips byte-identically at
> `SNAPSHOT_FORMAT` 1; a pre-field document restores as no memory. Verified: fmt clean, clippy 0
> warnings, the four transition suites + `allocator_restore` 37/37, the reference game's replay and
> transition suites (restore semantics moved; it marks nothing, so live-vs-replay held unchanged), doc build
> clean.
>
> Wave 1 COMPLETE (2026-08-17) — the `Residue {}` marker, capture/in-place-apply inside
> `transition::perform` (capture held locally, committed only after every fallible check), the
> `SessionState.residue` map with the triad wired (`scene.load` clears, switch carries, `restore`
> clears — the Wave-2 adopt semantics with an empty value), `L3026`/`L3027`, the `scene_residue`
> example, and 8 tests in `transition_residue.rs`. The invisibility proof held: every pre-existing
> transition suite and the reference game's district-switch test green **unchanged**. Two open points resolved in
> implementation: the residue key is the scene's **`name:`** (a path key diverges live-vs-replay —
> recordings embed the boot scene inline; an unnamed scene deterministically keeps nothing), and a
> named record whose authored counterpart is gone **spawns** rather than drops (the contract is
> total). `ComponentInfo::remove` answered: `reserved` gates authoring only, so both loops skip
> `Name` by `TypeId`, never through the registry.
> Authored 2026-08-16 from the reference game's design gap analysis (a private plan).
>
> Engine plan; the other half of `Plan/phase-17-transitions.md` (E7)'s carry seam. First consumer:
> a private plan of the reference game (vigil, which drove per-sector persistence).
> Across-*run* memory stays the profile's job.

## Why

A game whose world is a set of distinct maps (sectors) joined by passages buys that zone model with
the claim that a sector remembers what you did to it: progress made in a sector must survive leaving
it, and a sector's state should be readable **before** entering it.

None of it is possible today: `crates/ludion-engine/src/transition.rs::perform` instantiates the target
into a brand-new world and carries only the entities the game listed, and
the reference game's district-switch test asserts that as the contract.
The engine owns half of a two-halved design — *what travels with you* — and has never owned *what stays
behind*. This plan adds it: **scene residue**, a per-scene overlay of opt-in entity state captured on
the way out and re-applied on the way back in, within one session.

## What exists today

- **The switch** (`crates/ludion-engine/src/transition.rs`). `SceneSwitch::request(scene_path,
  spawn_anchor, carry)`, drained at the frame boundary (`Engine::drain_scene_switch`, third after audio
  and profile) into crate-private `perform`, **transactional** in six steps: (a) copy
  `rng`/`frame`/`run_state`/`speed`/profile text; (b) capture each carried entity's bag over
  `registry.iter()` + `info.serialize`; (c) `instantiate_named_world(target_text, registry)` into a
  **local** world (`L3018`); (d) `anchor_translation(&world, anchor)` (`L3019`); (e) spawn each carried
  bag, overriding only `Transform.translation`; (f) `SimDriver::restore` + `Engine::install_switched`.
  A dead carry id is skipped with `L3020`, never fatally.
- **Non-world session state that already survives a switch** — the private `SessionState`
  (`crates/ludion-engine/src/engine.rs`): `audio_log`, `profile_save_log`, `profile_save_error`,
  `last_transition_error`, `last_system_panic`, `doc_reloads`. Its doc comment states the rule — the
  three world-reset sites (`scene.load`, `restore`, `switch`) each touch it **as one unit**, `reset()` /
  `carry()` / `adopt_reloads()`. `doc_reloads` is the precedent for everything here: engine-owned, never
  hashed, carried across a switch, snapshotted additively, re-applied over a rebuilt world.
- **Additive-format discipline** — `snapshot.rs`: `SNAPSHOT_FORMAT = 1`, `SnapshotDoc` grown by
  `scene_name`, `profile`, `allocator`, `doc_reloads`, each `#[serde(default, skip_serializing_if …)]`
  and documented byte-identical when absent. `crates/ludion-protocol/src/replay.rs`:
  `REPLAY_FORMAT_VERSION = 1`; `ReplayRecord::SceneSwitch { frame, scene_path, document }` (kind 4)
  embeds the target text so a replay never reads disk.
- **Identity and per-component IO** — `Name` is `register_reserved` in `builtin_registry`
  (`crates/ludion-engine/src/components.rs`) and is already the switch's identity mechanism (the anchor
  is found by `Name`). `crates/ludion-ecs/src/registry.rs` gives `ComponentInfo::serialize`,
  `deserialize_insert` **and `remove`** — the three primitives a capture/replace overlay needs.
- **Game-side state that already crosses a gate** — the reference game holds run facts as components on the carried
  run-state, reconciled into per-district entities on arrival by a game system; a
  dedup helper resolves the carried-vs-authored duplicate by keeping the
  **highest** `EntityId`. A test of leaving mid-quest documents the wart this creates:
  the quest NPC is a per-district marker, so gating out mid-quest silently reverts its progress.
- **Headroom** — the `L30xx` block runs to `L3025` (`crates/ludion-core/src/codes.rs`); `L3026` is free.
  A private plan records the streaming trigger ("Entry points already paid for:
  the Phase 10 chunk-door lookup seam + E7's carry-set design"); `Plan/phase-17-transitions.md`'s
  out-of-scope list records async loading and cross-scene references — but **not** scene retention.

## Decisions

**1. Residue is an engine facility, not a game-side summary.** Alternatives: (a) a per-sector summary
component on the carried run-state (a `StableMap<String, SectorRecord>` in the `QuestLog` mold) applied
by a game-side system — zero engine change; (b) engine-captured residue;
(c) dormant retained worlds (decision 2).

(a) is genuinely strong for the *core* of the design: spawners sit at fixed authored positions,
a spawner's state is an enum plus a `destroyed_at` frame, secured is a bit,
and since regrowth is recomputed from a scalar, the game stores only the
fact that fixed the timeline, never the growth. The reference game already proves the pattern crosses a
gate. It fails on the rest: dropped items are physical objects
(arbitrary positions and counts left behind), quest characters live in the world (the wart
above), and each sector holds a spendable fixture. Each forces a hand-written capture/apply pass whose
forgotten field is a silent bug visible only at a human gate — at which point the game has hand-rolled a
generic entity-state store inside one component, and the next game rolls its own again. (b) wins because
**carry and residue are one design and the engine already owns half of it**: a switch already serializes
a full component bag out of the departing world and reconstructs it in the arriving one. Cost: one
built-in component, one `SessionState` field, **no wire method**, no format-version bump.

**2. Dormant retained worlds are rejected**, on five counts. *Memory:* a record is a few component bags
(KBs/sector); a large district's instantiated world is every wall, prop, torch and light — orders of
magnitude larger, ×16. *Snapshot:* `SnapshotDoc.entities` is one world's list; N worlds is a breaking
`SNAPSHOT_FORMAT` bump and keyframes carrying the whole city. *Identity:* `EntityId` allocation is
per-world and its free-list fidelity is a hard-won invariant (`snapshot.rs`'s `allocator` field,
`tests/allocator_restore.rs`); N live worlds needs namespacing or a shared allocator. *Simulation:*
dormant worlds must not tick or sim cost is O(sectors) — and the reference game *wants* the door to reset the
threat, not run off-screen combat.
*Streaming:* not a down-payment but a competing model at the wrong granularity (streaming wants chunks
of **one** world), so it would have to be deleted later.

**3. Opt-in per entity, via a marker component.** Alternatives: capture the whole non-carried world
(= decision 2 through serialization, and resurrects the horde the switch is meant to drop); opt-*out*
(wrong default, same memory profile). A new built-in `Residue {}` — a field-less marker authored
`Residue: {}`, as a game authors its own field-less markers. A game that marks nothing pays nothing: the map stays
empty, every path byte-identical to today.

**4. Identity is `Name`, spelled one way.** A record is `{ name: Option<String>, bag: Vec<(String,
Value)> }`; a named record matches the freshly instantiated entity of the same `Name` on return. `Name`
is **excluded from the bag** — identity is key, never payload (which also sidesteps re-inserting a
reserved component). Two `Residue` entities sharing a `Name` in one scene is **`L3026`**. A `Residue`
entity with no `Name` is *anonymous*: never matched, always respawned — the dropped-item case, free.

**5. Re-application replaces in place; it never despawns and respawns.** For a matched record the engine
removes every registered component the arriving entity carries (`ComponentInfo::remove`, `Name`
excepted) and inserts the bag (`deserialize_insert`). Despawn-and-respawn is easier to write but
recycles slots, perturbing `EntityId` allocation for everything after it and blast-radiusing every
committed hash trace in the consuming game. In-place replace keeps ids stable, so marking an authored
entity `Residue` moves no hash by itself.

**6. Upsert-only: no tombstones.** The engine cannot capture what is already despawned, and diffing the
arrival's authored set against a captured roster is machinery nobody asked for. The contract is a stated
rule instead: **an entity whose state must survive a visit stays alive.** A destroyed spawner keeps its
entity and flips a state component to `Destroyed` — what the design wants visually anyway
(what is left is a husk, not a hole). A collected authored pickup keeps its entity
with a `collected` flag and hides itself.

**7. Ordering is fixed and stated, so `reconcile_singletons`-shaped dedup keeps working.** Inside
`perform`, between (c) and (d): matched records apply in ascending `Name` order, anonymous records spawn
in capture order, then the anchor is located, then carried bags spawn. Carried entities therefore hold
the **highest** `EntityId`s, so `dedup_to_highest_id` still resolves the collision in the carried
entity's favor, unchanged. An entity both carried and `Residue`-marked is contradictory — it left *and*
stayed — so **carry wins**, its capture is skipped, and the conflict is **`L3027`**.

**8. No replay record, because residue regenerates.** Capture is a pure function of the departing world
and the registry; application a pure function of the residue map and the target text; neither reads a
clock or disk. A replay drives the same ticks, so the same system re-requests the same switch at the
same frame and the residue regenerates exactly as the switch itself does (`docs/scene-transitions.md`,
"Reproducibility: the switch regenerates"). `REPLAY_FORMAT_VERSION` and the record-kind set are unmoved.
Residue never enters the `WorldHash` directly — it is engine state, not world content — but the arrival
world it produces *is* hashed, so a residue bug surfaces as a divergence in the existing gate.

**9. The snapshot carries residue, additively.** A `ludion dev` restart or a keyframe seek landing after
a return must reproduce it, so a `residue` field joins `SnapshotDoc` under the `doc_reloads` precedent
(`SNAPSHOT_FORMAT` stays `1`, residue-free snapshots byte-identical). `SessionState`'s triad states the
choice: `switch` **carries** (and updates), `scene.load`'s `reset` **clears** (a load starts a session),
`restore` **adopts** the snapshot's — `adopt_reloads` grows into `adopt(reloads, residue)`.

**10. Systems read residue through a reinstalled mirror resource; only the engine writes.** The reference game wants a
sector's state shown *before* entering, so a system must read residue for a scene it is not in — and
resources are wiped at every world reset. The `DocStore` mold answers it: authority on `SessionState`,
a read-only **`ResidueStore`** resource installed into every fresh world beside
`AudioQueue`/`SceneSwitch`/`DocStore` (`install_audio_queue`, `install_scene_switch`, `install_docs`),
offering `scenes()`, `last_visit(scene)` and a typed `get::<T>(scene, name) -> Option<T>`. Its one write
channel is `ResidueStore::clear()`, a request drained at the frame boundary in the `SceneSwitch` mold,
because a game restarting a run in place must forget the city without a
`scene.load`.

**11. Bounds are explicit and overflow is loud.** `RESIDUE_MAX_SCENES = 32` (twice the reference game's sector count),
`RESIDUE_MAX_RECORDS_PER_SCENE = 4096`. On overflow the least-recently-visited scene's residue is
evicted — deterministic, since the visit frame is sim state — reported as **`L3028`**. Order of
magnitude: ~40 persistent entities/sector × ~4 components ≈ 16 KB/sector, that times the sector count, plus
the same again for the per-world mirror clone.

## Waves

**Wave 1 — capture and apply across a switch (in-memory).** The `Residue` marker (registry, JSON Schema,
generated TS types, `docs/components.md` row); `ResidueRecord`/`SceneResidue` in `transition.rs`; the
`residue: StableMap<String, SceneResidue>` field on `SessionState` with `carry`/`reset` wired; capture in
`perform` step (b′) and application between (c) and (d) per decision 7; `L3026`/`L3027` with `ludion
explain` text; `crates/ludion/examples/scene_residue.rs` (two scenes, a door opened, walk out and back,
still open). **Exit:** a new `crates/ludion-engine/tests/transition_residue.rs` — A→B→A returns a mutated
`Residue` value verbatim while an unmarked sibling resets; the anonymous path respawns, the matched path
keeps its `EntityId`; `L3026`/`L3027` fire on their shapes; **plus the invisibility proof** — a
residue-free two-scene switch produces a `WorldHash` trace bit-identical to the pre-change build (the
clustered-forward mold). `transition.rs`, `transition_replay.rs`, `transition_snapshot.rs` and
the reference game's district-switch test green **unchanged**, its spawns-fresh assertion included —
that test staying true for an unmarked world is the statement that this feature is opt-in.

**Wave 2 — reproducibility completeness.** The additive `residue` field on `SnapshotDoc`;
`SessionState::adopt(reloads, residue)`; an `Engine::session_residue()` accessor in the
`session_doc_reloads()` mold. **Exit:** a recorded A→B→A session replays to a bit-identical per-frame
hash trace with **no new replay record** (`REPLAY_FORMAT_VERSION` and the kind set asserted unmoved); a
keyframe seek landing after the return reproduces the trace; a `dev`-shaped snapshot→restart→restore
across a return preserves residue; a residue-free snapshot document is **byte-identical** to the
pre-change build's, `SNAPSHOT_FORMAT` still `1`.

**Wave 3 — bounds, diagnostics, docs.** The two caps, LRU eviction by last-visit frame, `L3028`;
`docs/scene-transitions.md` gains a "What stays behind" section (and loses the implicit "a switch is a
fresh load" reading); `dev-docs/engine-content-kinds.md` gains the residue paragraph beside the
transition one; `ludion explain L3026|L3027|L3028`; a `map-check` lint for duplicate `Name`s among
`Residue`-marked markers, in the `L1028`/`L1029` mold (`Plan/map-tooling-ergonomics.md`). **Exit:**
`node scripts/check.mjs` green including schema-drift, the embedded-docs mirror and topic size budgets;
a bounds test drives 33 scenes and asserts eviction is deterministic, reported, and never panics.

**Wave 4 — the query surface and the first consumer.** The `ResidueStore` mirror + `clear()` drain
(decision 10) installed at all three reset sites; handoff to the first consumer's private plan.
**Exit:** a system reads a *not-currently-loaded* scene's residue with identical values live and under
replay; `clear()` empties it at the frame boundary and the next return loads the sector fresh.
**Human gate (flagged):** clear a sector's persistent content, leave through two gates, come back — does it *feel*
cleared, and does the pre-entry readout change where you go next? The only judgement this plan
cannot mechanize, and the one that says whether "one sector = one state" delivers the intended tension.

## Engine / game boundary

**Engine (`crates/`):** the `Residue` marker; capture, in-place re-application and ordering inside
`transition::perform`; the `SessionState` field and its reset/carry/adopt decision; the snapshot field;
bounds and eviction; `L3026`–`L3028`; the `ResidueStore` mirror and its `clear()` drain; the docs
topics. The engine stores **facts**, opaquely — it never interprets a bag.

**Game (a consumer):** *which* entities carry `Residue` (spawners, fixtures, quest characters, dropped piles,
doors), what the stored components *mean*, and every recomputation on arrival — regrowth from
`destroyed_at` against a game curve, the secured flag, the pre-entry readout's
presentation. That split keeps residue small: **the engine stores facts, the game recomputes
consequences**, so a spawner destroyed at t=300 and revisited at t=900 has regrown with nothing ticking
while the player was away.

**Generic second consumer.** "The room remembers" is as genre-neutral as transitions themselves: a
metroidvania needs opened doors, thrown switches and beaten minibosses to stay that way for the session
(the across-session half stays `Profile<P>`'s); a dungeon crawler needs cleared rooms to stay cleared
while backtracking; an immersive sim needs a moved crate still where it was left. Each is `Residue: {}`
plus the game's own state component — no engine code per genre, no wire method.

**The streaming door stays open.** Residue is keyed by an **opaque string** and its two operations are
`capture(world, key, entity_set)` / `apply(world, key)` — nothing assumes "exactly one scene is loaded"
or "the key is a scene path". When the streaming trigger fires
(recorded in a private plan), a chunk unload calls the same capture with a chunk key and a
chunk load the same apply — precisely the round trip streaming needs, and the reason decision 2's
retained worlds would have been a detour. `Residue` also answers "which entities survive an unload" once
for both features.

## Deferred / non-goals (triggers recorded)

- **Cross-session / cross-run residue.** Residue dies with the process and with `scene.load`. A
  sector penalty that applies to *following runs* is
  *profile* state. Profile = across runs, residue = across visits
  within one run. Trigger: none — a routing decision, not a deferral.
- **Off-screen simulation** (anything *acting* in a sector you are not in). Recompute-on-arrival covers
  every consequence that is a deterministic function of elapsed time. Trigger: the reference game's open
  question on whether world events happen without the player resolving as *yes, with agency*.
- **Tombstones / remembered despawns** (decision 6). Trigger: a game measuring a real cost in keeping
  disabled entities alive — thousands per sector, not dozens.
- **A `residue.*` wire method.** Inspection rides `engine.snapshot` from Wave 2 on. Trigger: an editor
  workflow that needs to *edit* a sector's residue live.
- **An `Arc`-shared mirror instead of a per-world clone** (decision 10). Trigger: a `profile.capture`
  report showing the clone measurable in the switch frame.
- **Residue for hierarchies.** Like carry, residue is flat: no subtree collection, no `Parent` remap.
  Trigger: the one already recorded for carry in `Plan/phase-17-transitions.md`.

## Risks / open questions

- **Upsert-only is a convention the engine can only partly enforce.** A game that despawns a persistent
  entity gets it back on return with no diagnostic — the engine cannot see what is not there. Mitigated
  by stating the rule in `docs/scene-transitions.md` and by the failure being loud in play (the spawner is
  standing again); an ECS-level despawn hook is the escape if it bites.
- **Anonymous records shift hashes.** Spawning them before the carried bags moves carried `EntityId`s
  relative to today, so the first game using dropped-item residue re-records its committed `.lrp` and
  re-blesses (the standing "content changes shift the committed run" rule). Named residue alone moves
  nothing (decision 5).
- **Name uniqueness at map scale.** Generated districts already name markers per-id,
  but a city of sectors of generated content is where a duplicate first becomes likely. `L3026` catches it at
  runtime; the Wave 3 `map-check` lint catches it offline, where an agent wants it.
- **Silent late-run saturation.** A full-length run dropping items across every sector could approach the
  per-scene cap. `L3028` reports it, but a warning nobody reads is a bug nobody sees — open: should the
  residue record count join the `profile.capture` report as a measured series?
- **[USER CALL] Build this now, or let the reference game hand-roll option (a) for its first slice?** The one-sector
  prototype is the first thing needing any of this, and a hand-rolled
  summary would carry its spawners alone. **Recommendation: build the engine facility** — option (a)'s
  deletion cost is one component and one system today, but a rewrite after spawners, dropped items, fixtures and
  quest characters have each hand-rolled a capture pass; and "before adding a second way, remove the first" makes
  shipping both illegal anyway.
- **[USER CALL] Do dropped items persist in a sector you left?** A taste call that decides whether
  anonymous records ship in Wave 1 at all. Persisting them makes "go back for the cargo you dropped" a
  real decision; dropping them is gentler and cheaper.
  **Recommendation: persist.**
- **[USER CALL] Eviction policy at the bounds.** LRU-drop-with-`L3028` (recommended — never a crash,
  always bounded, and the cap sits at 2× the reference game's sector count so it should never fire), vs. refuse
  further capture and warn (a sector silently stops remembering — worse), vs. hard error (unshippable).
- **Open: does `ComponentInfo::remove` behave on `register_reserved` components?** Decision 4 avoids the
  question by excluding `Name` from the bag, but the implementation should confirm rather than assume.
