---
description: Masks, AnimationLayers, blend spaces and the AnimGraph state graph for skins and voxel assemblies
---

# Animation layers

Play a second clip over the base `Animation` on part of the body — carry a body over a run, aim over
a walk, lean around a corner. Two pieces: a **mask** in the skinned `kind: assembly` document says which
joints a layer touches, and an **`AnimationLayers`** component says which clip plays under which mask
and how strongly. Base playback, crossfade and events stay in `ludion docs character-animation`.

## Masks

```yaml
format: 1
kind: assembly
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
- A joint the mesh lacks is `L1138` (pointer `/masks/<name>/joints/<i>`, help names the nearest joint);
  a malformed mask or a weight outside `[0, 1]` is `L1137`.

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
| `params` | `[0, 0]` | the two values a blend named by `clip` is evaluated at (`L1671` when not finite) |
| `reference` | `""` | an additive layer's reference clip; empty measures from the layer's own first frame |
| `events` | `false` | the layer fires its clip's events, tagged `ClipEvent.layer` |

- **Override** blends each joint toward the layer clip by `weight × mask`; a joint the layer clip has
  no track for is untouched, and at exactly 1 the layer's channels are taken verbatim.
- **Additive** adds the layer clip's change from its own first frame (`t = 0`) on top of the pose
  beneath: a lean clip whose first frame is upright adds only the lean.
- With `reference: idle` an additive layer measures its change from the `idle` clip's first frame
  instead; an unknown reference skips the layer.
- The weight ramp and the layer clock follow the base `Animation.hold` grid.
- **Layer blends.** A layer `clip` that is not a clip may name a blend of the character, driven by the
  layer's own `params` (never `Animation.params`) and `set_layer_params(&mut layers, "lean", [x, 0.0])`,
  which writes only on change. Its phase lives in `BlendClock.layers`, one entry per such layer,
  integrated at the layer's `speed` and restarted when its `started` changes (`play_layer` to a new
  clip). Register `blend_clocks` as under Blend spaces; layers need no extra query line. An additive
  blend layer without `reference` measures from each point's first frame under the current weights.
- **Layer events.** `events: true` makes a layer fire its clip's events while its weight is above 0.
  Each `ClipEvent` carries the layer's name in `layer` (`""` for the base clip) and the layer's clip
  or blend in `clip`; an entity's base events come first, then its layers' in list order. A plain clip
  is walked statelessly over the tick's gameplay window, so a scaled game clock fires each event once.
  Register whatever calls `play_layer` before `animation_events`: a layer started after it loses only
  an event at exactly `t = 0`.

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

## Blend spaces

A `blends:` map in the skinned assembly document (or a voxel one, `ludion docs voxel-animation`) mixes point clips by a parameter — a walk-to-run space, a
strafe cross. `Animation.clip` (and a crossfade's `from`) may name a blend; `Animation.params` drives it.

```yaml
clips:
  idle: { clip: clips/idle.anim.yaml, events: [{ t: 0.5, name: foot_l }, { t: 1.5, name: foot_r }] }
  walk: { clip: clips/walk.anim.yaml }
  run:  { clip: clips/run.anim.yaml }
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

- **Points.** Two to sixteen `clip` (a binding name) and `at`. `at: 4` is a 1D point read from
  `params[0]`; an `at` block list of two numbers is a 2D point read from `params[0..2]`. Every point of a
  blend has the same dimension and a unique position. A point clip must loop.
- **Weights.** 1D is linear between the sorted neighbours and clamps at the ends. 2D is barycentric
  over a deterministic triangulation of the points (every non-degenerate triple with an empty
  circumcircle, taken in index order, skipping overlaps) and clamps to the hull. A query on a point
  gives that clip alone, exactly.
- **Phase.** All point clips advance on one shared phase in cycles. With `sync: foot_l`, each clip is
  offset so its `foot_l` lands on phase 0 (without `sync`, its start does). The rate is
  `Animation.speed * sum(weight * binding speed / duration)` per gameplay second, so a cadence follows
  the params with no pose jump.
- **Events.** The point with the largest weight (ties to the lower index) fires its events as the
  phase crosses them, so `foot_l` and `foot_r` fire once per stride at every speed. `ClipEvent.clip`
  is the blend's name. A crossfade's outgoing side never emits.
- **Registration.** Register `blend_clocks` after whatever writes `Animation` and before
  `animation_events`, declaring `.query::<(&Animation, &Mesh)>()`,
  `.query::<(&Animation, &PrefabInstance)>()` and `.query::<(&mut BlendClock,)>()`.
  It keeps a hashed `BlendClock` only on an entity whose clip names a blend. Without it, extract falls
  back to `elapsed * rate(params now)`: exact while the params hold still, and no events fire.
- **Limits.** `hold` does not step a blend's phase. A blend may not share a clip binding's name.
  A layer blend's phase is not stepped by `hold`. A prefab's blends follow these rules too
  (`ludion docs prefab-animation`).
