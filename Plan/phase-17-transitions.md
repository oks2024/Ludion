# Plan: Phase 17 — In-game scene transitions (E7)

> Status: **landed 2026-07-17 — all 5 waves committed same day (Lore revs 190–192, 194–195), one wave
> per commit, `check.mjs --full` green at Waves 3 and 5.** Implementation amendments, each recorded at
> its wave: (1) **L3020** allocated (Wave 2, orchestrator-settled) — decision 4's "report the dead
> carry id" needed a code; a switch that succeeds but skips a dead carry id leaves an L3020 warning on
> `last_transition_error()`, cleared only by a fully-clean switch. (2) **Doc-reload×switch divergence
> fixed** (Wave 3, review catch — the plan missed it): `install_switched`'s `install_docs` re-walks
> disk, so a doc reloaded before a switch would revert on a disk-reverted replay; a `session_doc_reloads`
> log (fed identically by the live and replay reload paths, cleared at `scene.load`/`restore`) is
> re-applied off-disk after the switch's store rebuild — proven by a neuter-the-fix-and-it-fails test.
> (3) **Restore stays disk-authoritative** (Wave 4): a plain restore reverts reloaded doc stores to disk
> (the E4 posture), and a post-restore switch is consistent with that — pinned by test, not a bug.
> (4) **No seek consumer exists yet** (Wave 4): the first engine-level seek-reconstruction consumer must
> pre-scan `SceneSwitch` records and `install_replay_scene_text` before driving deltas — the recipe is
> pinned by the in-crate seek test. (5) **Wave 5's host stderr print amended away**: `ludion::run` has
> no live host seam after `Server::bind` (the E5 L1122 precedent) — the failure surface is the
> `last_transition_error()` accessor, documented in the guide chapter. Original header follows.
>
> Status: **authored 2026-07-17, ready to start.** This is the wave-level plan for wave **E7** of the
> first-game arc; its brief is `Plan/engine-first-game.md` §"E7 — In-game scene transitions" plus the
> "Cross-cutting rules" there, scheduled by a private roadmap (rules 5 and 8). The
> consumer is game phase **G6 — The City** (a private plan, §G6 / roadmap M6): 2–3 districts as
> separate scenes in `games/vigil/scenes/`, gates that switch the loaded district mid-run, carrying the
> player and the run state across the switch without breaking determinism or replays. The E7 brief's
> "Proposed design" items are confirmed or amended below with current-code evidence (amendments called
> out inline). The thesis: **a scene switch is `scene.load`'s twin, not its clone** — it rebuilds the
> world from a target scene like `scene.load`, but where a load *starts a session* (frame 0, reseed,
> fresh logs) a switch *continues* one (the run clock keeps ticking, the RNG stream keeps flowing, the
> player travels), and where a load reads only disk a switch must reproduce off-disk in a replay. The
> whole facility is a new engine-owned frame-boundary action requested through a resource (the
> `AudioQueue`/`SceneSwitchRequest` mold) and performed by the host at the frame boundary — **no wire
> method, no new component, no format bump**, one additive replay record kind (the E4 `DocReload`
> precedent).
>
> Like E3/E4/E5 before it, E7 **starts on user direction ahead of its milestone**: per roadmap rule 2
> E7 is the engine wave in flight during M5 (its consumer is G6/M6), and its start ahead of the queued
> M0–M4 human gates is the same rule-2-throttle override the user exercised for E3, E4, and E5 (the
> notebook records each) — the queued human gates are unaffected (E7 shares only the guide/docs files
> and additive serde/record surfaces with them).
>
> **E7 composes with the fully-landed Phase 16 (E5) at HEAD.** All five E5 waves are committed:
> `EngineBuilder::profile::<P>` + `Profile<P>` (`crates/ludion-engine/src/player_profile.rs`), the
> save drain beside `drain_audio`, `write_atomic`, and the profile-at-start embedded in both
> `ReplayHeader.profile` (`crates/ludion-protocol/src/replay.rs:128`) and `SnapshotDoc.profile`
> (`crates/ludion-engine/src/snapshot.rs:62`). E7 must carry the profile across a switch and keep both
> embeddings intact — this plan reuses the E5 `emit_profile_text`/`install_profile_from_text` seams
> verbatim rather than adding any profile machinery.

## Current-code facts that shape the work

