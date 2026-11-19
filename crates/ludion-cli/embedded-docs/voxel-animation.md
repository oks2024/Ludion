---
description: Animating a voxel assembly: kind: animation clips, the Animation component, posed sheets
---

# Voxel animation

**Scope:** making a multi-part voxel model move — the `kind: animation` clip document, the
`Animation` component game code writes, and the posed sheets that judge a clip without running the
game. The model those clips address is `ludion docs voxel-assemblies`; the component table is
`ludion docs components`.

Animation is **rigid parts on declarative clips**: no skinning, no state machine; picking a clip is
ordinary game code. The same `Animation` component drives a `Sprite`'s atlas flipbook
instead of a `Mesh`'s clips — `ludion docs sprite-animation` covers that binding.

## The loop

1. **Bind the clip** in the assembly's `clips:` map — short name → file, relative to the assembly:
   ```yaml
   clips:
     walk: anims/humanoid-walk.anim.yaml
   ```
2. **Check it.** `ludion check-scene models/x.assembly.yaml` validates every bound clip and lists the
   bindings (`clip walk: anims/humanoid-walk.anim.yaml`) — the names `--clip` and `Animation` accept —
   then each `masks:` entry (`mask arm_only: arm (descendants)`) and each `blends:` entry
   (`blend locomotion (1D, sync foot_l): idle@0, walk@2`, with a 2D blend's triangles beneath); `--json`
   adds `masks` and `blends` arrays when there are any.
   `ludion check-scene <clip>.anim.yaml` alone prints the clip's numbers: duration, loop, key counts
   per track, each channel's key-value range and, for a looping clip, the local seam (`t = 0` against
   `t = duration`). A clip with `generated_from` pointing at a glTF is re-imported: key counts before
   reduction, the worst model-space error per joint, the model-space seam, and a note if the file is
   stale (a missing or unusable source is a note too). `--json` prints the same as one object.
3. **Look at the whole clip as one image**, with a text report per phase:
   ```
   ludion render --model models/figure.assembly.yaml --clip wave --strip 4
   ```
   ```text
   clip wave: duration 0.8, loop true, 4 phases quantized to 60 Hz ticks
     phase 0: t 0 (tick 0), 1 component
     phase 1: t 0.2 (tick 12), 1 component
   ```
   `--strip <n>` (2 to 8) renders `n` evenly spaced phases side by side and defaults its output to
   `<name>-<clip>-strip.png`; `--time <s>` renders one instant to `<name>-<clip>-t<s>.png`. Both are
   the ordinary model sheet, so every flag in `ludion docs voxel-sheets` still applies — except that a
   strip lays its phases out in one cell and therefore takes one view.
   `--model` also takes a `.character.yaml` (alone, never in a lineup): the same flags pose it, with
   no gap analysis (no `components`), and a phase's `t` is clip time through the binding's `speed`.
   **`--crossfade <from>:<to>:<seconds>`** (assemblies and characters, not with `--clip`) renders the
   blend as a strip: 5 phases, or `--strip n`, at fade weights 0 to 1, each printed as `weight <w>`
   under a `crossfade <from> -> <to>` header. `--time` is how far `from` has played when the fade
   begins. Output defaults to `<name>-<from>-<to>-crossfade.png`.
   **`--blend <name>:<p0>[:<p1>]`** (assemblies and characters, not with `--clip` or `--crossfade`)
   sweeps a blend: each `p` is a number or `lo..hi`, swept linearly over 5 phases (or `--strip n`).
   Each phase prints `params [x, y], phase <cycles>` and its gap analysis, all at `--time`; output
   defaults to `<name>-<blend>-blend.png`. **`--layer <clip>:<weight>[:<mask>[:override|additive]]`**
   (repeatable, at most four, `L1670`) stacks layers over `--clip`, `--crossfade` or `--blend`, echoed as
   `layers: …`, and the gap analysis poses through them; the output gains `-layered`. A prefab has no
   blends or layers.
4. **Wire it** from a game system that writes `Animation` when the *choice* changes.

## Format

`kind: animation` — one clip per file, **block style only** (a flow keyframe `{ t: 0.0, v: [25, 0, 0] }`
is `L1005` and does not parse):

