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

The prefab gains an optional `clips:` map, short name to clip file, resolved against the prefab file (like a nested `prefab:` path). It is absent when empty, so every prefab without clips is byte-identical. A track is keyed by **part name**; its keys are the node's absolute local TRS from the glTF.

| Import case | Result |
|---|---|
| translation/rotation/scale channel on a node that became a part | a track on that part |
| `STEP` interpolation | `ease: step` keys |
| `CUBICSPLINE` | resampled to linear keys within the reduce tolerance (`L1206` if it cannot) |
| morph weights, a channel on a node with no part, an animation mixing skin joints and parts | `L1202` (import aborts unless `--allow-lossy`) |

## Rules

- **The pose is relative to the part's authored rest.** The playback transform is stored local × rest⁻¹ × sampled, so the placement baked into the root part and any `transform`/override ride on top; an unplaced part takes the sample exactly. Exact for uniform scale.
- **`Animation` goes on the root part** (`PrefabInstance.root` equals the entity). Put it in an `overrides:` entry, or write it after `spawn_prefab`. The `clip` name resolves through the instanced prefab's `clips:` map.
- **A root whose `Mesh` is an assembly or a character plays that model's clips**, not the prefab's: the prefab's parts stay at rest (the rule `animation_events` follows).
- **Two instances animate independently**, each with its own `Animation`.
- **A track must name a part: `L1314`** from `check-scene` of the prefab or of any scene placing it, and from wire `scene.check` (`the clip snap animates lidd, which is not a part of this prefab`), with the nearest part name as the fix. Playback draws an unmatched track's part at rest, silently, so run the check.
- **Visual only.** Posing happens at extract; the sim `Transform`, the `WorldHash` and recordings are unchanged. Children of a posed part follow, and so do point and spot lights (in position — a spot's aim is its world-space `direction`, not the part's rotation), rendered reflection probes, decals, particles and drop shadows under it; **colliders, cameras and the sun do not**, and the `probes` debug lines stay at the stored pose — move an analytic collider from game code.
- **`animation_events` needs one more query.** A game registering that system declares `.query::<(&Animation, &PrefabInstance)>()` or the query-drift gate reports `L1126`.
- **Profiling:** the load report gains a `prefab_clips` row (`ludion devdocs profiling`).

## The model sheet

`render --model <prefab>` renders the prefab **alone**, framed by its rest-pose bounds, with `--clip`, `--time`, `--strip` and `--crossfade` as for an assembly. There is no lineup, `--set` or `--sweep` (the clip resolves relative to the prefab file; override the scene that places the instance instead) and no gap analysis. A prefab that draws no mesh is `L1117`; so is `--clip` with a name the prefab does not bind (the error lists the clips it does).

## Gotchas

- **A generated prefab is never hand-edited.** Change the Blender action and re-import; `prefab.apply` on it is `L1237`.

## Next

- `ludion docs prefabs` — the prefab file and `spawn_prefab`.
- `ludion docs prefab-instances` — `overrides:` and instance entries.
- `ludion docs voxel-animation` — strip flags and the clip document.
- `ludion docs character-animation` — `crossfade`, `AnimationEvents`, skinned clips.
- `ludion docs blender-craft` — exporting animations from Blender.