- **`scene.load` is the world-reset point E7 must *fork*, not reuse — because it resets exactly the
  three things a switch must preserve.** `load_scene_document`
  (`crates/ludion-engine/src/engine.rs:734`) checks + instantiates the scene, projects entity `name:`
  into `Name` components (`:747-753`), then rebuilds the driver with **`SimDriver::new(world, schedule,
  SimConfig { seed, ..default })`** (`:762-769`) — which **starts the frame counter at 0** and
  **reseeds the RNG from the scene's `seed:` (default 0)** (`:754`, `reseed` at `:795`). It then
  **clears the audio log** (`:773`) and **the profile save log** (`:776-777`). A district switch must
  do the opposite on all four: the run clock **keeps running** (FrameIndex continuity — the brief's
  "carrying run state"), the RNG **keeps flowing** (one uninterrupted seeded stream — the
  determinism pillar's "one seeded `DeterministicRng`"), and the session logs **keep accumulating**
  (a switch is mid-session, not a new session). So the switch cannot call `load_scene_document`; it is
  a sibling path that shares the check→instantiate→Name-projection→`build_schedule`→`install_docs`
  front half but swaps the driver construction and the log/seed handling.
- **`SimDriver::restore` + `DriverRestore` is the ready-made continuity constructor.** Snapshot restore
  already rebuilds a driver that **carries** the frame, RNG, run-state, and speed:
  `ludion_sim::SimDriver::restore(world, schedule, DriverRestore { rng, frame, run_state, speed,
  tick_rate })` (`snapshot.rs:197-207`). The switch builds its new driver the same way — copying the
  live RNG out first (`*engine.rng()?`, the `snapshot::capture` mold at `snapshot.rs:111`,
  `engine.rng()` at `engine.rs:916`) and passing the **current** `engine.frame()` (the post-tick
  next-to-sim frame), the current `run_state()`/`speed()`. This is the single mechanical difference
  from `scene.load` (`SimDriver::new` + fresh `SimConfig`) that gives the run its continuity for free.
- **The frame-boundary drain trio is the exact seam for a switch.** `advance_live`
  (`engine.rs:340`), `advance_observed` (`:359`), and `step_ticks` (`:375`) each run a tick then call
  `drain_audio(frame)` and `drain_profile(frame)` (`:346-347`, `:366-367`, `:377`), where `frame` is
  the pre-tick frame whose systems emitted the requests. A switch is a **third drain** in the same trio
  — `drain_scene_switch(frame)` — that reads a request resource a game system set during the tick and,
  if present, performs the switch. Ordering: it runs **last** (after `drain_profile`), so a
  `profile.save()` requested in the same frame is written before the world is rebuilt. `drain_audio`
  (`:384`) is the precise mold: read-and-take a resource, act, never touch the sim tick.
- **Resources are wiped at every world reset and reinstalled from persistent registrations — the
  request resource joins that discipline.** `World::restore_entities` clears resources, so
  `install_audio_queue` (`engine.rs:453`), `install_docs` (`:469`), and `install_profile` (`:487`) are
  re-run at construction, `scene.load` (`:772,782,786`), and `restore` (`:1338,1346,1352-1355`). The
  `SceneSwitchRequest` resource (default = no pending switch) is installed by a new
  `install_scene_switch` at those same three points — including inside the switch itself, so the
  freshly-built district starts with an empty request. It is excluded from `WorldHash` and snapshots
  exactly like `AudioQueue` (`audio.rs`), so it trips no drift gate and needs no registry append.
- **The carry set is a registry-driven serialize→respawn, and the precedent is `snapshot::capture` +
  `migrate_component`.** Capturing an entity's components is `for info in registry.iter() {
  info.serialize(world, id)? }` into a `Map<String,Value>` (`snapshot.rs:88-98`); reinstating them is
  `registry.deserialize_insert(name, value, world, entity)` (`snapshot.rs:246`, `registry.rs:158`).
  The switch reuses both verbatim: it captures each carried entity's component bag from the **live**
  world *before* the rebuild, then after loading the target scene it `world.spawn()`s a fresh entity
  (`world.rs:79`) per bag and re-inserts each component — the same serialize-then-deserialize-insert
  round-trip a snapshot restore performs, so byte-equality of carried components is inherited from that
  machinery, not new code.
- **Anchors are already expressible: `Name` is a built-in reserved component and `Transform` is
  `Copy`.** `Name(pub String)` (`components.rs:35`) is projected from every scene entity's `name:`
  field at load (`engine.rs:747-753`) and enters the world hash; `Transform` (`components.rs:42`) is
  `Clone, Copy` with a `[f32;3] translation`. The switch finds the anchor by scanning the freshly-built
  world for the entity whose `Name` equals the request's `spawn_anchor` and reads its
  `Transform.translation` — **no new component needed** (the brief's "no new component unless the wave
  plan finds otherwise" — confirmed).
- **A replay drives ticks through the same primitives, so a switch *regenerates* under replay — the
  only thing replay lacks is the target scene's text off-disk.** `replay_hashes_with`
  (`crates/ludion-engine/src/replay.rs:157`) loads the header scene (`load_header_scene` `:216`),
  reseeds (`:167`), installs the header profile (`:173-175`), then drives every record through
  `drive_actions` (`:87`), whose `Command::SimStep` arm calls `engine.execute(SimStep)` → `step_ticks`
  → `advance_live` (`:94-102`) — **so `drain_scene_switch` fires during replay too**. The game's
  switch-triggering system re-runs each tick and re-sets `SceneSwitchRequest` at the identical frame
  (deterministic: same seed, inputs, profile). What replay cannot do is **read the target district off
  disk** — the exact cross-machine gap the `SceneRef::Inline` boot-scene embedding (`replay.rs:82-87`),
  the `DocReload` inline text (`replay.rs:164-179`), and the `ReplayHeader.profile` text
  (`replay.rs:128`) all already close. E7 closes it for districts the same way.
- **A new replay record kind is additive within `REPLAY_FORMAT_VERSION` 1 — the `DocReload` kind byte
  3 is the proof.** The record kind is a 1-byte discriminant scanned against `KIND_MAX`
  (`replay.rs:456`); `KIND_DOC_RELOAD = 3` was added at format 1 with `KIND_MAX` bumped and no framing
  change (`replay.rs:65-71`, module note `:30-42`), and every pre-`DocReload` `.lrp` still decodes
  (`replay.rs:32-38`). E7 adds **`KIND_SCENE_SWITCH = 4`**, bumps `KIND_MAX` to 4, and `write_record`
  (`replay.rs:279`) / `decode_record` (`replay.rs:485`) grow one arm each — `REPLAY_FORMAT_VERSION`
  stays 1 (`replay.rs:60`), old `.lrp` decode unchanged. *(E5 added a header field and no record kind;
  E7 adds a record kind — a settled difference, flagged in the report, not a format bump.)*
- **`schema_hash` excludes the replay/snapshot types, so the record-kind addition cannot move it, and
  no `Command` variant is added, so the wire surface stays 26.** `schema_hash` is over
  `Request`/`Response`/`Event`/`Command`/`ResponseBody`/`Hello`/`ServerHello`/`FrameHeader` only
  (`crates/ludion-protocol/src/schema.rs:36-54`), which `ReplayRecord` is not part of — the E4
  `DocReload` and E5 header/snapshot additions both left `schema_hash` unmoved (board-recorded). The
  switch is requested via a resource and performed at a drain, never dispatched as a command; `record_kind`
  (`crates/ludion-engine/src/recording.rs:54`) is an **exhaustive** `Command` match with no wildcard,
  and E7 adds no `Command` variant to it, so the parity gate and `schema_hash` are both untouched.
- **Snapshot/seek across a switch is *nearly free* if the switch sets `engine.scene` to the target's
  inline text.** `SnapshotDoc` already carries `scene: Option<SceneRef>` (`snapshot.rs:49-50`),
  `scene_name` (`:53-54`), the E5 `profile` (`:62-63`), the RNG (`:65`), the frame (`:41`), and every
  live entity's components (`:66-67`); `capture` reads `engine.scene_ref()` (`:108`) and
  `install_restored` rebuilds from `restored.scene` (`engine.rs:1332-1334`). If the switch sets
  `engine.scene = SceneRef::Inline { document: <target text> }` and `engine.scene_name` to the target's
  name, then a snapshot taken **after** a switch already embeds the switched district inline, its
  entities already include the carried player, and its `profile` already carries the in-memory profile —
  so `restore` (and a keyframe seek) reconstruct district 2 with **no snapshot-format change**
  (`SNAPSHOT_FORMAT` stays 1). `ludion dev`'s snapshot→restart→restore is that exact path.
- **The profile must be *carried*, not re-read, across a switch — and the E5 seams do it P-agnostically.**
  `emit_profile_text` (`engine.rs:502`) renders the live `Profile<P>` to canonical text without naming
  `P`; `install_profile_from_text` (`engine.rs:514`) installs from text off-disk. The switch emits the
  live profile before the rebuild and installs it into the new world from that text — preserving
  mid-run unlock progress across a district change (re-reading disk would revert it). This is the same
  off-disk continuation `restore` already prefers over the disk load (`engine.rs:1352-1355`).
- **Free L-codes: the engine-runtime `L30xx` band, next free `L3018`.** `L30xx` is the engine-runtime
  protocol/replay/snapshot band (`crates/ludion-core/src/codes.rs:379-457`), latest `L3017`; the
  nearest neighbour is **`L3013` `REPLAY_SCENE_UNRESOLVED`** — "a scene a replay names by path could
  not be loaded" (`codes.rs:440`), the precise analog of "a scene a switch names could not be loaded".
  E7 takes **`L3018`** (target district could not be loaded/validated → switch rejected, sim continues)
  and **`L3019`** (named spawn anchor not found in the target district → switch rejected). *(The
  content `L11xx` band's next free is `L1123`, but a mid-run switch failure is a runtime event beside
  `L3013`, not authored-content validation like `L1119`/`L1120`.)* Registration is three-part — a
  `const` in `mod code`, a `CodeEntry` in `CODES`, a `DECLARED_CODES` entry — enforced by the bijection
  and reachability tests (`codes.rs`); a new const must be **referenced outside `codes.rs`**, so each
  code registers in the wave where its raising site lands (the E5 `L1122` discipline).
- **The content root is the scene file's directory; districts share one root, so their docs are one
  set.** `project_root = scene_path.parent()` (`builder.rs:283-294`, `project_root` at `:103`); vigil's
  districts all live in `games/vigil/scenes/`, so `save/`, `voxels/`, and any `weapon`/`enemy`/`npc`
  docs resolve identically before and after a switch. The switch therefore **reinstalls `install_docs`
  on the new world** (same root → same doc set, generation reset to 0 like a fresh scene) — no
  per-district root juggling, and the E4 doc walk and the E5 `save/profile.yaml` skip-kind
  (`ENGINE_KINDS`/`ENGINE_OWNED_KINDS`, `docs.rs:31` / `lib.rs:463`) keep working unchanged.
- **The facade host is where a live switch reads disk and records — `ludion::run` already has the
  shape.** `run` (`crates/ludion/src/lib.rs:168`) roots at the scene file (`load_scene_file`, `:202`),
  enables profile persistence for the live run (`:227`, the disk-gated side effect mold), reports the
  boot migration (`:228`), and arms recording before the engine moves to the sim thread (`:232-247`).
  A live switch resolves the target text from **disk** and (when recording) writes a `SceneSwitch`
  record; a replay resolves it from the **embedded record text** — the same live-writes / replay-reads
  gating that governs profile persistence (`EngineBuilder::persist_profile`) and the audio
  sink (`set_audio_sender`, `engine.rs:605`).
- **Prelude + example + guide molds.** The prelude re-exports the engine's game-facing types
  (`crates/ludion/src/prelude.rs:45-49`, incl. `AudioQueue`, `Profile`, `DocStore`); E7 adds
  `SceneSwitch` (the request resource). Facade example mold: `crates/ludion/examples/data_docs.rs` /
  `player_profile.rs` with a two-run WorldHash behavioral test. Guide chapters are `##` H2 in
  `docs/engine-guide.md` ("Game data-docs", "Player profiles"); docs topics are `docs/*.md` stems gated
  by the `embedded_topics_match_the_docs_directory` drift test. `games/vigil` today is a single-scene
  plaza — G6 authors the second and third districts and the gates; E7 ships only the engine facility +
  a two-scene facade example.