- **Assemblies.** An assembly's `blends:` follows the same rules, with its clips bound at speed 1 and
  `sync` read from the clip document's own events; errors are `L1137`/`L1138`.

Errors: `L1137` for a malformed `blends:` entry, `L1138` for a duplicate position, a name clash,
collinear 2D points, an unbound or non-looping point clip, or a `sync` event a point clip lacks.

## State graph

An `AnimGraph` is a `static` table in your game's Rust: states, edges and `fn` conditions. It is not
an asset and not a pose source — it only chooses which clip and layers to write, and only when an edge
fires, so the hash stays quiet between transitions.

```rust
use ludion::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pose { Idle, Carry, Wave }
struct Inputs { carrying: bool, wave: bool }

static GRAPH: AnimGraph<Pose, Inputs> = AnimGraph {
    states: &[
        AnimState { id: Pose::Idle, name: "idle", plays: "idle", layers: &[], one_shot: false },
        AnimState { id: Pose::Carry, name: "carry", plays: "idle", layers: &[("carry", "carry", 1.0)], one_shot: false },
        AnimState { id: Pose::Wave, name: "wave", plays: "wave", layers: &[], one_shot: true },
    ],
    edges: &[
        AnimEdge { from: None, to: Pose::Wave, when: |i| i.wave, fade: 0.0, gate: AnimGate::Interrupts },
        AnimEdge { from: Some(Pose::Idle), to: Pose::Carry, when: |i| i.carrying, fade: 0.2, gate: AnimGate::None },
        AnimEdge { from: Some(Pose::Carry), to: Pose::Idle, when: |i| !i.carrying, fade: 0.2, gate: AnimGate::None },
        AnimEdge { from: Some(Pose::Wave), to: Pose::Idle, when: |_| true, fade: 0.15, gate: AnimGate::OnEvent("end") },
    ],
};
// In a system registered after `animation_events`: GRAPH.step(world, entity, &inputs, ctx)?
```

- **Initial state.** There is no implicit entry. Spawn `AnimGraphState { state: "idle", .. }` (scene
  YAML or Rust) beside an `Animation` already playing that state's clip. The state is recorded by its
  stable `name`, not an index.
- **`step(world, entity, &inputs, ctx)`** walks `edges` in declaration order; the first candidate whose
  `when` and gate pass is taken. An edge is a candidate when `from` is `None` or the current state, and
  `to` is not the current state (no self-transitions: a `from: None` edge never restarts its own
  target). Gates: `None`; `Interrupts`; `OnEvent(name)` (fired this tick on the current state's clip);
  `MinTicks(n)` (gameplay ticks since the state was entered).
- **One-shots.** While the current state is `one_shot` and its clip's `end` has not fired, only
  `Interrupts` edges qualify, so a one-shot finishes unless something interrupts it.
- **Entering** a state calls `crossfade` to its clip (even when the clip is unchanged: a state restarts
  it) and rewrites `AnimGraphState`. Each `(layer, clip, weight)` it lists is played and faded to its
  weight over the edge's `fade`; a layer the old state listed and the new one does not fades to `0`.
  `step` returns `Ok(Some(new))`, or `Ok(None)` having written nothing.
- **Errors write nothing.** `L4290`: no `AnimGraphState` or `Animation`, an unknown state name (the
  help names the nearest), or a layer the entity's `AnimationLayers` lacks. `L4291`: duplicate state
  names or ids, or an edge naming an undeclared state. `describe()` prints the table as text.
- `current(world, entity)` reads the id of the state the entity is in.

## What it does not do

- **The pose is presentation only.** It is never hashed, and a scene without the component poses and
  hashes exactly as before. The component is hashed state, like `Animation`; `blend_clocks` and
  `animation_events` read it for layer blends and events.
- An unknown clip or mask name, or a layer at weight 0, is a silent degrade — the layer does nothing.
- A skin named by `SkinBone` parts refuses `AnimationLayers` (`L1672`). On a prefab instance's
  root it poses the parts (`ludion docs prefab-animation` §Layers and blends).
  An assembly takes layers, plain or blend, with its own `masks:` (`ludion docs voxel-animation`);
  `ludion render --model --blend/--layer` previews both and `check-scene` lists them.
- A layer raises no `AnimationEvents` unless it sets `events: true`.
- IK over assembly parts (`ik:`, `IkTargets`) is separate, solved after layers: `ludion docs voxel-animation` §IK (`render --model --look/--ik` previews it).
- `ParentJoint` sockets and `posed_joint_world` read the layered pose.
- Root motion is the base's `$root` tracks only, a blend's points at their weights; layers contribute none (`ludion docs voxel-animation` §Root motion).

## Next

- `ludion docs character-animation` — the base clip, crossfade, events and sockets.
- `ludion docs voxel-animation` — layers on an assembly: part masks, override and additive rules.
- `ludion docs components` — the `AnimationLayers` row.
