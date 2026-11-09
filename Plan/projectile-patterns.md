# Plan: Projectile patterns — closed-form volleys and the `kind: pattern` document

> Status: **Waves 1–2 SHIPPED (2026-09-12) — W3–W4 open.** Design settled 2026-09-04 (grilling session — every ruling the user's); the Status section at the end is the shipping log.
> Sequenced **after** `Plan/particles-engine-and-editor.md` Wave 1 (the motion kernel it evaluates on)
> and `Plan/sprite-engine.md` W1 (the atlas frame a bullet kind names). Every claim of current-code fact below was read from the tree on
> 2026-09-04.
>
> **Relationships.** The gameplay sibling of `Plan/particles-engine-and-editor.md`: the same
> evaluator, one hashed entity per fired *volley* instead of a visual-only emitter. Answers the
> question `Plan/bullet-hell-stress.md` opened (can the engine carry a Cave-class field — that plan's
> `games/shmup-stress` instrument migrates here and keeps one entity baseline). Bullets draw as
> `Plan/sprite-engine.md` sprites, and that plan's W4 measurement against `shmup-stress` and this
> plan's budget gate are **one** reference-box run. The editor half is the **pattern tab** of the
> Effects view the particles plan opens. Consumed in-repo by `games/vigil` (the ranged archetype,
> a private plan) and driven by the **external cave-style shmup** the user writes as a real
> user (the sprite engine's own driver). **The wire surface stays at 29 methods.**

## Why

The shmup needs thousands of hashed, replayable bullets on screen and a way to author boss patterns by
hand. `games/shmup-stress` proved the entity-per-bullet shape works and where its cost lands: at
~3,000 live bullets the laptop's untrusted numbers put the game's own systems under 1 ms, `extract` at
2–4 ms, and `world_hash` at 12–21 ms per frame — the hash is paid only by the offline profiler and by
replay recording, never by a live tick, but it prices every determinism tool the engine is built on,
and a whole-field cancel is thousands of spawns and despawns in one frame. A bullet whose trajectory
is fixed the moment it is fired (the Cave/Touhou rule) does not need to be an entity: it is a pure
function of `(volley, time)`, exactly what the particles plan's kernel computes. Storing one entity
per *volley* with a per-bullet kill bitset keeps the hash a few hundred bytes per volley, makes cancel
a despawn of a few hundred entities, and — because the sim's hit test and extract call the same
function — leaves nothing to drift. The reference game supplies the in-repo consumer the engine rule
demands: a ranged enemy's direct fire, and later a modified shot that fans into a rain of projectiles.

## What exists today (verified)

- **`games/shmup-stress`** (`Plan/bullet-hell-stress.md`, W1 shipped 2026-08-20): bullets are entities
  (`Transform` + `Sprite { texture: None }` + `Bullet { velocity, life }`), `Emitter { count, rate,
  speed, life, phase, spin }` lays down rings as a plain system, `player_hit_test` is a brute-force
  circle scan that never touches the spatial index, and every scene's live census is a closed form of
  the frame index asserted by `tests/stress.rs`. No budget is calibrated; the private CI's
  shmup-stress profile configuration dumps the three reports as artifacts on the reference box.
- **Vigil's ranged fire is an entity per shot.** `spawn_enemy_shot` (`movement.rs:536`) spawns a
  `Transform` + emissive `Mesh` sphere + `GiDynamic` + `EnemyShot { velocity, life, damage, radius }`
  (`components.rs:250`); `move_enemy_shots` (`combat.rs:114`) integrates and hit-tests. The ranged
  `EnemyDoc` fields are `attack_cooldown`, `shot_speed`, `shot_damage`, `shot_radius` (`docs.rs:50-61`);
  the mortar (`MortarShell`, `tick_mortar_shells`, `shot_flight_time`) lobs a stateful round to a
  landing marker. The player's bolt carries a tracer `PointLight`.
- **Data-docs are opt-in per game and hot-reloadable.** `Engine::builder().doc_kind::<T>("kind")`
  (`crates/ludion-engine/src/builder.rs:237`) walks the content root into a `DocStore<T>` resource;
  `asset.reload { path }` re-validates one doc from disk and upserts it (`reload_doc_from_disk`,
  `engine.rs:228`), and the recorder writes it as a `DocReload` record so a replay re-applies the same
  edit. A doc's schema is the same `schemars` output the wire validates against.
- **The editor can already write and list files.** The Tauri host exposes `write_editor_file`
  (`editor/app/src/terminal/pty.ts:99`) and `list_editor_dir(path, ext)`
  (`editor/app/src-tauri/src/files.rs:113`); the Game Designer view's designed loop
  (`Plan/editor-views.md` Slice #2) is "a schema-driven form edits the doc file, hot reload applies
  it" — no doc method exists on the wire and none is needed.
- **Time is an integer tick.** `TickRate::DEFAULT` is 60 Hz (`crates/ludion-core/src/time.rs`);
  `Animation.started: FrameIndex` is the convention for "when the choice happened"; `extract_at(now)`
  passes sim time as a parameter.
- **Registry and codes.** `ParticleEmitter` appends 24th (the particles plan); `Volley` is 25th and
  `PatternPlayer` 26th. `L2107` is the particles validator; **`L2108` is free** and this plan takes it.
  Scene-domain codes run to `L1143`, `L1144`–`L1147` are the sprite engine's; this plan reserves
  **`L1148`–`L1150`**. No CLI code.
- **This checkout is a low-power laptop**: it can run every test and render, bless no golden and
  calibrate no budget; each wave names what is owed on the reference box and the M4.

## Decisions

**1. A gameplay bullet is one index of a closed-form `Volley`; the entity is the volley.** `Volley` is
a hashed component on its own entity, spawned when a volley fires and reaped when its last bullet is
dead: `source: EntityId` (who fired — the game reads the source's components for side and damage),
`pattern: String` + `bullet: String` (the doc and the bullet kind by name), `started: FrameIndex`,
`shape`, `count`, `angle` (absolute, the captured aim already folded in), `spread`, `speed`, `seed`,
and `killed: [u64; 4]` — a kill bitset of at most 256 bullets (`L2108` above it). Its `Transform` is
the origin captured at fire time. Bullet *i*'s position at `now` is the kernel's `position_at(age)`
over the bullet kind's segment list, from the volley's origin along `angle + i × step`; alive while
`age < life` and `killed[i] == 0`. Extract evaluates live bullets into `SpriteInstance`s (the sprite
path: unlit, premultiplied, `layer`, additive per instance, atlas frame); the sim evaluates the same
function for hits. Nothing per bullet is stored, so the same `(volley, now)` yields the same field in
any process, on any thread count, after any snapshot.
- *Rejected — entity per bullet (`shmup-stress` today):* ten times the hash bytes and replay size,
  the per-frame extract walk, the cancel churn (Why). Kept as the **escape hatch**: a bullet that must
  react after spawn (homing on the live player, bouncing, a steered laser) is an ordinary entity in
  game code, rare and few; and kept as **one baseline scene** in `shmup-stress` so the two shapes are
  measured side by side.
- *Rejected — one `ParticleEmitter` with a gameplay companion:* the same substance as this decision
  with the bullet description on a flat disc primitive (the particles plan's decision 12.4).
- *Rejected — a `Collider` per bullet:* the `steady-indexed` measurement already showed
  `spatial_rebuild` as the whole difference; volleys have no per-bullet collider and never enter the
  index. The pure overlap query (decision 6) is the hit test.
- *Not bridged — 3D swept shots:* the pooled, physics-swept 3D projectiles are a separate system,
  `Plan/bulk-projectiles.md`; volleys do not feed it (trigger recorded there).

**2. Motion is a list of at most four segments, each with a closed form.** A segment is `ramp
{ frames, from, to }` (speed linear from `from` to `to` along the current heading) or `turn { frames,
speed, rate_deg }` (constant speed, heading rotating at `rate_deg` per second — a circular arc). A
segment with both a ramp and a turn has no elementary solution and is `L1149`. Heading and position
carry across segment boundaries; after the last segment the bullet continues straight at its final
speed until `life` (frames, on the bullet kind). The particles plan's `ballistic { gravity, drag }` is
the third variant, used by FX only. `position_at(t)` sums the completed segments and the partial
current one — O(4) per evaluation, `ludion_core::math` for the transcendentals. "Slow, then curve
toward the player" is `ramp` then `turn`, the classic shape.

**3. Three volley shapes.** `ring` (count evenly over the full turn), `arc { center: absolute | aim,
spread_deg }` (count evenly over the spread, centred on an authored angle or on the aim captured at
fire), `random { spread_deg }` (each bullet's angle and speed drawn inside the cone from
`DeterministicRng::derived_salted(seed, FrameIndex(volley index), bullet index)`). A spiral is a ring or
an arc with a per-repeat angle step; a `line`/wall shape waits for a pattern that needs it.

**4. `kind: pattern` is a pure function of time, one level deep, no expressions.**

```yaml
kind: pattern
name: boss-phase-1
bullets:
  needle:
    frame: bullets/needle        # atlas frame (sprite-engine W1); omit for an untextured quad
    size: [0.3, 0.3]
    color: [1.0, 0.4, 0.8, 1.0]
    blend: additive
    hit_radius: 0.12
    life: 240                    # frames
    motion:
      - ramp: { frames: 30, from: 6.0, to: 2.0 }
      - turn: { frames: 90, speed: 2.0, rate_deg: 45.0 }
timeline:
  - at: 0                        # frames after the player's `started`
    repeat: 8
    every: 12
    shape: ring
    count: 24
    bullet: needle
    speed: 4.0
    angle_deg: 0.0
    angle_step_deg: 7.5          # per repeat — the spiral
    speed_step: 0.0              # per repeat
    speed_range: [4.0, 4.0]      # per bullet index, linear over the volley
  - at: 60
    shape: arc
    center: aim
    spread_deg: 40.0
    count: 5
    bullet: needle
    speed: 6.0
  - at: 120
    play: sweep-left             # another pattern's timeline, offset — one level deep
loop: 240                        # frames; 0 = play once
```

A timeline entry is a volley or `play: <pattern>`; `play` inside a played pattern, a cycle, or a
`play` of an unknown name is `L1150`; an unknown bullet kind is `L1148` (did-you-mean); a `motion`
list over four segments, a ramp-and-turn segment, `count` over 256, `every: 0` with `repeat > 1` are
`L1149`. Every bound schemars can express is `L1014` and the validator covers only the rest. Timing is
in **frames** (fire offsets, `every`, `loop`, segment `frames`, `life`) — exact integers at the fixed
tick, the genre's own unit, and what the census closed forms need; speeds in units per second. `loop`
makes a boss phase; a phase change is the game setting a different `pattern` on the player.
- *Rejected — expressions, conditions, arithmetic:* a language (the no-DSL pillar); the vocabulary is
  a fixed enumerated set like the sprite engine's effect ops, and anything beyond it is Rust.
- *Rejected — a rank/intensity knob on the engine:* the game picks the doc per difficulty; a rank
  multiplier is a recorded trigger.

**5. Aim comes from a named target entity, read at fire time.** `PatternPlayer { pattern: String,
started: FrameIndex, target: Option<EntityId> }` is the 26th component: `play_patterns` walks the
player's timeline from `started` (looping on `loop`), and on a volley's frame captures the player
entity's translation as the origin and, for `center: aim`, the direction to `target`'s translation;
a missing or dead target degrades to the absolute angle with an `L2108` warning once, never a crash.
The engine never knows what a player is.
- *Rejected — the game writing an aim point every frame:* per-frame writes on a hashed component for
  a value read once per volley.

**6. The engine ships components, evaluator, two systems and one query; the game owns every hit,
side, damage and cancel.** `play_patterns` (stands volleys up on schedule) and `advance_volleys`
(reaps a volley whose bullets are all dead or past `life`) are `pub fn`s the game registers in its own
schedule order — the `spatial_rebuild` precedent. `volley_hits(volley, transform, doc, now, centre,
radius) -> impl Iterator<Item = u32>` is the pure overlap query; the game's hit systems call it and
set `killed` bits (a player hit, a player shot on an enemy — the shmup's own shots are volleys of
count 1–3 with the ship as `source`). Cancel is a game-side despawn over a `Volley` query; what
appears in a cancelled bullet's place (score items, an FX burst through a `ParticleEmitter`) is the
game's spawn at that frame. Nothing hashed changes except the bits the game sets.

**7. Units and precision.** A bullet's age is `(now − started − at) × seconds_per_tick`; the longest
authored life (a few hundred frames) keeps every closed form in comfortable `f32` range, pinned by a
test at `life` 600 against an `f64` reference.

**8. Registration is opt-in through the existing mechanism.** `PatternDoc` is an engine-owned
`schemars` type registered by a game through `doc_kind::<PatternDoc>("pattern")` — vigil and
`shmup-stress` register it, breakout never walks for pattern files. No new loader path.
- *Rejected — the engine loading it for every game as it does assemblies:* an assembly is a model
  a `Mesh` names; a pattern doc is game data and follows game data's rule.

**9. Consumers.** (a) **`shmup-stress`**: `steady` and `burst` migrate to a pattern doc played by
`PatternPlayer`s (the four staggered emitters become four players with `phase`-equivalent `started`
offsets); one **entity baseline** scene keeps today's `Emitter`/`Bullet` path so the A-versus-B
number is a committed artifact; the census closed forms in the scene headers stay valid; the
determinism + census gate covers both. (b) **Vigil**: the ranged archetype's direct fire becomes
`games/vigil/docs/patterns/ranged-bolt.yaml` (one `arc { center: aim }` bullet per `attack_cooldown`),
`EnemyDoc` gains `pattern: String` (serde default empty = no player), `EnemyShot`/`spawn_enemy_shot`/
`move_enemy_shots` are **deleted** ("before adding a second way, remove the first"), and a vigil hit
system applies `volley_hits` against the player with the doc's `shot_damage`. The mortar keeps its
shell (a landing is stateful) and the player's bolt keeps its entity and tracer light until a modified
shot gives a reason to move it. The look changes from an emissive sphere to an additive sprite — the
sitting judges it. (c) **The external shmup** on its local path dependency is the driver and the human gate (a from-git build waits on the user's decision to publish the dependency, ruled 2026-11-02).

**10. The Effects view's pattern tab.** A schema-driven form over one `kind: pattern` file (the same
`schemars` output that validates the doc; nested lists get list widgets, not a raw-JSON bucket — the
`inspect-fields` rule is for components, the Game Designer form is the doc precedent), a file picker
over `list_editor_dir(<project>/docs, "yaml")` filtered on `kind: pattern`, save = `write_editor_file`
then `asset.reload { path }`. The stage subject carries a `PatternPlayer` naming the open doc and a
`target` dummy entity the designer can drag with the existing gizmo; restart re-stamps `started`; the
time strip is the emitter tab's. **A doc edit writes the file immediately**, unlike a scene edit,
because the file is the doc's only truth — stated in the view and in `ludion docs patterns`. Zero wire
movement; the parity gate holds because an agent does exactly the same two steps.

**11. Docs.** `docs/patterns.md` (new spoke ≤ 12 KB: the doc form, the segment vocabulary, the
boundary, the frames rule), a `docs/components.md` row each for `Volley` and `PatternPlayer` (the
split the particles plan pays), the prelude pointers, one line in `editor/personas/effects-artist.md`.
The `ludion-docs` skill writes every topic.

**12. Rulings register (2026-09-04).** Graze: a second bitset behind a trigger (the external shmup
counting near-misses). Cancel-to-score-items, bullet-destroying shots, lasers: triggers. Rank scaling:
the game picks the doc. `line` shape: trigger. Reactive bullets: entities. Frames for timing. The
migration set of decision 9. The exit gates below.

## Waves

### Wave 1 — the engine: doc, components, systems, query, docs
- **`crates/ludion-core`**: `L1148`–`L1150`, `L2108`.
- **`crates/ludion-engine`**: `Segment::Ramp`/`Segment::Turn` added to the particles kernel (their
  closed forms pinned at known `t`, boundary carry-over, the four-segment cap); `PatternDoc` + its
  validator (`ludion-scene`'s generic walk, the codes above); `Volley` (25th, `register_validated`,
  `L2108`) and `PatternPlayer` (26th); `play_patterns`, `advance_volleys`, `volley_hits`;
  `extract_volleys` feeding `RenderScene.sprites` through the existing sprite path; prelude exports;
  `crates/ludion/examples/pattern_player.rs` (a ring pattern on a key press, a hit counter).
- **Tests**: the closed forms; a played timeline stands volleys up on the exact frames (loop
  included, `play` nesting, the `L1150` cycle); the census of a `shmup-stress`-shaped doc equals its
  closed form; `volley_hits` against a known circle at known `t`; a kill bit removes a bullet from
  both the query and extract; the hash-neutrality proof's twin (a volley world extracted every frame
  vs never: identical `WorldHash` trace, byte-identical `.lrp`); `extract_is_deterministic` extended;
  `asset.reload` of a pattern doc re-shapes the live field on the next frame and records a
  `DocReload`; `ludion explain` for the four codes.
- **Docs**: decision 11.
- **Exit, here**: everything above green serially; `check.mjs --rust-only` detached.

### Wave 2 — `shmup-stress`: the migration and the one measurement
- Decision 9(a): the pattern doc, the two migrated scenes, the entity baseline scene, `tests/stress.rs`
  covering both shapes with the same census, `tests/profile.rs` dumping all four reports; the TeamCity
  artifact list extended.
- **Owed on the reference box**: `budgets/pinned-rtx3090-shmup-stress.toml` calibrated from the
  artifacts (the bullet-hell plan's owed follow-up, paid here) — `extract_ms`, the sprite pass, and the
  sim's `play_patterns`/`advance_volleys` lines — **the same run as `Plan/sprite-engine.md` W4's**
  3,000-sprite measurement; the M4 twin.

### Wave 3 — vigil: the ranged archetype
- Decision 9(b): the doc, the `EnemyDoc.pattern` field, the hit system, the deletions, the
  `tunables.rs` numbers moved into the doc. `games/vigil/tests/patterns.rs`: a ranged enemy stands a
  volley on its cooldown aimed at the player, the hit lands the doc's damage once and sets the bit, a
  mortar doc stands no player, the committed replay re-derives its trace.
- **Re-record**: `run.lrp` and the segment replays once at the wave's end (hashed components on
  ranged enemies), through the replay-scoped bless, then `cargo test -p ludion-protocol`.
- **Owed on the reference box / the M4**: the district goldens re-proved (a boot frame fires no
  volley; any that moves is a finding).

### Wave 4 — the pattern tab
- Decision 10 in `editor/app`; the stage subject and target in `effects-stage.yaml`; the persona
  line; the live-smoke list in `Plan/editor-live-smoke.md`.
- **Exit, here**: `check.mjs --frontend-only` green; the parity audit at 29 methods.
- **Human gate (the plan's exit)**: your external shmup's first boss pattern authored in the Effects
  view and played on its local path dependency; the vigil ranged-fire look at the next sitting.

## Engine/game boundary

- **`crates/ludion-core`**: the four codes.
- **`crates/ludion-engine`**: the kernel variants, `PatternDoc`, `Volley`, `PatternPlayer`, the two
  systems, the query, the extract step. **`crates/ludion-renderer`**: nothing — bullets are sprites.
- **`editor/app`**: the pattern tab; nothing on the wire.
- **`games/shmup-stress`**: the doc, the scenes, the baseline. **`games/vigil`**: which enemies play
  which doc, the hit system, the damage, the deletions. **The external shmup**: every pattern, every
  hit rule, cancel, items, sides.

## Deferred / non-goals

- **Graze** — trigger: the external shmup scores near-misses; a second `[u64; 4]` and a query.
- **Cancel to score items** — the game spawns them at the cancel frame; no engine shape needed.
- **Bullet-destroying player shots, lasers and beams** — triggers: the external shmup needs them;
  a laser is a different primitive, not a volley.
- **Rank / dynamic difficulty** — trigger: the shmup has a rank system to feed a multiplier.
- **`line` / wall shape** — trigger: a pattern that cannot be said with the three.
- **Reactive bullets in the engine** — never: they are entities in game code.
- **A time scrubber** — the particles plan's trigger; shared with the emitter tab.
- **A pattern profile-report section** — the system lines and `extract_ms` already carry the cost.

## Risks / open questions

- **The degenerate pattern.** A timeline firing `count: 1` volleys every frame is entity-per-bullet
  again with a doc on top. The census math is in `docs/patterns.md`; a `map-check`-style lint is a
  trigger, not a wave.
- **Extract evaluates every live bullet every frame**, and the sim evaluates the ones the hit test
  asks about. At 3,000 bullets that is O(4) segments each, cheaper than the ECS walk it replaces —
  but it is a claim until Wave 2's artifact.
- **A doc edit changes bullets already in flight** (the volley references the kind by name). Right
  in the stage, surprising in a game; `ludion docs patterns` says so.
- **Vigil's look change** (sphere to sprite) rides a sitting; the deletion is reversible only by
  history.
- The laptop can bless or calibrate nothing; every "owed" line names the box.

## Status

### Wave 1 — shipped 2026-09-12 (driven by the external shmup's request 01)
- `crates/ludion-engine/src/patterns.rs`: `PatternDoc` + `validate_pattern_docs` (`L1148`–`L1150`), `Volley`
  (25th, validated, `L2108`) and `PatternPlayer` (26th), `play_patterns` / `advance_volleys`, `volley_hits`,
  `bullet_motion` / `bullet_position` / `bullet_is_alive` / `bullet_age`; `extract_volleys` feeds
  `RenderScene.sprites` before the sprite sort; `Segment::Ramp` / `Segment::Turn` (+ `straight`,
  `end_heading`, `end_speed`) in the particles kernel; `EngineBuilder::patterns()`; prelude exports;
  `crates/ludion/examples/pattern_player.rs`; `crates/ludion-engine/tests/patterns.rs` (the Wave 1 test
  list, the `L2108`-once check through a live log session); `docs/patterns.md`, the `components-fx` rows.
  Wire surface unchanged (29 methods, hash `73b28b8a495cd4b5`); the registry is 26 components.
- **Rulings made while building (not in the decisions above):**
  1. Pattern references (`PatternPlayer.pattern`, `play:`) are `DocStore` **keys** — content-root-relative
     paths such as `patterns/boss.yaml` — never a doc `name`, the same rule every other doc reference follows.
  2. Segment speeds are absolute units/s; an omitted `ramp.from` / `turn.speed` **carries the speed in force**
     (the volley's fire speed for the first segment, the previous segment's end speed after). The bullet
     continues straight at its end speed after the last segment.
  3. `speed_range`, when present, **replaces** `speed` as the per-index range; `speed_step × repeat` shifts
     both ends. A bullet kind names its art as the sprite pair `atlas` + `frame` (not the plan's `frame` alone).
  4. Headings live in the game's `Plane` (`Engine::builder().plane(...)`, shipped with the shmup's request 02):
     θ → `right·cos θ + forward·sin θ`; a turn's normal is `right × forward`, so a positive `rate_deg` turns
     toward increasing θ on both planes (`Plane::normal()` would flip the sense on XZ).
  5. `Volley.seed` = `derived_salted(player id, frame, VOLLEY_SEED_SALT ^ ordinal)` where the ordinal counts every
     candidate volley the frame's timeline walk passes, so a seed never depends on which siblings were due.
  6. Opt-in is one call, `.patterns()` (doc kind + validator together): forgetting the validator was the trap.
  7. A data doc the strict-YAML subset rejects (flow style) was silently skipped by the content walk — found
     here, fixed (`peek_kind_checked`, reported as `L100x`, live reload rejected). Author pattern docs in block style.
- **Owed:** W2 (`shmup-stress` migration + the reference-box budget), W3 (vigil ranged fire), W4 (the pattern
  tab). The `play_patterns` walk is `world.entities()` + `get` per entity, not a typed query (the resource borrow) —
  W2's measurement decides whether that matters. A `count: 1` cadence lint stays a trigger.

### Wave 2 — shipped 2026-09-12 (the `shmup-stress` migration and the reference-box budget)
- Scene set after the wave: `steady.yaml` and `burst.yaml` are **volleys** (four `PatternPlayer`s staggered a
  frame apart playing `scenes/patterns/{steady,burst}.yaml` — a ring of 25/50 every 4 frames, `life: 119`,
  one atlas region, on `steady-drawn`'s plane, camera and backdrop); `steady-drawn.yaml` is the **entity
  baseline** and A/B twin (unchanged data, sprite W4's gate); `steady-indexed.yaml` stays on entities (indexing
  is per-entity) and moved onto the same plane so `Plane::Xy` — a per-game property — does not flatten its
  grid. The undrawn entity `steady`/`burst` are gone. `cancel_bullets` despawns `Bullet`s and `Volley`s alike;
  `player_hit_test` tallies both shapes (the entity scan + `volley_hits`), setting no kill bit so the census
  stays the header's closed form. Order: `spatial_rebuild → play_patterns → emit_bullets → move_bullets →
  advance_volleys → cancel_bullets → player_hit_test → scroll_backdrop`. `tests/stress.rs` asserts census +
  determinism per scene and that `steady`/`steady-drawn` agree frame by frame.
- **Measured on the RTX 3090 (2026-09-12, four 200-frame release runs each; the record is
  `budgets/pinned-rtx3090-shmup-stress.toml`):** the same 2,975-bullet field costs extract 0.4229 ms as volleys
  against 1.9321 as entities, `world_hash` 0.1782 against 5.7944, sim frame 0.1185 against 0.2513; the volley
  hit query is the one line that costs more (0.1076 against a 0.0376 brute scan — it evaluates every bullet).
  Ceilings: `extract_ms` 0.85, `scene` 0.12, `play_patterns`/`advance_volleys` at the 0.05 floor,
  `player_hit_test` 0.22, `frame_total_ms` 16.6 fixed. Wired as a `budgets (windows)` step in the private
  `provision.mjs`; the shmup-stress-profile dump already lists all four scenes.
- **Findings:** `play_patterns`/`advance_volleys` cost ~0.035 ms each idle on the 2,975-entity baseline (an
  `entities()` walk with a `get` per entity) — the typed-query fix is the notebook's open item, now priced.
  `steady-drawn`'s extract re-measured 1.932 against the morning's 1.694 after the `Sprite` uv fields and this
  migration landed; recorded in the sprite budget's prose, ceiling untouched, no bisection yet.
- **Owed on the M4:** `budgets/pinned-apple-m4-shmup-stress.toml` measured there (never derived).

### Wave 3 — shipped 2026-09-13 (the reference game's ranged enemies)
- A ranged enemy's direct fire became a pattern doc played by a permanent `PatternPlayer` (inserted at
  spawn with `started = VOLLEY_UNFIRED` (`u64::MAX`, the engine's "not playing yet" — `started: 0` fires
  on frame 0); the AI re-stamps `started` and `target`, and `play_patterns` stands the volley the same
  frame). A game-side system adopts each fresh volley, copies the shooter's damage onto a game
  component, then `volley_hits` against the player: the first hit `kill`s the index and lands the damage
  once; `advance_volleys` follows and reaps. The old per-bullet entity path was deleted. A doc-kind
  cross-check in `--check-docs` covers the enemy's `pattern` field (`L1144`). The committed replays were
  re-recorded on the reference box; every golden was unchanged.
- **Rulings made while building:** the test harness rooted at the crate directory, one level above the
  host's `scenes/` root, so a content-root-relative key never resolved — every test engine now roots
  where the host does. One doc holds one speed, so two enemies with different bolt speeds author two docs.
- **Ruled (2026-09-13):** the game holds its `GameClock` in every non-`Running` phase. Why: the
  closed-form bolt aged through a pause while its hit test was phase-gated — a pause past a bolt's life
  deleted an inbound bolt and a shorter one teleported it. Clip and burst stamps moved to gameplay time
  so a clip or burst after a hold starts on time; only the committed run replay moved, so it alone
  re-recorded.
- **Owed:** the game's stress budgets carry the renamed hit-query line with the old values —
  re-measure with the joint budget pass; the M4's goldens re-proved and the `sim-hash` cross-arch
  comparison re-run.

### Wave 4 — shipped 2026-09-13 (the pattern tab)
- Engine half: `engine.describe` gained `documents: { "<kind>": <schemars schema> }` for every registered
  doc kind (sorted; `{}` when none) — the validator's own document, so the form and the engine read one
  truth; wire still 29 methods, hash unmoved. `games/vigil/scenes/effects-stage.yaml`: `subject` carries
  `PatternPlayer { pattern: "" }`, a `target` marker sits at `[3, 0.4, 0]`.
- Frontend (`editor/app/src/editor/effects/`): the Pattern tab enabled in `EmitterPanel`, sharing the
  one time strip (Restart re-stamps `PatternPlayer.started`; loop greyed — a doc carries its own);
  `PatternEditor.tsx` + `DocForm.tsx` over `docFields.ts`, a **recursive** schema walk (`$ref`,
  `additionalProperties` maps, `items` lists with add/move/remove, nullable toggles, enums as selects;
  leaves handed to `inspect-fields.ts` — vitest pins 31 leaves, none raw JSON, over the real
  `schema_for!(PatternDoc)` fixture in `patternSchema.ts`, also the bundled fallback for a host without
  the kind); picker = `list_editor_dir(<project>/scenes, yaml)` + a `kind: pattern` peek
  (`patternFiles.ts`); YAML through the `yaml` package's document API so comments and key order survive
  (`yamlDoc.ts`; block style enforced, schema numbers keep a fraction digit); every committed field =
  `write_editor_file` then `asset.reload { path: <key> }`, the reply shown verbatim; open =
  `query.entities` → `engine.describe` → `component.get` → `component.set PatternPlayer`; leaving clears
  `pattern`. Persona section rewritten; live-smoke list in `Plan/editor-live-smoke.md`; one sentence in
  `docs/patterns.md`. `check.mjs --frontend-only` green.
- **Rulings made while building (decision 10 drifted from the tree):** no schema-driven doc form
  existed to reuse, and no wire method exposed a doc kind's schema — hence `documents` on `describe`
  (a payload addition, not a method). Pattern docs live under the content root (`scenes/patterns/`),
  not a `docs/` folder. The stage viewport has no gizmo, so the aim target is moved by two typed fields
  (x/z) — a stage gizmo is a separate decision. The pure module is `docFields.ts`, not `docForm.ts` as
  first named: a sibling `DocForm.tsx` would differ from it only in case, which TypeScript's module
  resolution refuses on a case-insensitive Windows filesystem. The plain `ludion serve` stage registers
  the pattern kind: `EngineBuilder` distinguishes engine-owned kinds (`patterns()`) from a game's
  `doc_kind`, and an engine carrying only engine-owned kinds skips a foreign `kind:` with an
  **`L1155` warning** (`Engine::doc_skip_warnings()`, printed by `serve`/`sim-hash`) instead of the
  `L1119` boot failure a game builder keeps — vigil's stage boots with 28 skips and plays its patterns. A scene that cannot be
  read is `L1015` before the engine is built (the walk would otherwise sweep a root nobody named).
- **Exit (the human gate) still open:** the external shmup's first boss pattern authored in the tab on its
  local path dependency; the vigil ranged-fire look at the sitting. The Effects/pattern live smoke rides the
  next Tauri sitting.
