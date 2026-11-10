---
description: Skinned characters: kind: character, clips, crossfade, AnimationEvents, crowd posing cost
---

# Character animation

**Scope:** making a skinned mesh move under game code — the `kind: character` document, the clips
`ludion import` produces from Blender, `crossfade`, and `AnimationEvents`. The `Animation` component's
fields and defaults are owned by `ludion docs components`; the clip strips and their flags by
`ludion docs voxel-animation`; the rig itself by `ludion docs blender-craft`.

## The character document

A `kind: character` document binds a skinned mesh to named clips; an entity draws it with
`Mesh.shape: { character: "characters/hero.character.yaml" }`, and `Animation.clip` names a key of
its `clips:`. Example: `examples/gallery/characters/mannequin.character.yaml`; run
`cargo run -p ludion-engine --example character_clips` to print each clip's effective values.

```yaml
format: 1
kind: character
mesh: ../meshes/skin-mannequin.mesh.yaml
clips:
  run:
    clip: ../clips/run.anim.yaml
    speed: 1.2
    events:
      - t: 0.1
        name: foot_l
```

- `mesh` and each `clip` resolve relative to the character file; `..` is legal, an absolute, drive or device path is refused (`L1200`). A plain
  `{ asset: … }` of the same mesh shares one GPU upload.
- `events` overlays a clip that has none of its own (an imported clip never does); overlay events
  on a clip that has its own are `L1200`, as are a mesh with no `joints:` and a clip track naming
  a joint the mesh lacks. An overlay event past the clip's duration, or at it on a looping clip (a loop's end is its
  start), is `L1198`; a malformed
  document is `L1199`.
- **Playback.** An entity with `Animation.clip` naming a bound clip poses the skin from it: a track
  binds to the joint of the same name, a joint with no track stays at rest, and a channel a track
  leaves out keeps its bind value. Rotation slerps, so
  a segment turning 180° or more on an axis, or past half a circle combined, is
  `L1142`: split it. `scale` composes down the joint chain, and the binding's `speed` multiplies
  `Animation.speed`. An empty or unbound clip name draws the rest pose. The pose is never hashed or recorded, and sim code cannot read it; presentation code reads it through `posed_joint_world` (§Sockets). A skin with `SkinBone`
  followers is posed by them instead. A running skin can leave its clip for a ragdoll with `switch_skin_to_ragdoll`, which drops `Animation` and poses from rest (`ludion docs physics-joints`). Lighting follows the pose: a non-uniform joint scale
  bends the skin's normals with the surface.
- Editing the character, its mesh or any clip re-reads it (the pose; `AnimationEvents` picks up retimed events at the next load, restore or scene switch). `ludion check-scene <file>` loads and
  reports it, with a `contact <joint>: height min … max …` line per clip for each ground-contact
  joint (a leaf joint at the mesh's floor at rest — the feet), sampled at 30 Hz in
  mesh units; `SkinBone` parts can name a character's skin.

## Clips

- A clip is a committed `kind: animation` file under `clips/`, imported from a glTF by
  `ludion import <glb>` (one clip per glTF animation, one track per joint, bound by joint name).
  `loop` and the key-reduction tolerances come from the Blender Action's custom properties (glTF
  animation `extras`), never from CLI flags.
- **Edit the bpy source, never the clip.** A clip carries `generated_from:`; a drift gate re-imports
  it and fails on a byte difference. The mannequin's source is
  `crates/ludion-import/tests/fixtures/skin-mannequin-clips.py` and its re-import recipe is the doc
  comment of `crates/ludion-cli/tests/import.rs`'s clip drift test.
- **A pelvis scale rescales every joint under it.** A squash on the root lifts or sinks the feet by
  `leg_length × (scale − 1)`; a `min` below zero in `check-scene <character>`'s `contact` lines is a foot through the
  floor. The mannequin's `jump` keeps `drop = 0.52 × (sy − 1) + air` for exactly this.

## Crossfade

- Call `crossfade(&mut animation, clip, seconds, ctx.game_frame)` instead of writing `clip`: the
  old clip becomes `from`, `started` restamps, and the pose blends until `seconds` have passed.
