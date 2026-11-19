# Plan: Voxel Animation — rigid-part assemblies + declarative clips

> Design settled 2026-08-17 (grilling session, all questions ruled by the user). This doc is the
> decision record; the wave breakdown is at the end and the shipping log is the Status section below.

## Status

**All five waves are SHIPPED — the arc is MECHANIZED-COMPLETE** (2026-08-18 → 2026-08-20, Lore revs
600–605 and 612–620). Wave 4b (rev 620): the player entity in the four shared `*-core.yaml` fragments
rides `Mesh.shape: { assembly }` at `scale: 0.125` (4.0 u — the height ruling) with `Animation: {}`;
`games/vigil/src/animation.rs` owns every write (`animate_player` before `drive_hud`, `trigger_attack`
from `fire`, attack held `ceil(0.35 × hz)` ticks); `run.lrp` re-recorded, `stress.lrp` byte-identical,
`district-plaza` re-blessed. Wave 5 (rev 620): `docs/voxel-assemblies.md`, `docs/voxel-animation.md`,
`dev-docs/assembly-animation.md`, persona §12. The arc closes on the human sitting (player look at
0.125, `LANTERN_HEIGHT`, the attack-latch under the 0.3 s crossbow cooldown, hinge read). The
deferred list further down is unchanged — nothing was pulled forward and nothing new was deferred
except the one item named below.

