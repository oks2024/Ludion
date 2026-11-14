# Plan: Animation clips — skeletal clips from Blender, crossfades and events

> Status: **W1–W3 mechanized (2026-09-30); the W3 sitting is owed.** Design settled 2026-09-29 (grilling session; every ruling is the user's).
> The driver is the next test game, a 3D platformer in the style of Crash Bandicoot, which gets its
> own plan (character controller, camera: `Plan/character-controller.md`). This plan is the animation system only. It builds on
> `Plan/skeletal-animation.md` (skins, the bone palette, `SkinBone`) and `Plan/voxel-animation.md`
> (`kind: animation`, `Animation`, `clip_eval`, posed strips). Every claim of current-code fact below
> was read from the tree on 2026-09-29, so re-grep before editing.

## Status

### W1

Format, glTF animation import, the `check-scene` clip report, and the mannequin fixture.

| file | role |
|---|---|
| `crates/ludion-import/tests/fixtures/skin-mannequin-clips.py` | the source (a sibling of `skin-mannequin.py`, which it reuses through `SKIN_MANNEQUIN_BUILD_ONLY`) |
| `crates/ludion-import/tests/fixtures/skin-mannequin-clips.glb` | derived, committed (Blender exports are not byte-stable across processes) |
| `examples/gallery/clips/{idle,run,jump,spin}.anim.yaml` | derived, committed, drift-gated by `the_committed_gallery_mannequin_clips_match_a_fresh_import` |

Codes: `L1198` `ANIMATION_EVENT_INVALID`, `L1206` `IMPORT_CLIP_ERROR_BOUND`,
`L1207` `IMPORT_CLIP_EXTRAS_INVALID`. The gallery clips' mesh document is identical to a fresh import of the
clips glb's, so the existing gallery `skin-mannequin.mesh.yaml` can play them.

The importer also drops channels constant at rest within the reduce tolerance (decision 12), so a
per-bone Blender export never yields rest tracks. Two defects found by the fixture and fixed in W1: the model-space compare decomposed sheared matrices (a
non-uniform parent scale, the jump's squash and stretch) into non-unit quaternions, so identical poses
measured up to 23 degrees apart (`orthonormal_rotation`, with a regression test); and `emit` wrote tracks
in stored order against its documented sorted-by-target order, which made every imported clip read as stale.

Measured (30 fps, keys after reduction of the per-frame bake, summed over the animated tracks; only
the bones and channels a clip moves are keyed, so the exporter writes no rest tracks):

| clip | bytes | lines | tracks | rotation | translation | scale | worst rotation error | worst translation | seam (rot / trans) |
|---|---|---|---|---|---|---|---|---|---|
| idle | 3838 | 187 | 5 | 28 of 244 | 6 of 61 | 0 | 0.1745 deg (head) | 0.65 mm (lower_arm_l) | 2.4e-6 deg / ~0 |
| run | 20442 | 848 | 8 | 144 of 175 | 21 of 25 | 0 | ~0 | ~0 | 1.7e-6 deg / ~0 |
| jump | 9948 | 439 | 3 | 44 of 50 | 22 of 25 | 19 of 25 | ~0 | ~0 | n/a (one-shot) |
| spin | 4554 | 203 | 3 | 38 of 39 | 0 | 0 | ~0 (pelvis 360 deg kept 12 of 13 keys) | ~0 | n/a (one-shot) |

Against decision 9: four clips of 4 to 20 KB (about 39 KB together) for an 11-joint rig. Reduction
barely bites on run, jump and spin because the source is a per-frame bake of eased curves; idle, the
small-amplitude clip, reduces most. Bound margins are wide (worst 0.17 deg against 0.5 deg).

### W2

Runtime, strips and the gallery scene.

- **Characters:** `kind: character` (`crates/ludion-scene/src/character.rs`, loader
  `crates/ludion-engine/src/character.rs`), drawn by `Mesh.shape: { character }` and sharing one upload with
  a plain asset of the same mesh; asset-cache edges re-read it on a character, mesh or clip edit. Overlay
  events on a clip that has its own and a track naming a joint the skin lacks are refused at load.
- **Sampling:** clips pose skins in `extract_at` (`SkinBone` > clip > rest); rotation interpolates by libm
  slerp (`ludion_scene::slerp`) for skins and assemblies alike; the scale channel composes down joint and
  part chains, and the ragdoll switch ignores it. No golden, trace or replay moved.
- **Crossfade:** `Animation` gains `from`, `from_started`, `from_speed`, `fade_started`, `fade`, skipped when
  default so existing hashes hold; `crossfade(&mut Animation, clip, seconds, now)`, a non-positive fade is a
  hard cut. A mid-fade interrupt drops the older outgoing clip.
- **Events:** `animation_events` fills `AnimationEvents` per tick from an unhashed, lazily loaded
  `AnimationLibrary` (not invalidated by `asset.reload`, so replays stay stable; a retime reaches events at
  the next load, restore or switch). Only the incoming clip of a crossfade emits; `end` is reserved and
  implicit on a one-shot; a loop may not carry an event at `duration`. vigil's attack releases on `end`, on
  the tick its old hold did, so its corpus did not move. A replay root must carry the model files.
- **Tooling:** `render --model x.character.yaml --clip` and `--crossfade from:to:seconds` (both kinds);
  `check-scene` on a character reports per-clip ground-contact heights.
- **Gallery:** `examples/gallery/character-cycle.{yaml,trace.txt,lrp}` with its Windows goldens, driven by
  `crates/ludion-testkit/tests/common/mod.rs`; run it with `cargo run -p ludion-testkit --example
  character_cycle`. The fixture's jump squash was fixed in the bpy source (feet no longer sink).
- Docs: `ludion docs character-animation`. Codes: `L1199` `CHARACTER_DOCUMENT_STRUCTURE`, `L1200`
  `CHARACTER_DATA_CONSISTENCY`, `L1208` `IMPORT_CLIP_AT_REST`, `L1209` `ANIMATION_SPRITE_CROSSFADE`,
  `L1210` `ANIMATION_MODEL_UNLOADED`. The M4 bless and events twin passed 2026-09-30 (rev 996).

*Amended 2026-10-31 (owner approval 2026-10-30, Clockout 0016): rigid glTF node animation imports as prefab clips (a `clips:` map on the prefab, tracks keyed by part name), plays on a prefab instance's root part, and is read by `check-scene` (`L1314`) and `render --model <prefab>`; `ludion docs prefab-animation`.*

### W3

Editor clip preview, no new wire method (decision 18). The Voxel Artist's clip strip
(`editor/app/src/editor/voxel/clipPlayback.ts`, `useClipPlayback.ts`, `ClipStrip.tsx`) gains the
character overlay speed and a crossfade row (pick a clip and seconds; a band marks the fade on the bar;
Restart and scrub replay the blend). The Models view gets a Characters library
(`editor/app/src/editor/mesh/character.ts`) that stands a `kind: character` on the stage and mounts the
same strip. Owed: the sitting (gallery feel in the window + the preview's live smoke,
`Plan/editor-live-smoke.md`); its verdict is recorded as a decision and the Status collapses then.

### Crowd posing (Serious Isaac 0019)

Shipped: S1a r2272 (`ExtractPhase::Skin`, `SkinCounts`, palettes sampled in parallel through `ludion_core::par`); S1b r2273 (`ProfileReport.skin`, one schema move, the `.lrp` corpus re-recorded header-only); S2 r2275 (`examples/gallery/crowd-animation.yaml` with its Windows golden, `crates/ludion-engine/tests/skin_pose.rs`). Piece 1, the palette reuse cache, was built, measured and rejected (§Decisions, "Piece 1"). Bench, `cargo test -p ludion-engine --release --test it -- skin_pose --ignored --nocapture` (RTX 3090 / 5950X, release, `crowd-animation`, 500 figures, 5,500 joints): `Skin` median 0.374 ms at 1 thread, 0.315 at 8, 0.317 at the default 32. Socketed skins are posed once (Serious Isaac 0033-02): `Poses` poses each in parallel per depth group and `Skin` copies the palette; `socket_pose` bench, 500 socketed figures, `Poses`+`Skin` (RTX 3090 / 5950X, release): 1.05 ms to 0.70 at 1 thread, 1.03 to 0.75 at 8. Pinned M4 figures are owed. Pieces 3 (`lod_hold`) and 4 (pose sharing) are Proposed, not approved: see §Decisions and `Plan/gpu-driven-culling.md` §Decisions.

## Trigger

`Plan/skeletal-animation.md` decision 1 kept glTF animations refused (`L1202`) "until a game asks for
clips". The platformer asks. Its hero needs about eight clips (idle, run, jump, fall, land, spin,
slide, death), with crossfades between them, squash and stretch, and a spin whose hit window follows
the clip.

## What exists (2026-09-29)

- **Clips:** `kind: animation` (`crates/ludion-scene/src/animation.rs`). It has `duration`, `loop`
  and `tracks` keyed by **part name**. Each track has `rotation` (Euler XYZ degrees) and
  `translation` (part-local voxel units) channels of `{t, v, ease}` keys, with `ease` one of
  `linear | smooth | step`. There is no scale channel and no events. Block-style keys only: the
  strict YAML subset refuses non-empty flow collections (`L1005`), so one key is about 5 lines.
- **Binding:** a voxel `kind: assembly` binds short names to clip files in its hand-authored
  `clips:` map (`games/vigil/scenes/voxels/player.assembly.yaml`).
- **Playback:** `Animation { clip, started, speed }` (`crates/ludion-engine/src/components.rs`) is
  shared by assemblies and sprite flipbooks. Clip time is `(now − started) × seconds_per_tick`. An
  unknown clip is a silent degrade to the rest pose. The pose is evaluated in `extract_at`,
  render-side, and proven hash-neutral (`crates/ludion-engine/tests/assembly_animation.rs`).
- **Skins:** the mesh document carries `joints` (rest TRS and inverse bind), `weights` and
  `bone_indices`. The GPU palette skins every mesh pass. `extract.rs::posed_palette` builds a palette
  from `SkinBone` parts or the rest pose. `Animation` on a skin that parts name is `L1197`.
- **Import:** glTF animations are `L1202` (`crates/ludion-import`). *Amended 2026-10-31 (Clockout 0016): TRS channels on non-joint nodes that become prefab parts leave `L1202`; morph weights stay.*
- **Tooling:** `render --model x.assembly.yaml --clip <c> [--time <s>] [--strip <n>]` for voxel
  assemblies only.
- **Game driver pattern:** vigil's `games/vigil/src/animation.rs` writes `Animation` from Rust and
  holds the attack for a hand-kept `ceil(0.35 × hz)` ticks, which duplicates the clip's duration.
- **Fixture:** the eleven-joint mannequin (`Plan/skeletal-animation.md` W1) imports clean.

## Decisions

1. **Scope: the animation system only.** The platformer's character controller, camera and
   gameplay get their own plans. **Why:** animation is a self-contained arc with its own exit and can
   be proven on a fixture before the game exists.
2. **The pose is visual-only; the playback state is hashed.** Bones are evaluated in `extract_at` and
   never enter `WorldHash`. `Animation` (clip, start, speed, crossfade) is sim state. **Why:** the
   platformer's gameplay is capsules and spheres, and re-animating must never move replays. A
   sim-side one-bone query can be added later without redesign, because the sampler is pure.
   *Proposed amendment (Serious Isaac 0016, not approved): see `Plan/skeletal-animation.md` §Decisions "Proposed"*.
3. **Clips are authored in Blender and imported to canonical text.** A committed bpy script keys the
   actions (the `Plan/asset-pipeline-blender.md` contract), `ludion import` writes text clips, and
   the glb stays derived. **Why:** Pillar 4, and baked skeletal curves are impractical to hand-author.
4. **Game Rust drives playback; the engine provides primitives.** Play, crossfade, speed and events.
   There is no state-machine asset. **Why:** a transition-condition language drifts into scripting,
   and one character does not justify a graph.
5. **v1 features: crossfade, scale tracks, events.** Masked layers, root motion and IK are
   non-goals, each with a trigger.
6. **Driving content:** the mannequin fixture with Blender-keyed clips proves the engine. The hero is
   a separate Blender brief, judged on its own. **Why:** it decouples this arc from the open Blender
   look verdict.
7. **One clip kind.** `kind: animation` is extended: a track names a *target*, resolved against
   whatever plays the clip (an assembly part or a skin joint). A `scale` channel and `events:` are
   added for both. **Why:** a part and a joint are both a named rigid node under a parent, and
   crossfade and events then land once for voxels and skins.
8. **Rotation stays Euler degrees.** Import converts each glTF quaternion key to the Euler solution
   nearest the previous key (no wrap flips), then samples the result against the source and refuses
   the clip above the error bound (decision 13). Runtime blending (crossfade, and interpolation
   between keys) happens in quaternion space, so Euler is storage only. **Why:** one spelling an
   agent can read and hand-tune, with no migration of voxel clips. **Trigger for quaternions:** a
   real clip that trips the bound after unwrapping.
9. **Baked clips are key-reduced; block style stays.** Import drops every key that interpolation
   reproduces within the reduction tolerance. W1 measures the mannequin clips' real size, and the
   spelling is revisited only if they are unworkable. **Why:** relaxing the YAML subset is a
   cross-cutting format decision, taken from a measured file and not a projection.
10. **`kind: character` binds a skinned mesh to its clips.** It is a hand-authored document naming
    the mesh and a `clips:` map. Entities use it as `Mesh.shape: { character: <path> }`, and
    `Animation.clip` resolves through its map. It wraps skinned meshes only: the voxel assembly keeps
    its own `clips:` map. **Why:** the mesh document is derived and can't hold a hand-kept map. The
    assembly is already the voxel model's hand-authored document. **Trigger to merge the two:** a
    character feature that assemblies also want.
11. **Imported motion is derived; the character carries a per-clip overlay.** An imported clip bears
    `generated_from` and is drift-gated. A character's clip entry may add `events:` and `speed`.
    Import ignores Blender markers, so a derived clip never carries events. A hand-authored clip
    (voxel ones included) carries its own `events:`, and an overlay adding `events:` to a clip that
    has its own is refused. **Why:** events are gameplay tunables iterated far more often than
    motion and must not need a Blender re-run (the same reasoning as skeletal decision 4). There is
    exactly one home for each clip's events.
12. **Clips bind by joint name.** Any skin whose joints match can play a clip. A track naming a joint
    the skin lacks is a diagnostic, and an untouched joint stays at rest. There is no proportion
    retargeting.
13. **Import tolerances (committed defaults, overridable per import):**
    - Key reduction: 0.1° rotation, 0.5 mm translation (1 u = 1 m), 0.001 scale.
    - Error bound: 0.5° / 2 mm / 0.005, measured per joint **in model space** at every source frame,
      so error accumulating down a chain is caught.
14. **Crossfade lives on `Animation`.** New fields are the outgoing clip (`from`, `from_started`,
    `from_speed`) and `fade_started`/`fade` (seconds), all defaulted so existing scenes read the
    same. One plain function, `crossfade(&mut Animation, clip, seconds, now)`, is the way games call
    it. Crossfading a sprite flipbook is a diagnostic. **Why:** a fade is part of what is playing;
    add/remove churn on a separate component complicates replays and snapshots.
15. **Events arrive in a per-tick resource.** A sim system, in registration order, appends each
    `(entity, clip, event)` crossing of the tick to `AnimationEvents`. A one-shot's end is an implicit
    `end` event. **Accepted trade:** clip timing (duration, event times, overlay speed) becomes sim
    input for scenes that use events, so retiming moves their `WorldHash` (the same shape as skeletal
    decision 6). It is parsed from canonical text on the CPU, so it is bit-stable across platforms.
16. **One pose source per skin stays.** `Animation` on a skin that `SkinBone` parts name remains
    `L1197`.
17. **The design lives in this plan.** `Plan/skeletal-animation.md`'s "no clips" non-goal points
    here, and `Plan/voxel-animation.md` stays closed.
18. **The editor clip preview needs no new wire method.** `component.set` / `component.remove` of
    `Animation` (its W2 crossfade fields included), `sim.pause` / `resume` / `step` / `set_speed` and
    `render.view` express play, pause, scrub, speed and crossfade preview; the Voxel Artist's assembly
    clip strip is the precedent, generalized so the Models view reuses it for `kind: character`.
    **Why:** every field a crossfade writes is a plain serde field of a registered component, so a
    whole-value `component.set` is exactly what `crossfade()` writes. Rejected: an `animation.*` wire
    method — a second way to write what `component.set` already writes, and a new method is the
    user's decision (`editor/CLAUDE.md`).
19. **Playback cadence is a per-entity tick hold.** `Animation.hold` (ticks, default 1) is applied to the
    clip clocks and the fade weight at extract, on a global grid (`ticks − ticks % hold`); the default 1
    is byte-identical to today. Events and the ragdoll pose keep the exact tick. **Why:** Clockout 0027,
    stop-motion 'on twos' with crossfades included; per-entity only, no scene-wide resource. Rejected:
    `pose_hz` (not exact at 60 Hz), per-entity phase. `set_speed` between two steps
    stores the grid tick's clock in the anchor (`grid_ticks`), so the pose moves only on the grid.
    *Proposed amendment (Serious Isaac 0019, not approved): a per-level cadence factor, `Plan/gpu-driven-culling.md` §Decisions "Proposed (Serious Isaac 0019 …)"*.

**Proposed (Serious Isaac 0019, owner approval owed). Crowd pose sharing and palette reuse.** Ready to build once approved.

- (0) **Piece 1, palette reuse: measured and rejected.** A cache copying a held palette (and a serial reuse pre-pass) was built and measured on `crowd-animation` (`Skin` median ms, reuse off/on): per-entry cache T1 .381/.298, T8 .324/.370, default .321/.409; serial pre-pass T1 .373/.250, T8 .370/.380, default .374/.378. The gate was a win or tie at both T=8 and the default thread count; it lost. Why: parallel posing already sits near the thread-spawn floor, so copying a held palette costs about what sampling it does. The evidence comes from one 32-thread box, where run-to-run noise (the reuse-off baseline read .324 vs .370 at T8 and .321 vs .374 at the default count) is about the size of the effect, and serial reuse at T1 (.250) beat every parallel figure, so the rejection is provisional. Re-measure on the M4 with more rounds, including reuse plus serial posing, and on rigs above ~30 joints or a few-core target such as the iOS device.
- (i) **Exact sharing.** A palette is in skin-model space, so two skinned instances with the same key have bit-identical palettes: the mesh `Arc`, character `Arc`, sampler identities, held clip time bits and fade weight bits (the key is new; none exists yet). Sharing points their `MeshInstance.bone_offset` at one range. A serial pass over the jobs in registration order computes each sampled job's key and assigns a first-occurrence owner through `ludion_core::collections::StableMap`; only owners are sampled, in parallel as before; the concatenation and every `bone_offset` are then a pure function of job order, the same at 1, 2 and 8 threads. A sharer counts `palettes_shared` and no joints. The output is identical, so it amends no decision and is the first item to build if approved.
- (ii) **`bone_offset` audit.** Confirm at build that nothing writes into an instance's palette range after extract: grep the `bone_offset` consumers, the viewmodel and the skinned culling bounds first. The GPU bound (`Plan/gpu-driven-culling.md` decision 13) stays per instance, from the shared palette × its own model.
- (iii) **`pose_buckets: N`.** Per mesh level on `kind: character`: snap clip time to `duration / N` at that level so more instances share. It changes the picture, so it is opt-in, built after exact sharing, and only if the bench's `palettes_shared` at the far levels shows it pays.
- (iv) **Leaf-subtree hold at rest** (the request's third open question) is deferred until `joints_sampled` shows joint count, not instance count, is the cost.
- (v) **Acceptance.** Goldens byte-identical with exact sharing on; palette bytes identical at 1, 2 and 8 threads; `palettes_shared` above 0 on `crowd-animation`.
- (vi) Nothing enters `WorldHash`; the wire schema does not move.
- (vii) **Gate.** Sharing and `lod_hold` cut sample work, unlike reuse, so they may pay, but each ships only if it wins or ties at T=8 and the default thread count in `skin_pose_bench`.

## The formats

A clip (extended; the target is a part or a joint):

```yaml
format: 1
kind: animation
name: spin
duration: 0.4
loop: false
events:            # hand-authored clips only; derived clips take events from the character
  - t: 0.05
    name: spin_active
tracks:
  spine:
    rotation: [...]    # Euler XYZ degrees, block style
    translation: [...] # part-local voxel units for a part, mesh units for a joint
    scale: [...]       # new: per-axis multiplier
```

A character (new, hand-authored):

```yaml
format: 1
kind: character
mesh: mannequin.mesh.yaml
clips:
  run:
    clip: mannequin.clips/run.anim.yaml
    speed: 1.2           # overlay, optional
    events:              # overlay, optional; refused if the clip has its own
      - t: 0.1
        name: foot_l
      - t: 0.4
        name: foot_r
  spin:
    clip: mannequin.clips/spin.anim.yaml
```

Exact key names, the new `L1xxx` codes (the next free slots when each wave lands) and the imported
clip file layout are settled in W1 and W2 and recorded in Status.

## Waves

### W1 — format, import and the numeric clip report

1. `ludion-scene`: tracks name a target; the `scale` channel; `events:` (strictly increasing
   times within `[0, duration]`, non-empty names). Existing voxel clips re-emit byte-identically.
2. `ludion-import`: glTF animations become clip files with `generated_from`. Euler unwrapping, the
   model-space error bound and key reduction per decisions 8, 9 and 13. `L1202`'s help text drops
   "animations". STEP and LINEAR samplers map to `step` and `linear`; CUBICSPLINE is sampled and
   reduced.
3. **Fixture:** a committed bpy script keys the mannequin's four clips:
   - `idle`: a loop of subtle breathing (tiny motions must survive reduction);
   - `run`: a loop (the seam; foot events come from the W2 overlay);
   - `jump`: a one-shot with squash on takeoff and stretch in the air (the scale channel, `end`);
   - `spin`: a 0.4 s one-shot (the `spin_active` window).
   The glb is derived; the imported clips are committed with a drift gate.
4. **The numeric clip report:** `check-scene` on a clip prints duration, key counts before and
   after reduction, the worst error against source per joint, per-joint channel ranges and the loop
   seam error. A report on a character adds foot-joint height over time.
5. **Measure** the four clips' sizes and record them in Status (decision 9's trigger).
6. Docs: the clip format topic (targets, scale, events) and `dev-docs/ludion-import.md` (the
   animation mapping).

### W2 — runtime, strips and the gallery scene

1. `kind: character` (reader, canonical writer, validation, the overlay rules of decision 11) and
   `Mesh.shape: { character }`, with asset-cache dependency edges so that reloading a clip re-binds.
2. Binding by joint name (decision 12). Skinned sampling in `extract_at` feeds `posed_palette`, and
   scale composes down the joint chain. Assemblies gain the scale channel through the same sampler.
3. The crossfade fields and `crossfade` helper (decision 14), with blending in quaternion space.
4. The `AnimationEvents` system and the implicit `end` (decision 15), for characters and
   assemblies.
5. **Proofs:**
   - hash-neutrality: an extracted run's `WorldHash` sequence and `.lrp` match a never-extracted run;
   - event determinism: thread-count invariance, and the M4 twin (queued as owed);
   - the existing goldens and vigil's corpus. If the new `Animation` fields move vigil's hash
     encoding, its replays re-record last, after the schema snapshot.
6. **Strips:** `render --model <character> --clip <c> [--time] [--strip <n>]` for skinned
   characters, and a crossfade strip (`--crossfade <from>:<to>:<seconds>`) rendering the blend over
   its duration.
7. **Gallery scene:** `examples/gallery/` gets a mannequin cycling idle → run → jump → spin from a
   small driver, crossfading run↔idle, run→jump and jump→run. It has a trace, a `.lrp` and a golden
   blessed on the 3090 (the M4 re-bless is owed).
8. Docs: `docs/components.md` (`Animation`'s new fields, `AnimationEvents`), a character topic,
   and vigil's `ceil(0.35 × hz)` hold replaced by the `end` event.

### W3 — editor clip preview

1. The parity gate comes first. Check whether playing and scrubbing a clip on the Models stage is
   expressible with existing commands (an entity edit of `Animation`, plus pause/step). A new wire
   method is added only if that check proves one is needed, and the decision is recorded.
2. The preview: pick a character, list its clips, play, pause, scrub, and preview a crossfade.
3. **Exit (human gate, owed):** one sitting judges the gallery scene's feel (pops at loop
   seams and crossfades, foot sliding, the squash timing) in the window
   (`cargo run -p ludion-testkit --example character_cycle -- --scene examples/gallery/character-cycle.yaml --window`) and runs the editor preview's live smoke.
   The verdict is recorded as a decision. Close: the root `CLAUDE.md` line, and this plan's Status
   collapsed.

## Non-goals

- A declarative animation state machine or graph. **Trigger:** a second character whose driver
  Rust visibly repeats the first's.
- Masked or additive layers. **Trigger:** a clip that must play over another on part of the body.
- Root motion. **Trigger:** a move whose displacement can't be matched by code.
- IK (foot placement, look-at). **Trigger:** visible foot sliding or floating on slopes that the
  sitting rejects.
- Proportion retargeting.
- Morph targets (they stay `L1202`).
- A sim-side bone query. **Trigger:** gameplay that must read a bone. `posed_joint_world` (Clockout 0028) is a presentation read, refused from sim with `L0028`, so it does not fire this trigger.
- Switching a skin between clips and `SkinBone` at runtime (a death ragdoll).
  *Proposed lift (Serious Isaac 0016, not approved): see `Plan/skeletal-animation.md` §Decisions "Proposed"*.
- Quaternion storage (decision 8's trigger) and a compact keyframe spelling (decision 9's trigger).

## Risks

- **Euler import error.** Fast spins (the Crash spin is a full turn in 0.4 s) are exactly where
  unwrapping and reduction are stressed. The error bound makes the failure a diagnostic, and the
  spin clip is in the fixture set for that reason.
- **Clip file size.** Block-style keys are about 5 lines each. If reduction leaves the clips
  unworkable, decision 9's trigger fires.
- **The `Animation` hash encoding.** New defaulted fields may still move the encoding. vigil
  re-records if so.
- **Events make clip timing sim input.** A retimed clip silently moves the hash of scenes that
  consume events. That is accepted (decision 15), and the replay re-record is the visible cost.
- **No culling bounds for skinned instances** (skeletal risk, unchanged). An animated character
  that moves far from its rest bounds matters only once mesh culling lands.
