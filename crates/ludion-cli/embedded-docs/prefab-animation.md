---
description: Rigid glTF node animation on a prefab — import clips, preview, play on an instance's root part
---

# Prefab animation

**Scope:** a glTF whose Blender actions move whole nodes (a hinge, a lid, a door) imports as a `kind: prefab` with a `clips:` map; an instance plays a clip by carrying `Animation` on its root part. Covers import, `check-scene`, the model sheet, playback rules and what does not follow a posed part. The `Animation` fields are `ludion docs components-fx`; the clip format is `ludion docs voxel-animation` (assembly clips, the same document); skinned meshes are `ludion docs character-animation`.

## The loop

1. Export from Blender with animations on (`ludion docs blender-craft`) and import: `ludion import` of the glTF writes `model.prefab.yaml`, `meshes/` and `clips/<name>.anim.yaml`.
2. Check: `ludion check-scene model.prefab.yaml` lists each clip (`clip open: clips/open.anim.yaml, duration <d>, loop <bool>, <k> tracks`) and refuses a track naming no part.
3. Look: `ludion render --model model.prefab.yaml --clip snap --strip 4` writes `model-snap-strip.png`; `--clip open --time 0.5` poses one frame; `--crossfade open:snap:0.25` renders a blend. The runnable fixture is `examples/import-node-clips`.
4. Place an instance in a scene with the clip on its **root part** (the prefab's first entity):

   ```yaml
   - name: hinge-1
     prefab: prefabs/hinge/model.prefab.yaml
     overrides:
       base:
         Animation:
           clip: open
   ```

5. From game code, `crossfade(&mut animation, "snap", 0.25, ctx.game_frame)` on that same component (`ludion docs character-animation`).

## Format

The prefab gains an optional `clips:` map, short name to clip file, resolved against the prefab file (like a nested `prefab:` path). It is absent when empty, so every prefab without clips is byte-identical (`masks:` and `blends:` below likewise). A track is keyed by **part name**; its keys are the node's absolute local TRS from the glTF.

| Import case | Result |
|---|---|
| translation/rotation/scale channel on a node that became a part | a track on that part |
| `STEP` interpolation | `ease: step` keys |
| `CUBICSPLINE` | resampled to linear keys within the reduce tolerance (`L1206` if it cannot) |
| morph weights, a channel on a node with no part, an animation mixing skin joints and parts | `L1202` (import aborts unless `--allow-lossy`) |

## Rules

- **The pose is relative to the part's authored rest.** The playback transform is stored local × rest⁻¹ × sampled, so the placement baked into the root part and any `transform`/override ride on top; an unplaced part takes the sample exactly. Exact for uniform scale.
- **`Animation` goes on the root part** (`PrefabInstance.root` equals the entity). Put it in an `overrides:` entry, or write it after `spawn_prefab`. The `clip` name resolves through the instanced prefab's `clips:` map.
- **A root whose `Mesh` is an assembly (voxel or skinned) plays that model's clips**, not the prefab's: the prefab's parts stay at rest (the rule `animation_events` follows).
- **Two instances animate independently**, each with its own `Animation`.
- **A track must name a part: `L1314`** from `check-scene` of the prefab or of any scene placing it, and from wire `scene.check` (`the clip snap animates lidd, which is not a part of this prefab`), with the nearest part name as the fix. Playback draws an unmatched track's part at rest, silently, so run the check.
- **Visual only.** Posing happens at extract; the sim `Transform`, the `WorldHash` and recordings are unchanged. Children of a posed part follow, and so do point and spot lights (in position — a spot's aim is its world-space `direction`, not the part's rotation), rendered reflection probes, decals, particles and drop shadows under it; **colliders, cameras and the sun do not**, and the `probes` debug lines stay at the stored pose — move an analytic collider from game code.
- **`animation_events` and `blend_clocks` need one more query.** A game registering either system declares `.query::<(&Animation, &PrefabInstance)>()` for it or the query-drift gate reports `L1126`.
- **Root motion.** A `$root` track targets no part (never `L1314`): `root_motion_delta` reads it on the root part, in prefab-local units (`ludion docs voxel-animation` §Root motion).
- **Profiling:** the load report gains a `prefab_clips` row (`ludion devdocs profiling`).

## Layers and blends

A hand-authored prefab takes `masks:` and `blends:` in the assembly's spelling (`ludion docs animation-layers`), written after `clips:`. A mask's `joints` name the prefab's own parts (not a nested instance or its parts); `descendants: true` follows each part's `Parent`. A nested instance's part weighs 0 under any mask and 1 without one.

```yaml
masks:
  forearm:
    joints:
      - lower
    descendants: true
blends:
  sway:
    points:
      - clip: swing
        at: 0.0
      - clip: reach
        at: 1.0
```

- **Use.** The root part's `Animation.clip` (or `from`) may name a blend, with `params`; an `AnimationLayers` on the same entity poses the parts, with or without an `Animation`.
- **Composition** is the assembly's, over each sample's difference from the part's authored rest (turn `rest⁻¹ · key`, offset `key − rest`, scale `key / rest`).
- **Override** takes the layer's channels by `weight × mask`; **additive** adds the layer's difference from rest, so a key equal to the rest adds nothing.
- **`reference:`** measures an additive layer from that clip's first frame instead of the rest.
- An unknown clip, blend, mask or reference leaves the pose as if the layer were absent.
- **Phase.** Register `blend_clocks` (`ludion docs animation-layers` §Blend spaces) and the root keeps a hashed `BlendClock`, so a cadence follows `params` without a pose jump. It declares both `.query::<(&Animation, &Mesh)>()` and `.query::<(&Animation, &PrefabInstance)>()`, then `.query::<(&mut BlendClock,)>()`; leaving one out is `L1126`. Without the system a blend plays `elapsed * rate(params now)`: exact while the params hold still; changing them mid-loop jumps the phase. A root that plays no blend gets no clock.
- **Events.** With `animation_events` registered after it, a base blend fires its dominant point's events (`ClipEvent.clip` is the blend's name), and an `events: true` layer, clip or blend, fires its own with `ClipEvent.layer` set, even when `Animation.clip` is empty (the entity still needs an `Animation`). Blend events need `blend_clocks`; clip layers do not.
- **Root motion** stays plain-clip: `root_motion_delta` reads no prefab blend.
- **Check:** `ludion check-scene examples/gallery/prefabs/arm/arm.prefab.yaml` (a checkout's runnable example) lists `mask` and `blend` rows after the clips. `L1226` is a malformed entry; `L1674` an unknown mask part or an unusable blend.

## The model sheet

`render --model <prefab>` renders the prefab **alone**, framed by its rest-pose bounds, with `--clip`, `--time`, `--strip`, `--crossfade`, `--blend` and `--layer` as for an assembly (`--blend sway:0..1 --layer curl:1:forearm`; the sheet's blend phase is `elapsed * rate(params)`, so it matches a game without a `BlendClock`). There is no lineup, `--set` or `--sweep` (the clip resolves relative to the prefab file; override the scene that places the instance instead) and no gap analysis. A prefab that draws no mesh is `L1117`; so is `--clip`, `--blend` or `--layer` with a name the prefab does not declare (the error lists the ones it does).

## Gotchas

- **A generated prefab is never hand-edited.** Change the Blender action and re-import; `prefab.apply` on it is `L1237`.

## Next

- `ludion docs prefabs` — the prefab file and `spawn_prefab`.
- `ludion docs prefab-instances` — `overrides:` and instance entries.
- `ludion docs voxel-animation` — strip flags and the clip document.
- `ludion docs character-animation` — `crossfade`, `AnimationEvents`, skinned clips.
- `ludion docs blender-craft` — exporting animations from Blender.