| key | required | meaning |
| --- | --- | --- |
| `format` | yes | `1` |
| `kind` | yes | `animation` |
| `name` | no | human label |
| `generated_from` | no | source path of an imported clip; a derived clip is never hand-edited and carries no `events` |
| `duration` | yes | clip length in seconds; finite, strictly positive, at most 3600 |
| `loop` | yes | `true` wraps at `duration`; `false` is a one-shot that holds its final pose |
| `events` | no | named instants `{t, name}`, `t` within `[0, duration]` (below `duration` on a looping clip) and strictly increasing, `name` non-empty |
| `tracks` | yes | target name → the channels animating it; at least one track |

A track name is a **target**: an assembly part here, a skin joint when a skin plays the clip. A track
holds `rotation`, `translation` and/or `scale`, each a sequence of keyframes:

| key | required | meaning |
| --- | --- | --- |
| `t` | yes | seconds, within `[0, duration]`, strictly increasing within its channel |
| `v` | yes | `[x, y, z]` — Euler XYZ **degrees** for `rotation`, part-local **voxel units** for `translation`, a per-axis multiplier (`> 0`) for `scale` |
| `ease` | no | `linear` (default), `smooth` (smoothstep), or `step` |

```yaml
format: 1
kind: animation
name: figure-wave
duration: 0.8
loop: true
tracks:
  arm-l:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 0.4
        v:
          - 0.0
          - 0.0
          - 25.0
        ease: smooth
```

Playback applies `rotation`, `translation` and `scale` (a part scales about its pivot, and its
children ride the scaled frame; a ragdoll switch ignores `scale`); a game that registers `animation_events` receives its `events` (`ludion docs
character-animation`).
There is no visibility channel. Canonical form is `format, kind, name, generated_from, duration, loop,
events, tracks`, sorts tracks by target name, orders channels `rotation, translation, scale`, and
writes `ease: linear` as the key's absence. An event block:

```yaml
events:
  - t: 0.05
    name: spin_active
```

## Rules

- **A channel is total.** Before its first keyframe it holds the first value; past its last it holds
  the last. An absent channel is not a zero — the part keeps its rest value on that channel, so a clip
  that rotates an arm does not silently un-translate it. A part the clip never mentions poses entirely
  at rest, which is what lets one shared clip drive a model with extra parts.
- **Easing belongs to the *destination* key.** A segment eases the way the key it arrives at says, so
  two adjacent segments can ease differently and the first key's `ease` is inert. `step` holds the
  segment's start value until the destination key's own time.
- **Rotation interpolates as a shortest-path quaternion slerp** between keys (no gimbal flips);
  translation and scale interpolate per axis. Both are exact at the channel's ends. A rotation
  segment turning 180° or more on an Euler axis, or whose axes combine past half a circle, is
  `L1142` (it would play the short way round, a full turn as none): split it. A `step` segment is
  exempt.
- **A one-shot holds.** Past `duration` a `loop: false` clip stays on its final pose forever; a
  looping clip wraps into `[0, duration)`.
- **Structure is `L1141`, data is `L1142`, event data is `L1198`** — a missing key or an unknown
  `ease` against a non-positive `duration`, an out-of-range or non-increasing `t`, an empty channel, a
  trackless clip, a non-positive `scale`, a rotation segment of 180° or more on an axis or combined;
  an empty `events:`, an out-of-range or non-increasing event `t`, a blank event `name`, or `events`
  on a `generated_from` clip.
  A clip is validated when its assembly is checked; the diagnostic is prefixed
  ``clip `name` (path)``.
- **Track names are resolved at playback, not at check time.** A track naming a part the assembly does
  not have is simply not applied — that is what makes one humanoid clip reusable across models.

## Playback

`Animation` is a built-in component: `clip` (the short name, default `""`), `started` (the
`FrameIndex` playback is measured from, default `0`), `speed` (default `1.0`, `>= 0`), `hold` (ticks
each pose is shown, default `1`, `>= 1`).

- **The pose is visual-only, a hard line.** The pose is a pure function of `(clip, sim time)`
  derived at the render boundary and written back nowhere. The entity keeps its one `Transform` and
  collision footprint; pausing freezes the pose. No limb hitboxes. Once a game registers
  `animation_events`, clip timing is sim input (`ludion docs character-animation`).