| wave | shipped |
| --- | --- |
| 1 — formats + loader | `kind: part` (`L1139`/`L1140`), `kind: animation` (`L1141`/`L1142`), `kind: assembly` (`L1137`/`L1138`) in ludion-scene; per-part greedy meshing + the pinned rest-pose contract in `ludion_engine::assembly_mesh`; `check-scene` dispatches all three (`ENGINE_KINDS` 5 → 8). `L1136` was not taken (reserved by `Plan/audio-ambience.md`). |
| 2a — rest-pose runtime | `Mesh.shape: assembly`; asset-cache multi-source dependency edges (reloading any part re-meshes the assembly); extract expands one entity into one `MeshInstance` per part via `resolve_assembly` + `part_placements`. |
| 2b — playback | `Animation { clip, started, speed }` registered 22nd; clip loading + validation through the assembly; the pure sampler `ludion_scene::clip_eval`; posed `part_placements` composed down the parent chain; `extract_at(world, viewport, root, now)`. Proof: `crates/ludion-engine/tests/assembly_animation.rs` — an animated run's `WorldHash` trace and `.lrp` are byte-identical to one that never extracts. |
| 3a — assembled sheets | `render --model x.assembly.yaml` at rest, composing with `--views`/`--rig`/lineups/`--true-size`; gallery `figure.assembly.yaml` + `figure-sheet.png`. |
| 3b — analysis over the assembled grid | `assembly_assembled_grid` → `AssembledGrid { voxel, owners }`, so exposure / mirror-x / connectivity / slice / column run unchanged over a composed assembly with part attribution; then `L1143`, the joint-plug lint (silent iff parent↔child within the plug radius of the child's pivot). |
| 3c — posed sheets | `--clip <name> [--time <s>] [--strip <n>]` through the one evaluator (the sheet's entity carries a real `Animation`), clip time quantized to 60 Hz ticks, per-phase gap analysis over a conservatively rasterized posed grid; gallery `figure-wave-strip.png`. |
| 4a — content | `games/vigil/scenes/voxels/player.assembly.yaml` — the Lamplighter articulated, 11 parts / 7 files, 13×32×9, with `humanoid-walk` (0.6 s loop) and `player-attack` (0.35 s one-shot). The humanoid part convention it fixes is now `ludion docs voxel-assemblies`. |

Two implementation rulings were taken against this design and are recorded in place below: block-style
keyframes only (the sketch's flow style is `L1005`), and the `Animation` field types.

**One deferral, trigger fired and closed:** `render --model --set`/`--sweep` refused an assembly with
`L4008` (the override path staged one re-emitted file; an assembly needed itself plus every part
re-staged). Value calibration on the player character fired the trigger — it was the largest
remaining assembly-authoring cost. Closed by `Plan/assembly-set-sweep.md`.

**Reference box, 2026-09-07:** both gallery goldens (`figure-sheet.png`, `figure-wave-strip.png`)
provenance re-blessed on the RTX 3090 (the laptop bytes read SSIM 0.99978 / 0.99923 there) and the
workspace `check.mjs` lane run on the pinned box; the M4 blessed its pair at rev 637.

## Goal

Animated voxel models an LLM can author, verify, and animate end to end: a multi-part rigid
assembly format (`kind: assembly` + `kind: part`), declarative keyframe clips
(`kind: animation`), deterministic engine-side playback that is **visual-only** (sim, `WorldHash`,
and replays untouched by construction), and assembled/posed tooling shipped in the same arc so
agents are never authoring blind. Driving content: a new HD multi-part **player character** for
vigil with `walk` + `attack` clips.

Scored against the North Star: an agent animating a model holds only the ~30-line assembly doc in
context (part names, pivots, hierarchy) — never the voxel data; an agent editing a limb holds one
small part file plus the assembled sheet as an image; a clip is judged as a deterministic PNG
phase strip plus text-form joint/gap analysis, no game run needed.

## Settled context this plan builds on (not re-litigated)

- Voxels are a producer of meshes; greedy mesher, bottom-center origin, 1 voxel = 1 unit; the
  `.voxel.yaml` format is fixed and **stays fully valid forever** — no forced migration, enemies
  convert later as ordinary content work.
- Roadmap composition direction (2026-07-15): big props = assemblies of ≤48³ parts,
  `kind: assembly` trigger-gated on "first reusable multi-part prop". **The trigger has fired**
  (the animated player character); this plan is that reserved format.
- `Parent` is a link only — no transform propagation exists, and this plan builds none.
- Renderer batches instanced draws per mesh asset: a horde of 500 five-part statues costs 5 part
  draws per pass, not 2500. Performance does not constrain the design.
- The model-view harness (`render --model`, sheets, `--rig`, lineups) and the check-scene analysis
  family (`--exposure`, `--mirror-x`, `--slice`, `--column`, L1129) are the verification substrate
  this arc extends to assemblies and poses.

## One recorded reversal

`Plan/telegraphs-and-vfx.md` recorded "animation is game-side via per-frame component writes …
zero animation logic in `ludion-renderer`/`ludion-engine`". That stance was written for
hit-flashes and scale pulses and is **deliberately overturned** for character animation (user
ruling 2026-08-17): clips are declarative data played by a deterministic engine-side evaluator.
Game-side procedural writes remain legal and shipped (yaw-to-heading, pickup bob, fragments).

## Decisions (all ruled 2026-08-17)

1. **Paradigm: rigid parts + transform clips.** No skinning (rejected — hostile to text authoring;
   a future skeletal arc for imported meshes is a separate matter). No frame-swap flipbook
   (rejected for token cost; not kept as a v1 mechanism).
2. **Visual-only, hard line.** The sim keeps its single collision footprint and states. Pose is a
   pure function of (clip, sim time); pause freezes it, replays are byte-identical for free.
   Per-limb hitboxes / foot placement: trigger-gated later.
3. **Storage: files-only assembly-of-parts.** A `kind: assembly` doc is the model of record; every
   part is its own file, always (no inline parts — one authoring shape, one path through every
   tool). Global vision comes from tooling, not from one mega-file.
4. **The assembly owns the one palette.** Parts are lean slices-only docs whose glyphs resolve
   against the owning assembly's palette (unknown glyph = lint at assembly check). Enforces
   whole-model value structure by construction; re-skin = new assembly over the same part files.
5. **Runtime: one entity, expansion at extract.** `Mesh.shape` gains `assembly:`. The entity keeps
   one `Transform`, one `Mesh`; extract expands into N `MeshInstance`s with part matrix = entity
   transform × animated part-local transform. No part entities, no transform propagation, no ECS
   or scene-format change. Parts don't exist sim-side, so nothing *can* couple to them.
6. **Clip selection is explicit game code.** A small engine component —
   `Animation { clip, started, speed }` — ordinary hashed sim state, written by game systems
   (movement sets `walk`, combat sets `attack`). No data-driven state machine (a behavior DSL —
   pillar violation). Transitions are hard cuts in v1.
7. **Clips are separate files bound by short name.** `kind: animation`, one clip per file; the
   assembly declares `clips: { walk: anims/humanoid-walk.anim.yaml, … }`; the component uses the
   short name so game code is model-agnostic and clips are reusable across models sharing the
   part-naming convention.
8. **Clip schema v1**: `duration` in seconds (fixed-step sim makes this exact); `loop` bool
   (one-shots hold the final pose); per-part-name `tracks` with `rotation` (Euler XYZ degrees,
   same convention as `Transform`) and `translation` (part-local voxel units, floats allowed)
   keys `{ t, v, ease? }`; easing set `linear` (default) | `smooth` (smoothstep) | `step`;
   per-axis lerp between keys; all rotation about the part's pivot; evaluation through
   `ludion_core::math` only. **No scale channel** (user ruling), no visibility channel.
9. **Assembly schema v1**: `size` (assembled bounding grid, rest pose), the palette, `parts` list
   — `name`, `file`, `place` (integer offset into the **assembly-global** grid; preserves the
   author-in-one-grid mental model; loader derives parent-relative transforms internally),
   `parent` (exactly one root; cycles/unknowns are errors), `pivot` (part-local voxel coords,
   floats allowed — halves reach even-width limb centers), `mirror: x` (one limb file serves both
   sides) — and the `clips` map. The rest pose IS the assembled grid: what sheets show, what
   analysis runs over, what renders when no clip plays.
10. **Joint overlap is legal and expected** (the joint-plug technique: limb geometry
    interpenetrates the socket so rotation never opens a gap). Overlapping cells count as filled
    for grid-level analysis, with per-part attribution. A warning-class lint (L1028/L1029-style
    heuristic, never an error) classifies overlap clusters using the hierarchy + pivots:
    parent↔child overlap near the child's pivot (radius scaled to the child's cross-section) is a
    silent joint plug; overlap between unrelated parts, or parent↔child overlap far from the
    declared pivot (usually a wrong *pivot* — the more valuable catch), warns with pair, cluster
    bbox, cell count, and pivot distance. Reuses the connected-region machinery from `--mirror-x`
    / L1129.
11. **Tooling is a launch requirement of the arc, not follow-up.** `render --model
    x.assembly.yaml` produces assembled rest-pose contact sheets (composes with `--rig`,
    lineups, `--true-size`); `--exposure`, `--mirror-x`, connectivity, and the overlap lint run
    over the assembled grid; `--clip <name> --time <t>` poses a sheet at an instant;
    `--strip <n>` renders one PNG of n evenly-spaced phases (the wave-C lineup composition); the
    joint gap/overlap analysis is runnable per strip phase so "elbow opens at t=0.4" is text.
    **No mp4 export in v1**; no wire methods, no editor surface (file is the editor, the voxel
    precedent).
12. **Driving content**: a new HD multi-part player character — the first assembly — with `walk`
    (looping, wired into vigil movement) and `attack` (one-shot, wired into fire/melee). Its part
    naming (`torso`, `head`, `upper-arm-l`, …) becomes the documented humanoid convention for
    clip reuse. No enemy retrofits this arc; horde cost gates through E6 when enemies convert.

### Reference sketches (ratified verbatim in the session)

```yaml
# player.assembly.yaml
format: 1
kind: assembly
name: player
size: [12, 24, 8]
palette:
  S: { color: [0.30, 0.22, 0.18, 1.0] }
parts:
  - name: torso
    file: parts/torso.part.yaml
    place: [4, 10, 2]
  - name: upper-arm-l
    file: parts/arm.part.yaml
    parent: torso
    place: [1, 14, 3]
    pivot: [1.0, 5.0, 1.0]
  - name: upper-arm-r
    file: parts/arm.part.yaml
    parent: torso
    place: [9, 14, 3]
    pivot: [1.0, 5.0, 1.0]
    mirror: x
clips:
  walk: anims/humanoid-walk.anim.yaml
  attack: anims/player-slash.anim.yaml
```

```yaml
# anims/humanoid-walk.anim.yaml
format: 1
kind: animation
name: humanoid-walk
duration: 0.6
loop: true
tracks:
  upper-leg-l:
    rotation:
      - { t: 0.0, v: [25, 0, 0] }
      - { t: 0.3, v: [-25, 0, 0], ease: smooth }
      - { t: 0.6, v: [25, 0, 0] }
  torso:
    translation:
      - { t: 0.0, v: [0, 0, 0] }
      - { t: 0.3, v: [0, 0.5, 0] }
```

> Wave 1 implementation note (2026-08-18): the clip sketch's flow-style keyframes
> (`{ t: 0.0, v: [25, 0, 0] }`) do not parse — the strict-YAML subset rejects non-empty flow
> collections (L1005), a rule this arc does not relax. The canonical and only accepted keyframe
> spelling is block style (one field per line, `v` as a block sequence); the shipped
> `kind: animation` module documents it and its canonical golden pins the block-form translation
> of the humanoid-walk sketch.

## Deferred, with triggers

- **Crossfade blending** — first sitting where a hard cut reads as a pop.
- **Sockets / attachments + game-callable pose evaluation** (weapon-in-hand: the engine's clip
  evaluator exposed as a pure deterministic function game code may call) — first content that
  needs an object to follow a limb. **Fired by Clockout 0028 on skins:** `ParentJoint` attaches an
  entity to a joint of a skinned parent, the draw follows the posed joint, `posed_joint_world` is the
  presentation-only pose read, and the `character-socket` gallery golden proves it.
- **Prefab parts as socket targets** (imported prefab parts) — a consumer asks. Assembly parts shipped
  as sockets (`Plan/animation-layers.md` decisions 41–44; owner ruling owed on lifting this deferral).
- **Scale channel** (squash-and-stretch) — a sitting asks for it.
- **Part visibility keyframes** — first content that needs them.
- **mp4 clip preview** — first clip that looked right as a strip but reads wrong in motion
  (strips show poses, not velocity; timing/weight is the likely gap).
- **Enemy conversions** (statue et al.) — ordinary content work, per model, when wanted; E6 gates
  the horde-scale evaluation cost then.
- **Data-driven animation state machine** — rejected outright, not deferred (pillar violation).
- **Skeletal skinning for imported meshes** — a separate future arc; not voxel animation.

> Wave 2 implementation ruling (2026-08-19): the `Animation` component's field types, left open at
> design time, are settled as `clip: String`, `started: FrameIndex`, `speed: f32`. String because
> components are plain serde data with String precedent throughout (`Sprite.texture`,
> `MeshSource::Asset`) and an interner would be global mutable state shared across sim and extract.
> `FrameIndex` because it is an exact integer (derives `Eq`/`Hash`, canonical YAML integer, no
> float in hashed sim state), while `SimTime` embeds a `TickRate` — a world-level property that
> would be denormalized into every component instance. Elapsed clip seconds derive on demand at
> extract (`(now_ticks − started) × seconds_per_tick`), never accumulated — the `SimTime::seconds`
> discipline.

## Draft wave breakdown (not yet scheduled)

1. **Formats + loader**: `kind: part` / `kind: assembly` / `kind: animation` in ludion-scene
   (strict keys, canonical writer, L-codes for unknown glyph/part, cycle, missing root, clip
   binding); assembly meshing in ludion-engine (per-part greedy mesh, rest-pose composition);
   conformance fixtures.
2. **Runtime**: `Mesh.shape: assembly`, `Animation` component, deterministic clip evaluator in
   extract; unit tests that armed/animated runs leave `WorldHash` sequences and `.lrp` streams
   byte-identical to the same sim without the visual layer.
3. **Tooling**: assembled sheets, analysis over the assembled grid, the overlap/joint lint,
   `--clip/--time/--strip`, per-phase gap analysis; gallery golden (rest sheet + one strip).
4. **Content + wiring**: the player assembly + `walk`/`attack` clips; vigil movement/combat set
   clips; humanoid naming convention documented.
5. **Docs**: assembly/animation format + authoring topics (joint-plug paragraph, pivot craft),
   engine-guide section, voxel-artist persona update, embedded mirror re-sync.