- `seconds <= 0` is a **hard cut** and clears `from`. Interrupting a fade in progress drops the older
  outgoing clip, which pops.
- `from` **persists after the fade ends**, so a stateless driver can read where a clip was entered
  from (the cycle below picks the next clip after `run` from it).
- To change speed mid-clip call `set_clip_speed(&mut animation, speed, ctx.game_time)`: clip time stays
  continuous for the pose and for events. It moves the incoming side only; a fading-out clip keeps the
  rate it had. Write `speed` directly only together with a new `started`.
- A sprite flipbook cannot crossfade: `L1209`.

## Stepped playback

- `hold: N` on `Animation` shows each pose for `N` sim ticks (default `1`, every tick): `2` is 30 poses
  a second at 60 Hz, `5` is 12 — stop-motion "on twos" at 24 fps. `0` is `L1317`, in files and on the wire.
- The grid is global: the pose is evaluated at the last tick that is a multiple of `N`, so every entity
  with the same `hold` changes pose on the same ticks, whenever its clip started.
- The incoming clock, the outgoing clock and the crossfade weight step together, so a fade is stepped too.
- Visual-only: `AnimationEvents` and the ragdoll pose keep the exact tick, and `set_clip_speed` between two
  steps changes the pose only from the next step (the grid does not drift). It lowers the visible cadence, not the cost: every skinned instance is sampled every extract (§Crowd posing).
- A clip switched between steps shows its first pose at once; start clips on the grid or crossfade.
- Review a stepped strip with `ludion render --model <x>.character.yaml --crossfade run:idle:0.5 --strip 8 --hold 5`: phases snap to the grid, so neighbours repeat a pose; the output name gains `-hold5`.
- Stepping *inside* a clip is the clip's own `ease: step` keys (`ludion docs voxel-animation` §Format).

## Crowd posing

- **Parallel, deterministic.** Extract's `Skin` phase samples palettes in fixed chunks through
  `ludion_core::par`, joined in order: byte-identical at any thread count. `hold` does not save sampling: a rest skin samples no joints; a `SkinBone`-followed skin is posed from its parts every extract and counts in `palettes_sampled`/`joints_sampled`; every other skinned instance is sampled every extract.
- **Reading a profile.** `ludion profile` and a game's `--profile N` (`ludion docs running`) report
  `ProfileReport.skin`, offline only: `pose_median_ms` (the `Skin` phase) and `<count>_median` for `instances`,
  `palettes_sampled`, `palettes_reused`, `palettes_shared` (both reserved, always 0) and `joints_sampled`.
  Per extract `instances = sampled + reused + shared`. Read the time beside `sim.threads`.
  `ProfileReport.extract.phases` times every extract phase: `poses` scales with `ParentJoint` sockets on
  animated skins (each such skin is posed once, here), `place` with entities, `meshes` with drawn instances, `skin`
  with skinned instances and joints.
- **Indicative figures** (RTX 3090 / 5950X, release): 500 figures, 5,500 joints: 0.37 ms at 1 thread, 0.32 ms at 8 or 32. The owner is `Plan/animation-clips.md` Status "Crowd posing".
- **Sockets pose once.** A skin carrying a `ParentJoint` child is posed in the parallel `Poses` phase and `Skin` reuses that palette, so a socket costs no second pose. 500 socketed figures, `Poses`+`Skin` (RTX 3090 / 5950X, release): 1.05 ms to 0.70 at 1 thread, 1.03 to 0.75 at 8.
- Bench: `cargo test -p ludion-engine --release --test it -- skin_pose --ignored --nocapture` (`socket_pose` for the socketed crowd).

## AnimationEvents

- Register `animation_events` **before** the system that reads the resource, declaring
  `.query::<(&Animation, &Mesh)>()`, `.query::<(&Animation, &Sprite)>()` and `.query::<(&Animation, &PrefabInstance)>()` (observed even with no prefab in the world; a missing one is `L1126`); a consumer reads
  `events.fired(entity, clip, name)` the same tick.
- A one-shot clip emits an implicit `end`; a clip's own events come from the clip or the character's
  `events:` overlay (`L1198` for a malformed one, `L1200` for an overlay on a clip that has its own).
  Only the incoming clip of a crossfade emits.