## Design decisions (settled by this plan)

1. **The switch is requested through a `SceneSwitch` resource carrying an explicit
   `SwitchTo { scene_path, spawn_anchor, carry: Vec<EntityId> }`, performed at the frame boundary by a
   new `drain_scene_switch` — the `AudioQueue`/`SceneSwitchRequest` mold, no wire method.** A game
   system calls `world.resource_mut::<SceneSwitch>().request(scene_path, spawn_anchor, carry_ids)`
   during its tick; the engine drains it last in the tick trio (after `drain_profile`). This confirms
   the brief's "sim requests, host performs (the queue pattern again)" and the `SceneSwitchRequest`
   shape, with one amendment: the **`carry` list is an explicit `Vec<EntityId>` the game supplies**
   (typically `[player_id]`, found by the game's own `Name`/marker query), not a hidden convention.
   *Rejected:* a `scene.switch` **wire command** — it would add the 27th wire method, move
   `schema_hash`, and put the sim in the position of issuing a command to itself (the sim owns `World`
   and requests via resources; the host performs — architecture §; a switch is not a peer-client
   action). *Rejected:* a new **builtin `Carry` marker component** the switch scans for — a new
   registry-borne component (scene validation, inspector, schema, world-hash bless) for a value the
   game already knows (its player entity); the explicit `carry` list costs nothing and keeps the switch
   free of a new component (confirming the brief).

2. **The carry set travels as *entities and their components*, not as game-registered resources; run
   state lives on carried components (+ engine-carried FrameIndex/RNG for the clock) — amending the
   brief.** The brief lists "the player entity's components … plus game-registered carried resources
   (run timer, HP, inventory)." But **resources are excluded from snapshots and from `WorldHash`**
   (fact above): a carried *resource* would vanish on a `ludion dev` reload or a seek across a switch,
   and never reach the determinism trace. So E7 carries **only entities+components**: HP and inventory
   are components on the carried player entity; the **run clock is FrameIndex continuity** (the switch
   preserves the frame counter, so `run_elapsed = frame - run_start_frame` is derived, never stored as
   a carried resource); the **RNG stream continues** (the live `DeterministicRng` is carried into the
   new driver, not reseeded). A game needing a scalar to survive a switch models it as a component on a
   carried entity. This is also the **streaming-ready** shape the roadmap's rule-8 entry point requires
   ("design them as if chunks will one day carry entities the same way, same serialize-respawn seam"):
   the carry seam is entity migration, and it is the exact `snapshot::capture`/`deserialize_insert`
   round-trip a chunk hand-off would use.
   *Rejected:* a `SceneSwitch` that also carries a game-registered `Resource` set — snapshot/seek would
   silently drop it and the determinism trace would never see it; forcing carried state onto entities
   keeps one honest rule (everything that survives a switch is hash-visible and snapshot-captured).
   *Rejected:* storing the run clock as a carried resource — FrameIndex continuity makes it free and
   keeps the timer inside the deterministic sim.

3. **A switch *continues* the session: it forks `scene.load`'s front half but builds the driver via
   `SimDriver::restore`/`DriverRestore`, carrying (frame, RNG, run-state, speed), and does not clear the
   session logs or honour the target scene's `seed:`.** The switch lives in a new
   `crates/ludion-engine/src/transition.rs` (the `player_profile.rs`/`docs.rs` module precedent). It:
   (a) copies the live RNG and reads `engine.frame()`/`run_state()`/`speed()` **before** the rebuild;
   (b) captures each carried entity's component bag from the live world; (c) checks + instantiates the
   target scene into a fresh local world with the `Name` projection (the `load_scene_document`
   front half, `engine.rs:739-753`); (d) locates the anchor and respawns the carried entities at it;
   (e) builds the driver with `SimDriver::restore(world, build_schedule(), DriverRestore { rng: <live>,
   frame: <engine.frame()>, run_state, speed, tick_rate: DEFAULT })`; (f) reinstalls `install_audio_queue`
   / `install_scene_switch` / `install_docs`, and **carries the profile** via `emit_profile_text` →
   `install_profile_from_text` (decision 5); (g) sets `engine.scene = SceneRef::Inline { target text }`
   and `engine.scene_name`, and **leaves `audio_log`/`profile_save_log`/`profile_save_error` intact**
   (a switch is mid-session — the opposite of `scene.load`'s `:773,776-777`). The target scene's
   `seed:` is **ignored** on a switch (the run's RNG stream is uninterrupted).
   *Rejected:* calling `load_scene_document` and then patching the frame/RNG back — it resets the driver
   via `SimDriver::new`+`SimConfig` (frame 0, reseed) and clears the logs, so "patch back" would be
   re-deriving continuity the `restore` constructor gives directly; forking the front half and building
   via `DriverRestore` is the honest expression. *Rejected:* honouring the target scene's `seed:` on a
   switch — it would fork the RNG stream mid-run, breaking reproducibility and the one-seeded-stream
   pillar.

4. **The switch is *transactional*: it builds the new district in a local world and swaps it in only on
   full success; any failure (target won't load → `L3018`, anchor missing → `L3019`) rejects the switch,
   leaves the live world untouched, and the sim continues in the current district — confirming the
   brief's safe answer.** Because the target scene instantiates into a **local** `world` value (the
   `load_scene_document` pattern, `engine.rs:740`), a load or anchor failure simply discards that local
   value — the live driver is never touched, so the sim runs on in the current district and the failed
   frame is a deterministic no-op on the world. The failure is a **warning-shaped `Diagnostic` stored on
   the engine** (the `profile_save_error` mold, `engine.rs:414-448`) and exposed via a
   `last_transition_error()` accessor for tests + the host's stderr; it **never panics or stops the
   sim**. A carried `EntityId` that is not live at drain time is skipped (it carries nothing) rather than
   aborting the switch — but because a well-formed game sets the request in the same frame it names live
   entities, a dead carry id is a game bug; the switch reports it in the same warning rather than
   silently dropping the player. *(Determinism note: whether a switch fails at the load step, e.g. under
   replay with no embedded text for a switch that failed at record time, or at the anchor step, e.g.
   live with the district loaded, the outcome is the identical world no-op, so the `WorldHash` trace is
   the same; only the diagnostic message — not hashed — differs.)*
   *Rejected:* aborting the whole process, or leaving the world half-rebuilt, on a bad target/anchor — a
   mid-run crash on content the player triggered violates "load-failure is a diagnosed error, not a
   crash" (the brief) and the errors-are-UX pillar.

5. **The player profile is carried across a switch through the E5 text seams, never re-read from disk.**
   The switch calls `emit_profile_text()` (live `Profile<P>` → canonical text) before the rebuild and
   `install_profile_from_text(text)` after — installing the exact in-memory profile into the new world,
   preserving unsaved mid-run progress (an unlock earned this run must survive walking through a gate).
   A profile-free game is a no-op on both (both return `None`/skip). This reuses `engine.rs:502` and
   `engine.rs:514` unchanged — **zero new profile machinery**, and it mirrors how `restore` already
   prefers the embedded profile over the disk load (`engine.rs:1352-1355`).
   *Rejected:* re-reading `save/profile.yaml` at a switch — it would revert mid-run profile changes and
   couple a district change to disk state that a replay does not carry.

6. **Cross-machine replay rides an additive `ReplayRecord::SceneSwitch` (kind byte 4) carrying the
   target district's canonical text inline; the switch is *regenerated* under replay, and the record is
   consumed as the off-disk text source, not as a drive action.** During a live/recording run the switch
   drain resolves the target text from **disk** and, when a recorder is active, writes a
   `SceneSwitch { frame, scene_path, document }` record (the `write_doc_reload` mold,
   `replay.rs:262-276`, via a new `recording::write_scene_switch` beside `write_delta`). Under replay,
   `replay_hashes_with` **pre-scans** all `SceneSwitch` records into a `BTreeMap<String,String>`
   (path→text) installed on the engine as its replay scene-text source (the `install_profile_from_text`
   pre-drive step at `replay.rs:173-175` is the mold), then drives the records — `SceneSwitch` records
   are **skipped** in the `drive_actions` action loop (like `Keyframe`, `replay.rs:200`), because the
   switch **regenerates** from the sim's re-set request and only needs the text. When the regenerated
   drain fires, it resolves the target text from the installed map (never disk under replay — enforcing
   cross-machine purity, the `apply_doc_reload_inline` off-disk posture). This confirms the brief's
   "scene text for every visited scene inlines into LRP1, extending the existing single-scene
   embedding," and settles the task's open question in favour of an **additive record carrying inline
   text at each switch** over a header map.
   *Rejected:* a **`ReplayHeader.scenes` map** of every visited district — the header is written **once,
   up front** by `ReplayWriter::new` (`replay.rs:232-240`) before any district is visited, so it cannot
   accumulate progressively-visited scenes without the game **declaring its whole district set at build
   time** (speculative surface the vertical-slice pillar forbids); a per-switch record needs no upfront
   declaration and matches `DocReload` exactly. *Rejected:* making the `SceneSwitch` record **drive** the
   switch during replay (suppressing the regenerated request) — the record would then have to embed the
   whole carry set (serialized components + anchor), coupling the replay format to entity serialization
   and re-deriving state the reproduced sim already holds; regeneration carries only the scene *text* in
   the record and re-derives the carry from the live world (the audio/profile "regenerated, not recorded"
   posture, applied to the switch decision while the *external text* rides a record like `DocReload`).

7. **Snapshot/seek/`dev` across a switch are free of a snapshot-format change: the switch sets
   `engine.scene` to the target's inline text, so `SnapshotDoc` already captures the switched district.**
   Because the switch updates `engine.scene`/`engine.scene_name` (decision 3) and the carried player is
   an ordinary entity in the new world, `snapshot::capture` (`snapshot.rs:85-113`) already embeds the
   post-switch district inline, its entities, the carried profile text, the carried RNG, and the carried
   frame — so `restore_into` (`snapshot.rs:145`) rebuilds district 2 with the player, and `SNAPSHOT_FORMAT`
   stays 1. `ludion dev`'s snapshot→restart→restore in district 2 therefore works through the **existing**
   restore path with no new code. A replay **seek** that lands after a switch installs the same replay
   scene-text map (decision 6) before reconstructing, so a switch between the keyframe and the target
   regenerates with its text available; a switch before the keyframe is already baked into the keyframe.
   *Rejected:* embedding a district list in `SnapshotDoc` — the single `engine.scene = Inline{target}`
   assignment makes the post-switch snapshot self-contained with the field that already exists.

8. **Anchor placement sets the carried entity's `Transform.translation` to the anchor's translation;
   the rest of each carried component is re-inserted verbatim.** The anchor is a plain `Name`d entity in
   the target district (`gate-from-plaza`), invisible or decorative, whose `Transform.translation` is
   the drop point. The switch finds it by `Name` in the fresh world, and for each carried entity that has
   a `Transform`, sets `translation` to the anchor's (leaving rotation/scale and every other component
   as carried). This is the minimal predictable rule; anchor-driven **facing** (setting rotation from the
   anchor) is deferred with a recorded trigger (a game that needs the player to face into the district).
   Carry brings each listed entity's whole subtree; `Parent` and every marked entity reference between
   carried entities are remapped to the respawned ids. A reference to an entity that stayed behind
   arrives empty (optional field) or leaves its component behind (required field), warning `L3043`.
   *Rejected:* copying the anchor's whole `Transform` onto the carried entity — it would overwrite the
   player's scale with the marker's; translation-only is the least-surprising drop.

9. **No wire method, no new component, no format bump, zero golden churn; one additive replay record
   kind and two `L30xx` codes.** Wire surface stays **26**, `schema_hash` unmoved (record kinds are
   outside it); `REPLAY_FORMAT_VERSION` and `SNAPSHOT_FORMAT` both stay **1** (an additive kind byte and
   the reused `scene` field); no `Command` variant, so `record_kind`'s exhaustive match is untouched
   (`recording.rs:54`); no renderer change, so **no golden churn by construction** (a transition is
   temporal — nothing renders differently in a single frame). New surface is exactly: `KIND_SCENE_SWITCH
   = 4` + its `ReplayRecord::SceneSwitch` variant, the `SceneSwitch`/`SwitchTo` resource,
   `transition.rs`, `drain_scene_switch`/`install_scene_switch`, `L3018`/`L3019`.
   **If any wave discovers otherwise, that is a flagged decision, not a drive-by.**

10. **Non-visual wave: a two-scene facade example crossing a gate + behavioral WorldHash tests + G6 as
    proof.** `crates/ludion/examples/scene_transition.rs` ships two tiny scenes (a start room and a next
    room with a `gate-from-start` anchor), a system that requests the switch when the player reaches the
    gate carrying `[player]`, and a **cross-transition WorldHash behavioral test**: drive the run across
    the gate, assert the trace is identical on a second run (regeneration) and that the carried player's
    HP/inventory components are byte-equal after the respawn at the anchor. The E4/E5 non-visual posture
    (example + behavioral test, no gallery golden) applies — a transition renders nothing new in a frame,
    so there is **no gallery scene and no golden**. G6 consumes exactly this to connect its districts.
    *Rejected:* a gallery scene with a blessed golden — there is no single-frame visual the golden would
    pin; the behavioral/determinism tests are the real proof (the E4/E5 precedent).

## Waves

Ordering rationale: the pure additive protocol record first (Wave 1, testable with no engine wiring —
the Phase 16 "protocol type first" mold) → the in-process switch execution + carry + continuity + codes,
the meat everything else composes with (Wave 2) → the replay embedding + regeneration + the cross-machine
reproducibility story on top (Wave 3) → snapshot/seek/`dev` across a switch (Wave 4) → the runnable
example, host wiring, guide, docs, and exit (Wave 5). Each wave is independently landable, one commit,
ending in `node scripts/check.mjs` green.

### Wave 1 — the additive `ReplayRecord::SceneSwitch` record kind

- `ludion-protocol`: `KIND_SCENE_SWITCH = 4` (bump `KIND_MAX` to 4, `replay.rs:71`); a
  `ReplayRecord::SceneSwitch { frame, scene_path: String, document: String }` variant (the `DocReload`
  variant mold, `replay.rs:169-179`); `frame()` and the `write_record`/`decode_record` arms
  (`replay.rs:182-197,279-291,485-524`); a `ReplayWriter::write_scene_switch(frame, scene_path,
  document)` writer + a borrowed/owned body pair (`DocReloadBody`/`DocReloadBodyOwned` mold,
  `replay.rs:310-324`). Module note updated: the switch kind is additive within `REPLAY_FORMAT_VERSION`
  1, no framing change (`replay.rs:30-42`).
- **Exit gate (mechanized):** a `scene_switch` record round-trips through `ReplayWriter`/`ReplayReader`
  carrying its inline text intact (the `doc_reload_record_round_trips` mold, `replay.rs:713`); a
  `write_record` round-trip of the decoded variant (the `write_record_round_trips_a_doc_reload` mold,
  `:751`); a `seek` treats it as a delta at its frame; **every pre-E7 committed `.lrp` still decodes**
  (kinds 0–3 unaffected; the `a_header_without_a_profile_key_decodes_as_none` posture); an unknown kind
  5 is still rejected (`replay.rs:456`). `REPLAY_FORMAT_VERSION` unchanged. `node scripts/check.mjs`
  green.

### Wave 2 — in-process switch: `SceneSwitch` resource + `transition.rs` + carry + continuity + `L3018`/`L3019`

- `SceneSwitch` resource + `SwitchTo { scene_path, spawn_anchor, carry: Vec<EntityId> }` in
  `transition.rs`: `request(scene_path, spawn_anchor, carry)` sets the pending switch; `take()` clears
  and returns it (the `AudioQueue::take` mold). Prelude re-exports `SceneSwitch`.
- `install_scene_switch` inserts a default (empty) `SceneSwitch` at construction, `scene.load`,
  `restore`, and **inside the switch** (the `install_audio_queue` mold, `engine.rs:453,772,1338`).
- `transition::perform(engine, SwitchTo) -> Result<(), Diagnostic>`: copy the live RNG + read
  `frame`/`run_state`/`speed`; capture each carried entity's component bag (the `snapshot::capture`
  loop); check+instantiate the target text into a local world with the `Name` projection; locate the
  anchor by `Name` (missing → **`L3019`**); `spawn` + `deserialize_insert` each carried bag, setting
  `Transform.translation` to the anchor's; build the driver via `SimDriver::restore`/`DriverRestore`
  (continuity); reinstall audio queue / scene-switch / docs; carry the profile via
  `emit_profile_text`→`install_profile_from_text`; set `engine.scene = Inline{target}` +
  `scene_name`; **do not** clear the session logs. A target that fails check/instantiate → **`L3018`**;
  both leave the live world untouched (transactional) and store the warning on the engine
  (`last_transition_error()`).
- `drain_scene_switch(frame)` after `drain_profile` in `advance_live`/`advance_observed`/`step_ticks`
  (`engine.rs:346-347,366-367,377`): `take()` the request; if present, resolve the target text from
  **disk** (`project_root.join(scene_path)`) and call `perform`; a resolve/perform failure is the
  non-fatal warning (sim continues). *(Recording of the switch is Wave 3; a Wave-2 recording that
  crosses a switch replays by disk read — portable/cross-machine replay lands in Wave 3.)*
- `ludion-core`: **`L3018`** (target district could not be loaded/validated at a switch — rejected,
  sim continues) and **`L3019`** (named spawn anchor not found in the target district — rejected)
  registered three-part; `ludion explain L3018`/`L3019` work.
- **Exit gate (mechanized):** an in-process test drives a run to a gate, requests a switch carrying the
  player, and asserts: the world is now the target district's entities **plus** the carried player at
  the anchor's translation; **carried components are byte-equal** before/after (HP, inventory, `Name`);
  the **frame counter is continuous** (no reset to 0); the **RNG stream continues** (the post-switch
  hash sequence matches a control run that never resets the stream); `audio_log`/`profile_save_log`
  survive the switch; the in-memory **profile is carried** (an unlock set this run is still set after
  the switch). A **missing anchor** → `L3019`, world unchanged, sim continues; a **malformed/missing
  target** → `L3018`, world unchanged. Two identical drives across a switch produce the identical
  `WorldHash` trace (in-process determinism via `drive_hashes`). `node scripts/check.mjs` green.

### Wave 3 — replay embedding: record the switch, regenerate off-disk, the reproducibility story

- `recording::write_scene_switch(recorder, frame, scene_path, document)` beside `write_delta`
  (`recording.rs:138`); `drain_scene_switch`, when a recorder is active and a live switch succeeded,
  writes the `SceneSwitch` record carrying the **disk text just read** (the `AppliedDocReload`/`DocReload`
  seam, `recording.rs:41-48,144-148`).
- The engine gains a replay scene-text source (`Option<BTreeMap<String,String>>`): under replay the
  drain resolves the target text from it (never disk — cross-machine purity); live it stays `None`
  (disk + record). `replay_hashes_with` **pre-scans** the reader's `SceneSwitch` records into the map
  and installs it after `load_header_scene`/`reseed`/`install_profile_from_text` (`replay.rs:166-175`);
  `SceneSwitch` records are **skipped** in the `drive_actions` action loop (the `Keyframe` skip,
  `replay.rs:200`) — the switch regenerates from the sim's re-set request.
- **Exit gate (mechanized):** the **cross-transition reproducibility test** — a recorded session that
  crosses a gate reproduces its `WorldHash` trace from the committed `.lrp`, run twice. The
  **cross-machine test** — the same `.lrp` reproduces on a checkout whose target-district file is
  **reverted or absent** (the embedded record text wins; the disk-reverted `DocReload` replay test is
  the mold). A run that switches to district B **diverges** from a run that stays in A (the switch is
  hash-visible). **Every pre-E7 committed `.lrp` still reproduces** (no `SceneSwitch` records → no map →
  unchanged). Wire surface still **26**, `schema_hash` unmoved, `REPLAY_FORMAT_VERSION`/`SNAPSHOT_FORMAT`
  still 1. `node scripts/check.mjs --full` green (the determinism canary).

### Wave 4 — snapshot / seek / `dev` across a transition

- Confirm (and test) that the Wave-2 `engine.scene = Inline{target}` assignment flows through
  `snapshot::capture` (`snapshot.rs:108`) so a post-switch snapshot is self-contained; the replay
  scene-text map (Wave 3) is installed before any seek-based reconstruction so a switch after a keyframe
  regenerates with its text. No new `SnapshotDoc` field, no `SNAPSHOT_FORMAT` change.
- **Exit gate (mechanized):** a snapshot taken **after** a switch round-trips (`capture` →
  `restore_into`) into the switched district with the carried player intact and the frame/RNG/profile
  continued; a **`ludion dev`-style snapshot→restart→restore in district 2** rebuilds district 2 with
  the player (the existing restore path, now exercised across a switch); a **replay seek** landing after
  a switch reconstructs the correct district and its post-seek trace matches the linear replay. A
  profile-free / single-district snapshot round-trips **byte-identically** to before E7 (no field
  perturbation). `node scripts/check.mjs` green.

### Wave 5 — facade example, host wiring, guide chapter, docs topics, exit

- `crates/ludion/examples/scene_transition.rs` (decision 10): two inline scenes, a gate system that
  requests the switch carrying `[player]`, and the cross-transition WorldHash behavioral test
  (regeneration + carried-component byte-equality) — the copyable G6 pattern.
- Host wiring in `ludion::run`: after a live switch fails, print `last_transition_error()` to stderr
  (the `report_profile_migration` mold, `lib.rs:228,507`); a live run already reads target districts
  from disk (the switch drain) and records them under `--record` (Wave 3) — no new host flag.
- Engine-guide chapter **"Scene transitions"** (after "Player profiles"): the `SceneSwitch` resource +
  `request(scene_path, spawn_anchor, carry)`, the `Name`d-anchor convention, what carries (components on
  the carry-set entities; run clock = FrameIndex continuity; RNG continues) and what does not (resources,
  the target scene's `seed:`), the transactional load-failure/missing-anchor semantics (`L3018`/`L3019`,
  sim continues), the reproducibility guarantee (the switch regenerates; district text inlines into the
  `.lrp`), snapshot/`dev` across a switch, and the non-goals (no async/streamed load, no preload, no
  cross-scene query). Docs topics via the `ludion-docs` skill: `ludion-engine` (the switch/carry/drain +
  continuity), `ludion-protocol` or the replay topic (the `SceneSwitch` record kind), `determinism`
  (the switch regenerated + district text embedded + FrameIndex/RNG continuity), `ludion` (facade +
  prelude `SceneSwitch`). No new crate → no new topic; the
  `embedded_topics_match_the_docs_directory` drift test satisfied by rebuild.
- **Exit gate (roadmap "ready"):** the record round-trip (W1) + in-process carry/continuity/failure
  tests (W2) + cross-transition + cross-machine reproducibility (W3) + snapshot/seek/`dev` (W4) all
  green; wire surface **26**, `schema_hash` unmoved, `REPLAY_FORMAT_VERSION`/`SNAPSHOT_FORMAT` still 1,
  no new component, no golden churn; the two-scene example + behavioral test green; guide chapter + docs
  topics shipped. **Handoff to G6:** districts are separate scene files in `games/vigil/scenes/`, a gate
  system sets `SceneSwitch.request(next_district, anchor, [player, …])`, run state rides carried
  components + FrameIndex continuity, and the cross-district replay-determinism gate is the W3 embedding.
  **No human gate is owned by E7** — M6's G6 gate ("a run that spans ≥2 districts where the route choice
  cost something") judges the facility in anger.

## Explicitly out of scope (triggers recorded)

- **Async / streamed / preloaded loading, adjacent-district streaming** — a switch is a synchronous
  frame-boundary world rebuild (a district load is milliseconds of voxel-kit instantiation, not a stall
  worth hiding). Belongs to the **seamless-streaming long-term goal** whose entry point is the Phase 10
  chunk-door seam **plus this wave's carry-set serialize/respawn seam** (roadmap "NOT in this arc",
  the private reference-game notebook); trigger: the prototype proving the game.