- **`hold: N` steps the pose** every `N` ticks ("on twos"); assemblies, prefab clips and flipbooks all
  honour it. See `ludion docs character-animation` §Stepped playback.
- **Write the component only when the choice changes.** Re-stamping `started` every frame restarts
  the clip every frame (and churns the world hash for a `String` that did not change). The component
  is ordinary hashed sim state: it snapshots, and it rides a scene transition.
- **An unbound name is a silent rest pose** — no clip, an empty name, or a name the assembly does
  not bind — with no diagnostic: a complaint would fire sixty times a second.
- **A plain `clip` write is a hard cut.** `crossfade(&mut animation, clip, seconds, ctx.game_frame)`
  blends from the outgoing clip (rotation slerps, translation/scale lerp).
- `speed: 0.0` freezes on the clip's first instant; a `started` frame in the future reads as that same
  first instant rather than a negative time.

### What the evaluator does

Elapsed clip seconds are `(now − started) × seconds_per_tick × speed`, **derived from tick counts on
every frame and never accumulated** — which is why `started` is a frame index and not seconds, and
why clip time is always a whole number of sim ticks. At 60 Hz that quantization is 1/60 s; the posed
sheet reports the exact tick and seconds it used, so `--time 0.401` renders tick 24 and says `t 0.4`.

Per part, in the assembly's authored order:

```text
local_i = T(offset_i − offset_parent + translation_i) · pivot_i · R_i · S_i · pivot_i⁻¹
world_i = world_parent · local_i
```

`S_i` is identity when the part has no `scale` channel.

- **A part translates in its REST frame, then rotates about its pivot.** The ordinary `T · R` order,
  so a `translation` key is never bent by that same part's own rotation — a torso authored to bob `+Y`
  bobs along +Y however far it has turned.
- **The chain composes down to the root**, so a rotated upper arm carries its forearm and anything
  parented to it; subtracting the parent's rest offset stops it translating twice.
- **A part with no track is exactly the identity**; the rest pose survives composition bit for bit.
- **`mirror: x` is consumed at meshing**; nothing at pose time re-mirrors.

### From game code

`cargo run -p ludion-engine --example assembly_clip` (`crates/ludion-engine/examples/assembly_clip.rs`)
prints an animated arm's origin beside a resting one at four ticks. In a game the write is a system's:

```rust
fn drive_animation(world: &mut World, ctx: &SystemCtx) {
    for (mover, animation) in world.query::<(&Mover, &mut Animation)>() {
        let wanted = if mover.speed > 0.0 { "walk" } else { "idle" };
        // The guard is the whole pattern: assigning `started` every frame restarts the clip
        // every frame, so the model never leaves its first pose.
        if animation.clip != wanted {
            animation.clip = wanted.to_string();
            animation.started = ctx.game_frame;
        }
    }
}
```

A ragdoll folding back into its assembly crossfades from a captured `PoseSnapshot` instead of a clip
(`switch_ragdoll_to_assembly`, `ludion docs voxel-assemblies` §Ragdolls); a later `crossfade()` retires it.

### Layers

An `AnimationLayers` component plays a second clip over the base `Animation` on some parts only
(`ludion docs animation-layers` has the component and the mask spelling). An assembly's masks name
**parts**, and `descendants: true` walks the parent links, so a child authored before its parent is fine:

```yaml
masks:
  arm_only:
    joints:
      - arm
```

```rust
use ludion::prelude::*;

// The scene lists a `wave` layer (clip `wave`, mask `arm_only`) at weight 0.
fn wave(layers: &mut AnimationLayers, now: SimTime) {
    fade_layer(layers, "wave", 1.0, 0.2, now);
}
```

- **Override** blends each channel the layer clip keys toward the layer by `weight × mask` (verbatim
  at 1). A channel it does not key keeps the base, and a part with no track in it is untouched.
- **Additive** measures from the part's **rest**, because a rigid part's sample already is its change
  from rest; `reference: <clip>` measures from that clip's first frame instead.
