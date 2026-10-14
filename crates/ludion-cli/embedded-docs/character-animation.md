---
description: Skinned characters: kind: character, imported clips, crossfade, AnimationEvents, the cycle example
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
  leaves out keeps its bind value. Rotation interpolates as a quaternion slerp (no gimbal flips), so
  a segment turning 180° or more on an axis, or whose axes combine past half a circle, is
  `L1142`: split it. `scale` composes down the joint chain, and the binding's `speed` multiplies
  `Animation.speed`. An empty or unbound clip name draws the rest pose. The pose is render-side only — never hashed or recorded. A skin with `SkinBone`
  followers is posed by them instead. Lighting follows the pose: a joint with a non-uniform scale
  bends the skin's normals the way it bends the surface.
- Editing the character, its mesh or any clip re-reads it. `ludion check-scene <file>` loads and
  reports it, with a `contact <joint>: height min … max …` line per clip for each ground-contact
  joint (a leaf joint whose vertices sit at the mesh's floor at rest — the feet), sampled at 30 Hz in
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
  `leg_length × (scale − 1)`; `check-scene <character>` reports each ground-contact joint's height
  range per clip (`contact <joint>: height min … max …`) — a `min` below zero is a foot through the
  floor. The mannequin's `jump` keeps `drop = 0.52 × (sy − 1) + air` for exactly this.

## Crossfade

- Call `crossfade(&mut animation, clip, seconds, ctx.game_frame)` instead of writing `clip`: the
  old clip becomes `from`, `started` restamps, and the pose blends until `seconds` have passed.
- `seconds <= 0` is a **hard cut** and clears `from`. Interrupting a fade in progress drops the older
  outgoing clip, which pops.
- `from` **persists after the fade ends**, so a stateless driver can read where a clip was entered
  from (the cycle below picks the next clip after `run` from it).
- To change speed mid-clip call `set_speed(&mut animation, speed, ctx.game_time)`: clip time stays
  continuous for the pose and for events. It moves the incoming side only; a fading-out clip keeps the
  rate it had. Write `speed` directly only together with a new `started`.
- A sprite flipbook cannot crossfade: `L1209`.

## AnimationEvents

- Register `animation_events` **before** the system that reads the resource, declaring
  `.query::<(&Animation, &Mesh)>()` and `.query::<(&Animation, &Sprite)>()`; a consumer reads
  `events.fired(entity, clip, name)` the same tick.
- A one-shot clip emits an implicit `end`; a clip's own events come from the clip or the character's
  `events:` overlay (`L1198` for a malformed one, `L1200` for an overlay on a clip that has its own).
  Only the incoming clip of a crossfade emits.
- A `t: 0` event fires once per clip start, never lost and never twice. A clip stamped by a system
  registered before `animation_events` fires it on the start tick; one stamped by a later system (a
  reaction to `end`) fires it on the next tick, together with that tick's first clip time.
- **Consuming events makes clip timing sim input.** A retimed or re-imported clip moves the
  `WorldHash` and a replay's recorded run, and a replay root must hold the model files (`clips/`,
  `characters/`, the mesh) — a root without them plays no clip.
- Write `Animation` only when the choice changes, and stamp with `ctx.game_frame`, never `ctx.frame`
  (a held gameplay clock would restart the clip late).

## Strips

`ludion render --model <x>.character.yaml --clip <name>` (or `--crossfade`) renders a strip without
running a game; the flags are owned by `ludion docs voxel-animation`.

## Worked example

`examples/gallery/character-cycle.yaml` is one mannequin; `crates/ludion-testkit/tests/common/mod.rs`
is its whole game half: a transition table of `(clip, entered from, leaves on, next, fade)` driving
idle → run → jump → run → spin → run → idle, one-shots leaving on `end`, the spin a hard cut. Its
proofs are `crates/ludion-testkit/tests/character_cycle.rs` (the clip-switch log, the committed
trace and replay) and the `character-cycle-driven` golden in `gallery.rs`.

The same driver is a runnable host, `crates/ludion-testkit/examples/character_cycle.rs`. From the
repo root, watch it in a window, or replay-verify the committed recording:

```text
cargo run -p ludion-testkit --example character_cycle -- --scene examples/gallery/character-cycle.yaml --window
cargo run -p ludion-testkit --example character_cycle -- --replay-verify examples/gallery/character-cycle.lrp --scene examples/gallery/character-cycle.yaml
```

## Next

- `ludion docs voxel-animation` — the clip strips (`--clip`, `--crossfade`) and rigid assemblies.
- `ludion docs components` — the `Animation` component's fields and defaults.
- `ludion docs sprite-animation` — the same component over a `Sprite`'s flipbook.
- `ludion docs blender-craft` — authoring the rig and clips in Blender.
