---
description: Bone masks on a character and AnimationLayers — masked override and additive clips over the base pose
---

# Animation layers

Play a second clip over the base `Animation` on part of the body — carry a body over a run, aim over
a walk, lean around a corner. Two pieces: a **mask** in the `kind: character` document says which
joints a layer touches, and an **`AnimationLayers`** component says which clip plays under which mask
and how strongly. Base playback, crossfade and events stay in `ludion docs character-animation`.

## Masks

```yaml
format: 1
kind: character
mesh: ../meshes/guard.mesh.yaml
clips:
  run:
    clip: ../clips/run.anim.yaml
  carry:
    clip: ../clips/carry.anim.yaml
masks:
  upper_body:
    joints:
      - torso
    descendants: true
  head_look:
    joints:
      - spine
      - neck
      - head
    weights:
      - 0.2
      - 0.3
      - 0.5
```

- `joints` lists skin joints (block list; at least one, each once). `descendants: true` makes every
  joint under a listed one inherit its nearest listed ancestor's weight. Every other joint weighs 0.
- `weights` is one number in `[0, 1]` per listed joint, in order; omitted means 1.
- A joint the mesh lacks is `L1200` (pointer `/masks/<name>/joints/<i>`, help names the nearest joint);
  a malformed mask or a weight outside `[0, 1]` is `L1199`.

## Layers

```yaml
components:
  Animation:
    clip: run
  AnimationLayers:
    layers:
      - name: carry
        clip: carry
        mask: upper_body
        weight: 1.0
        fade: 0.2
```

Layers evaluate in list order over the base pose; at most four (`L1670`).

| field | default | meaning |
|---|---|---|
| `name` | required | unique per component; `fade_layer`/`play_layer` select by it (`L1671`) |
| `clip` | `""` | a clip bound in the character's `clips:` |
| `mask` | `""` | a `masks:` key; empty is the whole skin |
| `mode` | `override` | `override` or `additive` |
| `weight` | `0` | target weight in `[0, 1]` (`L1671` outside it) |
| `from_weight`, `changed`, `fade` | `0`, frame `0`, `0` s | the ramp: from `from_weight` at `changed` over `fade` seconds |
| `started`, `speed` | frame `0`, `1.0` | the layer clip's own clock; it does not restart with the base |

- **Override** blends each joint toward the layer clip by `weight × mask`; a joint the layer clip has
  no track for is untouched, and at exactly 1 the layer's channels are taken verbatim.
- **Additive** adds the layer clip's change from its own first frame (`t = 0`) on top of the pose
  beneath: a lean clip whose first frame is upright adds only the lean.
- The weight ramp and the layer clock follow the base `Animation.hold` grid.

Steer a layer from game code, writing only when the choice changes:

```rust
use ludion::prelude::*;

fn aim_on(layers: &mut AnimationLayers, now: SimTime) {
    play_layer(layers, "carry", "aim", now);
    fade_layer(layers, "carry", 1.0, 0.2, now);
}
```

Pass `ctx.game_time`. Both return `false` when no layer has that name; `fade_layer` to the current
target and `play_layer` of the current clip write nothing.

## What it does not do

- **Presentation only.** No system reads the component; the pose is never hashed, and a scene without
  it poses and hashes exactly as before. The component itself is hashed state, like `Animation`.
- An unknown clip or mask name, or a layer at weight 0, is a silent degrade — the layer does nothing.
- A skin named by `SkinBone` parts refuses `AnimationLayers` (`L1672`); assemblies and prefab parts
  ignore it.
- Layer clips raise no `AnimationEvents` yet.
- `ParentJoint` sockets and `posed_joint_world` read the layered pose.

## Next

- `ludion docs character-animation` — the base clip, crossfade, events and sockets.
- `ludion docs components` — the `AnimationLayers` row.
