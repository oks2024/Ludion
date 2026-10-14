The measurement log of `Plan/asset-pipeline-blender.md` (decisions 13–15). There is one row per brief,
written at acceptance and never rewritten. Each brief's text is sealed below before its subagent
starts.

| asset | phase | MCP calls | Blender screenshots | sheet renders | tokens | wall-clock | human looks | import clean | triangles | follow-up edit iterations | headless replay byte-identical | verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| idol (vigil) | 1 | 5 (4 `execute_blender_code`, 1 window screenshot) | 1 | 4 `render --model` sheets of the idol (+1 of the sculpt baseline); 12 `render --scene` stage renders | 231 k (task usage, 110 tool calls) | 18.4 min | 0 (the user's look is pending) | yes, on the first import: no `--allow-lossy`, no diagnostics, scale and facing correct | 4 844 (body 4 588, eye 168, plinth 88) | 1 (`WING_SCALE = 1.2`, kept) | yes, after two fixes (friction 1): glb and imported tree, and MCP = headless | **rejected** (user, 2026-09-27: "does not look good"); attempt ignored, the idol restarts from scratch, and this row does not count toward the verdict |

## Baselines

| route | asset | attempts | tokens | source |
|---|---|---|---|---|
| sculpt | idol | 10 sheet renders | ~191 k | `Plan/artifacts/mesh-modeling-log.md` |
| ComfyUI | idol | 5 sheets (36 candidates) | 142 k | `Plan/artifacts/comfy-generation-log.md` |
| Blender over MCP (earlier projects) | per mesh | ~15 | ~2 M | `Plan/mesh-modeling.md` decision 17 |

## Sealed briefs

### 1. Idol (vigil, head-to-head; sealed 2026-09-26)

This is the subject the sculpt and ComfyUI routes were given (`Plan/mesh-modeling.md` decision 2; the
prompt of `games/vigil/generation/recipes/idol-portrait.gen.yaml`):

- **Subject:** a small weathered stone idol, a crouching winged creature, squatting on a low dark
  plinth with a flat square top.
- **The head:** exactly one eye, a single glowing green eye set deep under a heavy overhanging brow
  ridge. The rest of the face is bare stone, with no other eyes. A mass of short strands hangs over
  its mouth.
- **The wings:** two bat wings, raised and spread clear of the back, with a visible gap between each
  wing and the body.
- **Size and facing:** the same overall size and facing as the sculpt idol
  (`examples/gallery/sculpts/idol.sculpt.yaml`, read with `check-scene --bounds`), so the three can be
  judged on one stage.
- **Materials:** flat factor materials, no textures: a dark stone body, a darker plinth, and one
  emissive green eye (the sculpt idol's palette is the reference).
- **Budget:** at most 5 000 triangles.

## Tool-friction list (ranked, grown by every brief)

1. **`bmesh.ops.create_uvsphere` and `bmesh.ops.solidify` change face order per process** (idol). The
   first headless replay was not byte-identical (same vertices, same triangle set, permuted indices)
   while three exports inside one MCP session hashed identical, so the defect is invisible in the
   interactive loop. Four probe runs localized it. Fixed by building spheres and thickened sheets by
   hand; recorded in `ludion docs blender-craft`. A phase-3 drift gate must compare two fresh
   processes, never two exports from one session.
2. **No coloured six-view sheet for a flat-factor, multi-object import** (idol). `render --model`
   takes one `.mesh.yaml`, which carries no base colour, so the sheet shows the body alone, in white;
   colour and the glow need a hand-built scene with a camera and lights. A `render --model` over an
   imported `scene.yaml` (every entity with its `Mesh` factors, the six views) would close this.
3. **Stage-scene authoring** (idol): flow-style lists are refused (block style only),
   `Camera.projection` is `type: perspective` plus `fov_y_deg`, a sculpt is `shape: { sculpt: ... }`
   and not `asset:`, and a 180-degree-yaw camera needs a positive pitch to look down. Each cost one
   render round; every diagnostic named its fix.
4. **Emissive proportionality** (idol; avoided by reading `ludion devdocs ludion-import` first):
   Blender's emission colour times strength must equal `k × base` within 8 ULP. Dyadic factors with
   strength 1.0 make it exact.
5. **`mat.use_nodes` is deprecated in 5.2** (a warning only).
6. **The exporter writes a channel for every keyed (bone, property)** (mannequin clips): keying all
   bones every frame produced constant rest tracks (11 tracks, a 2.5x larger file) that
   `export_optimize_animation_keep_anim_armature=False` did not drop. The script now keys only what
   leaves rest.
7. **`export_frame_range` and slotted actions**: the Action's manual range (`use_frame_range`) plus
   `export_anim_slide_to_zero` gave the intended frame counts; an Action left in
   `arm.animation_data.action` after the last bake would be exported twice, so clear it first (mannequin clips).
8. **Actions survive the base build's datablock sweep**: a re-run in one MCP session exports
   `idle.001` unless the script removes every `bpy.data.actions` entry first (mannequin clips).

## Per-brief notes

### 1. Idol

- Setup: the sculpt idol's bounds (plinth ±0.45 × 0.306, cap to 0.395, top 1.536, front −Z), the
  import dev-doc (the emissive rule), the unit check (gravity −9.81 on Y, so metres). The exporter's
  enum values were read over MCP: 5.2 has no `PNG` image format, so `AUTO` plus an assert.
- Build 1 (2 844 triangles): imported clean on the first try, `emissive: 3.0` exact. Sheet: facing
  right; two cheek spheres read as two extra eyes, the brow read as a cap visor, the torso leaned
  backward (the X-rotation sign misread), the wings were flat sticks.
- Build 2: cheeks replaced by a muzzle, brow reworked. Staged the three routes on the sculpt
  gallery's key and fill rig with `render --scene`, after three rounds of scene-format errors.
- Build 3 (3 432): the pose rewritten as a gargoyle crouch: forward lean, raised knees, hands planted
  on the plinth with claws. It reads as a gargoyle from here on.
- Builds 4 to 6: Catmull-Rom tubes, parallel-transport frames (the arms had twisted panels), joint
  masses at shoulder, elbow and knee (4 844 triangles), darker stone factors. The stage rig is hot
  enough that the 0.13 stone still reads pale; the factors follow the brief.
- MCP check: the unchanged script run with `exec` over MCP, plus the one window screenshot.
- Follow-up edit "make the wings 20 % larger": one edit (`WING_SCALE = 1.2`), one re-run, one
  re-import, so 1 iteration. Height 1.506 to 1.610 (sculpt: 1.536), wing span 1.17 to 1.36.
  **Kept:** the larger wings read better at stage distance; the overall height is 5 % over the
  sculpt's.
- Headless replay: the first measurement was **not** byte-identical (friction 1). With hand-built
  spheres and membranes, two fresh headless runs, the MCP run and the committed tree all match (glb
  sha256 `df5cea93…`). Build 9 flat-shaded the membranes (smooth normals streaked their edges).
- Measured by the harness: 231 172 tokens, 110 tool calls, 18.4 min (the author estimated ~45 min).
- Deleted after the rejection (2026-09-27), so the restart starts clean: the script, provenance and imported tree named below no longer exist. To look at (historical): `ludion render --scene games/vigil/scenes/meshes/idol-blender/scene-lit.yaml` (the
  idol on the sculpt gallery's rig, from its front) and the body sheet
  `ludion render --model games/vigil/scenes/meshes/idol-blender/meshes/body.mesh.yaml`. The author's
  read: placeholder quality. The silhouette is clear (one glowing eye under a heavy brow, strands,
  raised wings clear of the back, crouched on a dark plinth), but it is built from primitives (an egg
  torso, tube limbs), with less surface detail than the ComfyUI idol and a stronger pose than the
  sculpt idol.
