# Audit: skeletal animation and animation clips (2026-10-03)

Report only; no code was changed. Scope, at rev `d1da8bd5`: the glTF skin and clip import
(`crates/ludion-import/src/{skin,animation}.rs`, the `bind_skins`/`emitted_nodes` parts of `lib.rs`);
the clip document, its sampler and validation (`crates/ludion-scene/src/{animation,clip_eval,character}.rs`,
`check_character_clips`, the mesh `joints:` checks); the engine side, `crates/ludion-engine/src/{character,animation_events,skin_bone}.rs`,
`Animation`/`crossfade` in `components.rs`, `animation_pose`/`character_pose`/`fade_weight`/`sampled_palette`/`posed_palette`
in `extract.rs`, the `render --clip/--crossfade` sampling in `model_sheet.rs`, and the `AnimationLibrary`
install in `engine.rs`; the renderer's palette upload (`crates/ludion-renderer/src/render.rs`) and
`shaders/snippets/skin.wgsl` with its `mesh.wgsl` caller; `crates/ludion-cli/src/clip_report.rs`; the
editor's clip strip and Characters reader (`editor/app/src/editor/{voxel/clipPlayback.ts,voxel/anim.ts,mesh/character.ts}`);
the driving content (`games/vigil/src/animation.rs`, `crates/ludion-testkit/tests/common/mod.rs`); and the docs
`docs/{character-animation,voxel-animation}.md` and `Plan/{skeletal-animation,animation-clips,character-controller}.md`.
The `SkinBone` spawn-placement maths is already covered by hierarchy H1/L2 and is not repeated here.
Two findings were confirmed by replaying the exact IEEE-754 double arithmetic of `crossings` and
`elapsed_seconds` in a throwaway script outside the repo. No repository test was run.

## Overall assessment

The design is sound. Pose is a pure function of `(clip, gameplay tick)`, evaluated render-side and
never hashed. `Animation` is plain hashed sim state, counted in integer frames, so snapshot, restore
and replay need nothing special. Every transcendental in sampling, slerp and import goes through libm
`f64`. The importer handles STEP, LINEAR and CUBICSPLINE correctly, including tangents scaled by
`dt` and a final normalize. The weak spot is the event clock, in three ways. An event at a loop's
`t = 0` is dropped on some wraps because of float rounding. Any clip started by a system registered
after `animation_events` never fires its `t = 0` events, and that ordering is the documented
react-on-`end` pattern. Changing `speed` mid-clip rescales all elapsed time, so the pose jumps and
events fire twice or not at all. No shipped content triggers any of the three yet: there are no
`t = 0` events and no mid-clip speed changes. The platformer this arc was built for will hit all
three.

## High

None.

## Medium

### A1 — A looping clip's event at `t = 0` is silently dropped on some wraps · Medium · [verified]

**Evidence.** `crates/ludion-engine/src/animation_events.rs:342-352`: the loop branch walks cycles
`floor(prev / d) ..= floor(now / d)`, then tests `event.t + cycle as f64 * d` against the window
`(prev, now]`. Those two computations can disagree. `fl(k·d) <= now` does not imply
`floor(now / d) >= k`, because `now / d` can round to just below `k`. When that happens, cycle `k` is
not visited on the tick its offset is reached. On the next tick that offset is no longer `> prev`.
Replaying the exact arithmetic for a 60 Hz loop of `duration: 1.1` with an event at `t: 0`: at tick
990, `now = 16.5`, `fl(15 × 1.1) = 16.5`, and `16.5 / 1.1 = 14.999999999999998`, so cycle 15 is
skipped. Cycle 15's `t = 0` event is lost, and so are others. At speed 1 the same happens for
durations 0.4, 0.8 and 1.3 (first misses at cycles 43, 43 and 7), and 613 of 3,594 sampled
(duration, speed, event) cases lose at least one wrap. Events at mid-cycle times (`d/2`) were never
lost, and nothing ever fired twice.

**Why it matters.** `t = 0` is exactly where the validator sends authors: a loop event at
`t == duration` is refused with the help text "a loop's duration is its 0: move the event to `t: 0`"
(`crates/ludion-scene/src/animation.rs:1024`). A footstep or a fire-on-cycle gameplay hook then
skips a beat now and then. The drop is deterministic, so it replays identically, but it is a real
correctness gap that no test catches.

**Failure scenario.** A platformer's `run` loop (1.1 s) carries `footstep` at `t: 0`. After 16.5 s
of running, one footstep is silent. A gameplay system that counts strides drifts by one.