- A `t: 0` event fires once per clip start, never lost or doubled: on the start tick for a clip stamped by a system
  registered before `animation_events`, on the next tick (with its first clip time) for a later one (a reaction to `end`).
- **Consuming events makes clip timing sim input.** A retimed or re-imported clip moves the
  `WorldHash` and a replay's recorded run, and a replay root must hold the model files (`clips/`,
  `characters/`, the mesh) — a root without them plays no clip.
- Write `Animation` only when the choice changes, stamped with `ctx.game_frame`, never `ctx.frame`
  (a held gameplay clock restarts the clip late).

## Sockets

`ParentJoint` makes a prop follow a joint of a skinned mesh. The parent is the skin
(`shape: character`, or `asset` with `joints:`); the child's `Transform` is local to the joint's model transform.

```yaml
- name: baton
  components:
    Parent: { index: 3, generation: 0 }   # the mannequin entity
    ParentJoint: { joint: lower_arm_l }
    Transform: { translation: [0.0, 0.2, 0.0] }   # along the bone, to the hand
    Mesh: { shape: cube, color: [1.0, 0.15, 0.55, 1.0] }
```

- **Rest vs drawn.** Sim reads (`world_transform`, `world_affine`, physics, `reparent`) compose the joint's
  **rest** pose, so no `WorldHash` depends on clip data. The draw composes the **posed** joint — the clip,
  the crossfade, the held tick of `hold` and any `SkinBone` followers — and the child's descendants ride on it.
  The sim reads the rest pose from the skin's committed mesh document under the project root; without it
  (an engine with no root, or a missing mesh) the child composes as a plain `Parent`, so its sim results
  and hashes differ.
- Under present interpolation the socket child and its subtree take the skin's blend key, so the prop
  never swims against the body.
- `posed_joint_world(world, skin, joint, now) -> Result<Option<WorldPose>, Diagnostic>` is presentation
  and host only; a sim system gets `L0028`. `Ok(None)` means a dead skin, no `Transform`, not a skin, or an
  unknown joint; a skin without `Animation` returns the rest pose. Idiom: on the hit tick a presentation
  system spawns the thrown prop at that pose with `spawn_presentation`.
- `L1318` (no `Parent`, or the parent is not a skinned mesh) and `L1319` (unknown joint; the help names the
  nearest) fire at load, `check-scene` and prefab spawn. A runtime miss composes as a plain `Parent`, silently.
- Author a uniform skin scale: `WorldPose` is TRS, so a sheared pose is lossy. `reparent(.., Keep::World)`
  on a socket child keeps the rest-pose world, not the drawn one.
- Example (three phases of `run`, a prop on each `lower_arm_l`):
  `ludion render --scene examples/gallery/character-socket.yaml --frames 60`.

## Strips

`ludion render --model <x>.character.yaml --clip <name>` (or `--crossfade`) renders a strip without
a game; flags: `ludion docs voxel-animation`.

## Worked example

`examples/gallery/character-cycle.yaml` is one mannequin; `crates/ludion-testkit/tests/common/mod.rs`
is its whole game half: a transition table of `(clip, entered from, leaves on, next, fade)` driving
idle → run → jump → run → spin → run → idle, one-shots leaving on `end`, the spin a hard cut. Its
proofs: `crates/ludion-testkit/tests/character_cycle.rs` (switch log, trace, replay) and the
`character-cycle-driven` golden in `gallery.rs`.

The same driver is a runnable host, `crates/ludion-testkit/examples/character_cycle.rs`; add `--window` to watch it or `--replay-verify examples/gallery/character-cycle.lrp` to verify the recording:

```text
cargo run -p ludion-testkit --example character_cycle -- --scene examples/gallery/character-cycle.yaml --window
```

## Next

- `ludion docs hierarchy` — `Parent` and `ParentJoint` rules.
- `ludion docs voxel-animation` — clip strips and rigid assemblies.
- `ludion docs components` — the `Animation` component's fields and defaults.
- `ludion docs sprite-animation` — `Animation` over a `Sprite` flipbook.
- `ludion docs blender-craft` — authoring the rig and clips in Blender.
- `ludion docs prefab-animation` — rigid node clips on a prefab, no skin.