- **Cross-scene queries / references** (an entity in district A naming one in district B) — one district
  is loaded at a time (the Dead Rising zone model); trigger: seamless streaming.
- **Anchor-driven facing / full-transform placement** — the switch sets the carried entity's translation
  only; trigger: a district entrance that must orient the player (set rotation from the anchor).
- **Game-registered carried *resources*** — everything that survives a switch is an entity/component or
  engine-carried (frame/RNG); trigger: a genuine singleton a game cannot model as a component (none
  foreseen — the run clock is FrameIndex-derived).
- **A `scene.switch` wire command / editor-driven district switch** — the switch is sim-requested; an
  editor "jump to district" would be a peer-client command; trigger: a designer workflow that needs to
  drive a switch from the editor (would add a wire method — a flagged decision then, not now).
- **Honouring the target scene's `seed:` on a switch** — the run's RNG stream is uninterrupted; trigger:
  none (a per-district reseed would break run reproducibility).

## Risks

- **A switch that silently resets the run clock or forks the RNG would break the run's identity.** The
  whole point of decision 3 is continuity: FrameIndex carries, the RNG stream carries, the logs carry.
  Contained by building the driver via `SimDriver::restore`/`DriverRestore` (not `SimDriver::new`+
  `SimConfig`, which resets both) and by the Wave-2 continuity assertions (frame not reset, RNG hash
  sequence matches a no-switch control). A reviewer confirms `transition::perform` never calls
  `load_scene_document` and never `reseed`s.