**Fix.** Treat the window test as the authority and widen the cycle range by one on each side
(`first - 1 ..= last + 1`), or count cycles in integers from ticks. Add a test that sweeps durations
in 5 ms steps over a few thousand ticks and checks that each `t = 0` event fires exactly once per
cycle.

### A2 — `t = 0` events never fire for a clip started by a system that runs after `animation_events`, which is the documented react-on-`end` pattern · Medium · [verified]

**Evidence.** The window is `[0, 0]` only when `ticks == started` (`animation_events.rs:288-296`).
On every later tick it is `(T(n−1), T(n)]` with `T(started) = 0`, so `t = 0` is excluded. A system
that both reads `AnimationEvents` and writes `Animation` has to run after `animation_events`, and
then the tick where `ticks == started` has already passed when the system sees the new clip:

- the canonical example registers `animation_events` first, then `cycle_character` calls
  `crossfade(..., ctx.game_frame)` (`crates/ludion-testkit/tests/common/mod.rs:69-76`);
- vigil registers `animation_events` before `animate_player`, which writes `walk` with
  `started = ctx.game_frame` (`games/vigil/src/lib.rs:1047-1052`, `games/vigil/src/animation.rs:91-95`).

The module docs (`animation_events.rs:13-14`) promise that "an event at `t = 0` fires the tick the
clip starts", and `docs/character-animation.md` tells games to transition on `end`. The two rules
cannot both hold for the clip a transition enters.

**Why it matters.** This silently breaks the documented contract for the main way games drive clips.
Neither shipped character nor vigil has a `t = 0` event today, so nothing is visibly broken yet.

**Failure scenario.** `attack` → `recover` on `end`, and `recover` carries `t: 0 name: vulnerable`.
The `vulnerable` window never opens, every time, with no diagnostic.

**Fix.** There are two options:

- Let the system record, per entity, the `(clip, started)` it last walked. Store this in a
  snapshotted sim resource; it is sim input anyway. On the first walk of a new stamp, use `[0, T]`.
- Move the start rule to "the first tick at or after `started`" with a closed lower bound, and
  document a one-tick latency.

At minimum, document the restriction in `docs/character-animation.md` and add a cycle test with a
`t = 0` event on a clip entered by a transition.

### A3 — Changing `Animation.speed` mid-clip rescales all elapsed time, so the pose jumps and events fire twice or are skipped · Medium · [verified]

**Evidence.** Clip time is `(now − started) × spt × speed × binding.speed`, evaluated from scratch
each frame (`extract.rs:3072-3075`, `character_pose`, `animation_events.rs:289-296`). Nothing
re-bases `started` when `speed` changes. Worked through the formula: a 1 s loop with `step` at 0.5,
started at tick 0, speed 1. At tick 59, T = 0.983. At tick 60 the game writes `speed = 0.5`, so
`prev = 59/60 × 0.5 = 0.4917` and `now = 0.5`. `step` fires a second time in this cycle, and the
pose jumps back from phase 0.983 to 0.5. Raising the speed instead skips every event between the
old and the new phase. `crossfade` has the same flaw on the outgoing side through `from_speed`.

**Why it matters.** `Plan/animation-clips.md` decision 4 lists "speed" as one of the playback
primitives games drive. Scaling a run cycle by ground speed, the usual platformer locomotion trick,
writes `speed` every frame, which makes the pose stutter and the footsteps fire chaotically. Neither
the `Animation.speed` doc nor `docs/character-animation.md` warns about this.

**Failure scenario.** `Plan/character-controller.md`'s hero sets `animation.speed = velocity / 4.0`
each tick. The run cycle jitters back and forth, and footstep events spam or go missing.