- Layers read the base `hold` grid and are presentation only: `switch_to_ragdoll` samples the base
  pose alone. `events: true` fires the layer clip's events with `ClipEvent.layer` set.
- A layer `clip` may name a blend (below); its phase lives in `BlendClock.layers`.
- `render --model --layer` previews a stack, and `check-scene` lists the masks (step 3 above).
- `examples/gallery/assembly-layers.yaml` shows a blend, a mid-fade override, an additive lean and a layer blend on `voxels/humanoid.assembly.yaml`.

### Blend spaces

A `blends:` map in the assembly document mixes point clips by a parameter — a walk-to-run space, a
strafe cross. `Animation.clip`, a crossfade's `from` or an `AnimationLayer.clip` may name a blend;
`Animation.params` (a layer's own `params`) drives it.

```yaml
clips:
  idle: anims/idle.anim.yaml
  run: anims/run.anim.yaml
  walk: anims/walk.anim.yaml
blends:
  locomotion:
    sync: foot_l
    points:
      - clip: idle
        at: 0
      - clip: walk
        at: 2
      - clip: run
        at: 5
```

- **Points.** Two to sixteen `clip` (a name bound under `clips:`) and `at`: a number is a 1D point read
  from `params[0]`, a block list of two numbers a 2D point read from `params[0..2]`. One dimension per
  blend, unique positions, a point clip must loop, and a blend may not share a clip binding's name.
- **Pose.** Weights are linear (1D) or barycentric (2D, at most three active points). Parts blend in
  weight order: rotation by running slerp, translation and scale by weighted sum, a channel no active
  clip keys left unset so an untouched part keeps its exact rest. A query on a point is that clip alone,
  bit for bit.
- **Phase.** All point clips advance on one shared phase. With `sync: foot_l` each clip is offset so its
  own `foot_l` event lands on phase 0 (an assembly clip has no events overlay, so the event is in the
  clip document). The rate is `Animation.speed * sum(weight / duration)` cycles per gameplay second.
- **Events.** The heaviest point fires its events as the phase crosses them: `foot_l` and `foot_r` fire
  once per stride at every speed, with `ClipEvent.clip` the blend's name.
- **Registration.** Register `blend_clocks` after whatever writes `Animation` or `AnimationLayers` and
  before `animation_events` (`ludion docs animation-layers`). It keeps a hashed `BlendClock` only on an
  entity whose clip, crossfade source or layer names a blend; without it extract falls back to
  `elapsed * rate(params now)` and no events fire.
- `switch_to_ragdoll` samples a blended base at the switch instant; layers stay excluded.
- `render --model --blend` sweeps the parameters with no `BlendClock`: a sheet's phase is the stateless
  `elapsed * rate(params)`, which is what an entity without a clock draws.

### IK

An assembly's `ik:` names two-part chains and look-ats; an `IkTargets` component aims them. Extract
solves after the clip, crossfade and layers, so it is presentation only: the targets are hashed, the pose
never is.

```yaml
ik:
  hand_r:                       # a chain: parts (upper, then its child) + effector
    parts:
      - upper-arm-r
      - lower-arm-r
    effector:                   # part-local, unmirrored voxels on the second part
      - 1.0
      - 0.0
      - 1.0
    hinge:                      # optional: axis in the upper part's frame, bend 1 or -1
      axis:
        - 1.0
        - 0.0
        - 0.0
      bend: 1
  look:                         # a look-at: aim = a `masks:` entry, forward default -Z
    aim: head_look
    limits_deg:                 # yaw and pitch, each in (0, 180], default 180
      yaw: 70.0
      pitch: 40.0
```

- A chain has `parts` and `effector`, a look-at has `aim`; both or neither is `L1137`, as are a third
  part and `ground` (not supported). `L1138`: an unknown part or mask, a second part that is not a child
  of the first, or no `hinge:` where the second part has a non-hinge `joints:` entry or is not named
  `lower-leg-*`/`lower-arm-*`.
- The hinge is `hinge:`, else the second part's `joints:` hinge (its limits clamp the flex), else axis +X
  with bend −1 for `lower-leg-*` and +1 for `lower-arm-*` (the humanoid convention).
- `IkTargets.targets` maps an entry name to one of `point` (world) or `direction` (`Look`: +yaw turns
  left, +pitch looks down, relative to `forward`), plus `weight` in `[0, 1]` that ramps over `fade`
  seconds (`fade_ik_target`, no write when unchanged). Both or neither is `L1673`. A direction on a chain is
  ignored. To follow an entity, write `point` from its world position each tick it moves.
- Order: every look-at (name order), then every chain. A look-at turns its aim parts root-first, each by its
  mask weight's share of the total; the yaw and pitch of the target clamp to `limits_deg`. A chain bends the
  second part about the hinge to the right reach, then swings the first part to point at the target. A
  target out of reach leaves the limb straight or folded, aimed at it (no pole vector). `weight` blends from
  the animated pose to the solved one.
- No `IkTargets`, no matching entry or weight 0 poses byte for byte as without IK. Not in the ragdoll
  switch or `kind: character` skins; the model sheet solves it only for `--look`/`--ik`.
- `check-scene <assembly>` lists each entry: `ik hand_r: chain upper-arm-r -> lower-arm-r, effector [..],
  hinge [..] bend +1` (the hinge the solver uses, a defaulted one included) and `ik look: look-at head_look,
  forward [..], yaw ±70, pitch ±40`. `--json` adds `"ik"` only when there are entries.
- `render --model x.assembly.yaml --clip idle --look look=0..100,0 --strip 6` sweeps a look-at's yaw (a number
  or `lo..hi` per side, `Look`'s convention, `NAME=` optional with one look-at) across the strip's phases,
  every one at `--time`; `--ik hand_r=6,12,2` (repeatable) reaches a chain to a point **in the assembly's grid
  voxels**, the frame `place:` is written in. Both need `--clip`, `--crossfade` or `--blend` (not a `--blend`
  range with a `--look` range), assemblies only. Each phase prints `look [yaw, pitch]` and keeps the gap
  analysis, so "the elbow tears out of its plug" is text. Misuse is `L1117`.

### Live editing

Under `ludion dev` (or any attached viewer) saving the assembly, **any** part, or **any** bound clip
re-reads and re-meshes it, so a clip edit re-poses a running game with no restart. Clip bytes sit
outside the mesh content hash, so nothing is re-uploaded to the GPU.

## Gotchas

- **Author keyframe times on 60 Hz tick multiples** (`0.05`, `0.1`, `0.25`). A `t` between ticks is
  never sampled exactly; the pose you designed is not the pose that renders.
- **`--strip` shows the *arc*; `--time` shows one pose at full sheet size**, which is what you read a
  silhouette from. Strip first, then the instant.
- **The per-phase component count is the gap analysis, and it is conservative.** Each rest cell is
  carried into its posed frame as a *cube*, so a rotating limb never reports itself detached and a
  separation of a full cell or more still reports; a narrower hairline opening is not caught.
- **Hinge direction has no checkable guard.** A knee bent the wrong way validates, meshes, connects,
  and reads as a broken puppet; the axis conventions in `ludion docs voxel-assemblies` are the only defence.
- **`--clip` refuses what it cannot pose** with `L1117`: a `.voxel.yaml`/`.mesh.yaml` model (one rigid
  piece), a name the model does not bind (the message lists the ones it does), a lineup, and
  `--time`/`--strip` with no `--clip`, `--crossfade` or `--blend` at all. A malformed `--crossfade`,
  `--blend` or `--layer` spec is `L1117` too, as is a `--layer` with no base or an unknown blend, clip
  or mask name.

## Next

- `ludion docs voxel-assemblies` — the model the clips address: parts, pivots, the humanoid names.
- `ludion docs character-animation` — skinned characters: the `kind: character` document, clips, `crossfade`, `AnimationEvents`.
- `ludion docs sprite-animation` — the same `Animation` component over a `Sprite`'s atlas flipbook.
- `ludion docs prefab-animation` — imported rigid node clips on a prefab instance.
- `ludion docs components` — `Animation`'s row, and every other built-in component.
- `ludion docs voxel-sheets` — the sheet flags `--clip` composes with.
- `ludion docs dev-loop` — the live rebuild and reload loop a clip edit rides.