- **The new record kind is a replay-format change.** Contained by making it additive
  (`KIND_SCENE_SWITCH = 4`, `KIND_MAX` bumped, `REPLAY_FORMAT_VERSION` stays 1 — the `DocReload`
  posture) and by gating on both the old-`.lrp`-still-decodes test (W1) and the cross-machine
  reproducibility test (W3). `schema_hash` composition (`schema.rs:36-54`) confirms `ReplayRecord` is
  outside it, but the W3 gate re-asserts `schema_hash` and the 26-method surface unmoved regardless.
- **Regeneration + record-as-text-source must not double-apply or miss.** The switch regenerates from
  the sim's re-set request while the `SceneSwitch` record supplies only the text — so the record is
  **skipped** in the drive loop (never a second switch) and the text map is **pre-loaded** before
  driving (so the regenerated drain always finds its text). Contained by the W3 cross-transition test
  (exactly one switch per gate crossing, trace identical) and the skip being the same `Keyframe` skip
  the reader already trusts. A reviewer confirms `drive_actions` has no `SceneSwitch` arm and the map is
  installed pre-drive.
- **A mid-run switch failure must never crash or half-rebuild the world.** Contained by the transactional
  design (decision 4): the target instantiates into a local world, so a load/anchor failure discards it
  and the live driver is untouched; every failure path is a warning (`L3018`/`L3019`) stored on the
  engine, never a `panic!`/`unwrap` on content. A reviewer confirms `perform` performs no mutation of
  the live `engine.driver` before every check has passed.
- **Snapshot/seek/`dev` across a switch depends on `engine.scene` being the inline target.** If the
  switch set `engine.scene` to a `Path` (or left it stale), a post-switch snapshot would not be
  self-contained and a `dev` reload would rebuild the wrong district. Contained by decision 7's
  `engine.scene = Inline{target}` assignment and the W4 tests (post-switch snapshot round-trips into the
  switched district; profile-free snapshot byte-identical). `SNAPSHOT_FORMAT` stays 1 by construction
  (the field already exists).
- **Sequencing vs open gates.** E7 adds no wire method, no component, and no golden; it shares only the
  guide/docs files, two additive `L30xx` codes, and one additive replay record kind with the open
  M0–M4 human gates, so it is safe to run with those queued. Per roadmap rule 2 E7 is the engine wave in
  flight during M5 (its consumer is G6/M6); its start ahead of the queued human gates is the E3/E4/E5
  rule-2-throttle-override precedent, gates unaffected.