**Fix.** Provide a `set_speed(&mut Animation, speed, now)` that re-bases `started` (or stores an
accumulated-phase anchor in integer ticks plus the anchor's clip time) so the phase stays continuous.
Make it the one documented way to change speed, the way `crossfade` is for clips. Until then,
document that `speed` may only change together with a restamp.

## Low

### A4 — Skinned normals are multiplied by the bone matrix, not its inverse-transpose, so they skew under non-uniform joint scale · Low · [verified]

**Evidence.** `crates/ludion-renderer/src/shaders/mesh.wgsl:221-224` computes
`normal = (skin * vec4(normal, 0)).xyz` with the blended palette matrix. The per-instance path
deliberately uses the inverse-transpose for non-uniform scale (`extract.rs` `normal_matrix`), but
the skin path does not. Scale tracks ship (decision 5), and the mannequin `jump` squashes the pelvis
non-uniformly (`Plan/animation-clips.md` W1 notes).

**Why it matters.** During a squash or stretch, sloped surfaces shade as if tilted toward the
stretched axis. This is visual only, and many engines accept it, but it is undocumented and
inconsistent with the unskinned path.

**Fix.** Either document it as accepted (most rigs scale uniformly), or give the palette a parallel
normal-matrix entry per joint, or apply the cofactor in the shader when a scale track is present.

### A5 — A hand-authored rotation segment of 180° or more silently takes the short way, so a one-segment 0→360 spin does nothing · Low · [verified]

**Evidence.** `clip_eval.rs:140-160` converts each key's Euler triple to a quaternion and slerps
along the shortest path. This is documented (`docs/voxel-animation.md:119`, and the test
`rotation_takes_the_short_path`). Since W2 it also applies to hand-authored voxel clips, which
earlier lerped Euler axes. `validate` (`animation.rs`) has no check on segment size. A scan of every
committed `*.anim.yaml` (293 rotation segments) found a largest per-axis delta of 105°, so no shipped
clip is affected.

**Why it matters.** Euler text reads as winding (`0 → 360`), but the runtime ignores it. An author or
agent who keys a full turn as one segment gets a stationary part and no diagnostic. That is the
Pillar 5 gap.

**Fix.** In `validate`, report `L1142` with a suggestion for a rotation segment whose Euler delta on
any axis is 180° or more: "split the segment; rotation takes the shortest path". The importer's
unwrapped output never trips it.

### A6 — `docs/voxel-animation.md` stamps `started` with `ctx.frame`, contradicting the gameplay-frame rule · Low · [verified]

**Evidence.** `docs/voxel-animation.md:186` (and its embedded mirror) writes
`animation.started = ctx.frame;`. `docs/sprite-animation.md:95-97` and
`docs/character-animation.md:86` say "`ctx.game_frame`, never `ctx.frame`". Pose and events both
read gameplay time (`extract_at`, `animation_events` via `ctx.game_time`). This is the clip twin of
particles K2.

**Why it matters.** Code copied from this doc into a game with a held `GameClock` stamps a frame in
the gameplay future. The clip then freezes on its first pose, and its events wait, for as many ticks
as the game has held.

**Fix.** Change the example to `ctx.game_frame` and re-mirror `embedded-docs/`.

### A7 — Per-frame waste in palette sampling · Low · [verified]

**Evidence.** `extract.rs:1131-1146` / `sampled_palette` (`:2346-2380`) sample every skinned
instance every frame, with no visibility test. Per joint, `joint_pose` scans the track list by string
twice (`clip_eval.rs:74`, and again inside `part_pose`). It finds the key with a linear `rposition`
(`:119`, `:151`) and converts both bracketing Euler keys to quaternions (12 libm trig calls) before a
slerp (acos plus 3 sines). Each instance also allocates a `posed_model` `Vec` (`:2352`; likewise
`posed_palette` at `:2323`).

**Why it matters.** With one 11-joint mannequin the cost is negligible. A level with dozens of
characters on 50+ joint rigs pays for every off-screen character.

**Fix.** Resolve track indices per (clip, skin) once, at character load. Pre-convert keys to
quaternions in the loaded clip. Binary-search key times. Reuse one scratch `Vec` across instances.
Skip instances whose bounds the camera culls, once the renderer exposes culling to extract.

### A8 — Under `--allow-lossy`, a non-joint node between two joints is dropped, and its transform is silently lost from every descendant's rest · Low · [verified by reading]

**Evidence.** `crates/ludion-import/src/skin.rs:76-92` parents a joint to its nearest **joint**
ancestor, skipping non-joint nodes. The joint's rest is still its own node-local TRS
(`:186-190`), so an intermediate node's transform disappears from the chain. `emitted_nodes`
(`lib.rs:590-600`) reports this only as `L1202` "a node parented to a skin joint". That is a hard
error by default, but under `--allow-lossy` it is a warning that says only that the node is
dropped, not that the joints below it have the wrong rest.

**Why it matters.** Blender never exports this shape, but other DCC exporters can (helper or
"socket" nodes inside a rig). The skinned rest pose then deforms with no measurable error report.

**Fix.** Fold an intermediate node's local transform into the child joint's rest, or name the
consequence in the `L1202` text ("joints under it lose its transform").

### A9 — CUBICSPLINE segments are measured at only four sub-samples, so the error bound can understate deviation between them · Low · [suspected]

**Evidence.** `animation.rs:488-506` subdivides each cubic interval into 4 frames. `measure`
checks the bound only at those frames, and reduction and runtime interpolate linearly or by slerp
between them. A cubic segment with large tangents can bulge past `bound` between sub-samples.

**Why it matters.** The importer reports `worst rotation error` as a guarantee. Blender's exporter
writes LINEAR by default, so this only bites on cubic exports from other tools.

**Fix.** Choose the subdivision count from tangent magnitude, or take the max over a denser
measurement grid than the candidate-key grid.

## Checked and healthy

- **Determinism of sampling.** `clip_phase`, `sample_channel` and `sample_rotation`, the scene
  `slerp` (`acos`/`sin`/`sqrt` from `ludion_core::math::f64`), the importer's own slerp and Euler
  conversion, and `elapsed_seconds` (integer ticks × `1/hz` × widened `f32` speed) are all
  IEEE-exact or libm. No glam `slerp`/`normalize` on any path that feeds hashed state; ragdoll's
  sim-side use of `animation_pose` hand-normalizes and belongs to the physics audit.
- **Pose is presentation, playback is hashed.** Bones are evaluated in `extract_at` at gameplay time.
  `Animation` (with crossfade fields, `skip_serializing_if` at defaults) is ordinary snapshotted
  state, with no accumulated float time. `AnimationEvents` is rewritten every tick, and
  `AnimationLibrary` is reinstalled on every load, restore and switch (`engine.rs:1729`). Held
  gameplay frames clear the events.
- **Event walk outside A1–A3.** The windows tile `(T(n−1), T(n)]` from identical expressions on
  consecutive ticks, so no double fires were found in the sweep. A multi-cycle tick fires each
  crossing. A one-shot fires `end` once, after same-instant authored events. Speed 0 and a future
  `started` fire nothing. Only the incoming side of a crossfade emits. Order is ascending entity id.
  `L1210` is latched per entity.
- **glTF sampler semantics.** STEP holds `value(i)`. LINEAR rotation slerps with a sign flip.
  CUBICSPLINE reads `(in, value, out)` triples at `3i+1`, `3i+2` and `3(i+1)`, scales tangents by
  `dt`, and normalizes rotation after the Hermite. Times must strictly increase and be finite, and are
  snapped to the shortest `f32` decimal. Zero-length quaternions, non-finite outputs and count
  mismatches are `L1201`, and zero-length animations are `L1202`.
- **Euler storage.** Keys unwrap to the nearest equivalent triple (`nearest_equivalent_angles`).
  Reduction measures with the runtime sampler. The bound is measured in model space with a
  Gram-Schmidt rotation that survives sheared parents. Over-bound clips are `L1206`, and clips at
  rest are `L1208`.
- **Skins.** Inverse binds are carried verbatim, and reframed roots use
  `inverse(meshWorld) × jointWorld`, matching the glTF joint-matrix definition. Joints are reordered
  parent-first, and cycles and duplicates are refused. The 256-joint cap matches between importer
  and validator. Weights are renormalized, and weightless vertices are bound to the root.
  Out-of-range slots are refused. The palette upload prepends identity, so `bone_offset == 0` never
  reads it.
- **Crossfade.** `seconds <= 0` or non-finite is a hard cut that clears every fade field. A side with
  no track for a channel reads rest, and a joint with no track on either side keeps `rest_local`
  exactly. Weight is integer-tick derived, and the plain incoming pose returns at `weight >= 1`.
  Crossfading to the same clip restarts it and blends from the old phase, which is coherent.
- **Validation UX.** Durations, key ranges and ordering, scale positivity, event ranges (including
  the `t == duration` loop rule), reserved `end`, overlay-vs-own events, unknown joints and unskinned
  meshes all carry `L1142`/`L1198`/`L1199`/`L1200`, a JSON Pointer and help text.
- **Editor preview.** It writes only `component.set`/`remove` of `Animation` plus `sim.*`. Its TS
  readers are view-only (the parity audit covers the host).
- **Clip drift gate.** Gallery clips are byte-compared against a fresh import.

## Suggested order of work

1. **A1.** A one-line widening of the cycle range plus a sweep test. Cheap, and it removes a silent
   determinism-adjacent gap.
2. **A2 and A3 together.** Both are about how the event and pose clocks react to a write. Choose the
   start-tick rule and the speed re-base API in one design pass, ideally before
   `Plan/character-controller.md` starts driving locomotion speed.
3. **A6.** A one-line doc fix plus a re-mirror.
4. **A5.** A validator rule with a suggestion.
5. **A4, A7, A8 and A9** as the platformer's content demands them: A7 once a scene holds many
   characters, A4 if the squash shading is noticed at the human sitting.
