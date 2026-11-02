---
description: Authoring a mesh in Blender from a committed bpy script — export settings, axes, import traps
---

# Blender craft

**Scope:** making a game mesh in Blender 5.2 LTS (driven over its MCP extension or headless) so that
`ludion import` takes it clean: the script contract, the export settings, the unit and axis
convention, the traps that break an import or a replay, and the verification loop. What the imported
tree contains is `ludion docs materials` (the `Mesh` scalars and maps) and `ludion docs scene-format`;
the ComfyUI route to a mesh is `ludion docs generation-craft-mesh`.

## The loop

```
<pinned blender> -b --factory-startup --python generation/blender/<asset>/build.py -- target/tmp/<asset>.glb
ludion import target/tmp/<asset>.glb --out scenes/meshes/<asset>      # no --allow-lossy, ever
ludion check-scene scenes/meshes/<asset>/meshes/<part>.mesh.yaml --bounds
ludion render --model scenes/meshes/<asset>/meshes/<part>.mesh.yaml    # six-view sheet, one part
ludion render --scene scenes/meshes/<asset>/scene-lit.yaml --out target/tmp/<asset>-stage.png
```

To cook the imported tree's maps, name its directory in `[cook] roots` (`ludion sync` does the cook;
see Derived imports).

When the props are placed in a scene, `ludion check-scene <scene> --placement` flags the floating,
sunk and collider-overlapping ones (`ludion docs scene-format`, "Placement lint").

The script is the source; the `.glb` is scratch (never committed) and the imported tree is derived —
never hand-edited. Change the script, re-run, re-import. A headless run takes ~3 s, so it is the
fast loop; a Blender screenshot over MCP is only worth taking when a Ludion render cannot answer the
question. The idol (4 844 triangles) took nine build → import → render iterations and one Blender screenshot.

### Derived imports

A project that commits its glTF exports declares them in `ludion.toml`: `[[import]]` with `from`
(a glob, `*` inside one path segment, e.g. `art/kits/*/export/*.gltf`) and `to` (the derived root).
`ludion sync` compiles each `<dir>/<stem>.gltf` into `<to>/<stem>/` as `ludion import` would, skipping
the kits whose files and the importer are unchanged. Freshness compares each source file's size
and modification time (ns) with the record first and reads content only on a mismatch, so an edit
that keeps the size and restores the old mtime reads as fresh; deleting the `to` tree
forces a full import. The `to` tree is ignored by version control and
never hand-edited; `scripts/check.mjs` and CI run the sync before the tests. A `to` holding files the
sync did not write is refused (`L1245`), as is a symbolic link or junction under a source directory, or two stems that differ only in case. A
`to` inside a `from` directory (the glob's part before its first `*`) is refused (`L0023`): the sync
would rewrite files the next one hashes.
After the import, `ludion sync` also cooks every PNG map the imported meshes reference into compressed
KTX2 files in `.ludion/cooked/<target>/` (BC7 for `desktop`; `--target ios` or `[cook] targets` adds ASTC), keyed by content so only a changed map re-cooks and
an unreferenced file is swept. `[[import]] max_texture_size` caps a kit's map size; `--no-cook` skips the stage. A map that cannot be cooked fails
the sync (`L1270`) but never the import.
Trees the sync does not derive (a `ludion import` tree, a hand-placed or `mesh apply` one) cook with `[cook] roots = ["scenes/assets"]`: every `*.mesh.yaml` below a root (dot entries skipped) is read and its maps cook uncapped, beside the import trees, with no `[[import]]` table. A root overlapping an `[[import]] to` is refused (`L0023`), an absent root or a link under it is `L1324`, and a map leaving its root is not cooked (`L1325`).
A second sync of the same root waits for the first, saying so on stderr, and a sync killed midway recovers on the next run.

## The script skeleton

```python
import bpy
if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"<asset>/build.py is pinned to Blender 5.2, this is {bpy.app.version_string}")
import math, sys
import bmesh
from mathutils import Matrix, Vector

if bpy.app.background:
    bpy.ops.wm.read_factory_settings(use_empty=True)
else:  # over MCP: purge the live session instead of resetting the UI under the add-on
    for block in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.images):
        for item in list(block):
            block.remove(item)
    for coll in list(bpy.data.collections):
        bpy.data.collections.remove(coll)
scene = bpy.context.scene
scene.name = "<asset>"                        # becomes the imported prefab's name:
export = bpy.data.collections.new("export")
scene.collection.children.link(export)
# ... build objects with bmesh, link them into `export` ...
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
```

Over MCP the same file runs unchanged: set `sys.argv = ["blender", "--", "<out.glb>"]`, then `exec` it.
The idol's MCP export and its headless export were byte-identical.

## Export settings

Copy this call verbatim; every argument is explicit so an exporter default change cannot move output:

```python
layer = bpy.context.view_layer
layer.active_layer_collection = layer.layer_collection.children["export"]
bpy.ops.export_scene.gltf(
    filepath=argv[0], export_format="GLB", export_yup=True,
    use_active_collection=True, use_active_collection_with_nested=False,
    export_apply=True, export_image_format="AUTO", export_materials="EXPORT",
    export_texcoords=True, export_normals=True, export_tangents=False,
    export_vertex_color="NONE", export_attributes=False, export_cameras=False,
    export_lights=False, export_extras=False, export_animations=False,
    export_skins=False, export_morph=False, export_gpu_instances=False,
    export_draco_mesh_compression_enable=False, export_meshopt_compression_enable=False)
```

An animated asset flips two of those arguments: `export_animations=True, export_extras=True` (the
skin needs `export_skins=True` as well). `ludion import` then writes one `clips/<name>.anim.yaml`
per Action, with tracks named after the joints. A rigid animation (whole nodes, no skin) imports as prefab clips instead: `ludion docs prefab-animation`. Two custom properties on the Action steer it and
ride out as glTF extras: `loop` (bool, default false) and, only when the defaults refuse a clip
with `L1206`, the tolerances `bound_rotation_deg`, `bound_translation` and `bound_scale` (the
`reduce_*` twins keep more keys). An unknown key is `L1207`; `ludion explain L1206` has the numbers.

`export_image_format` has no `PNG` value in 5.2 (`AUTO`, `JPEG`, `WEBP`, `NONE`): `AUTO` keeps a PNG
source as PNG, so the script asserts every `bpy.data.images` entry is PNG before exporting.

## Units, axes, materials

- **1 Blender unit = 1 Ludion unit = 1 m** (the physics default gravity is `-9.81` on Y). No scale
  factor anywhere; the idol's plinth imported at exactly its authored 0.9 × 0.39 × 0.9.
- **Model facing Blender +Y, up +Z.** The +Y-up export maps Blender `(x, y, z)` to `(x, z, −y)`, so
  +Y becomes Ludion's **−Z front**, the view `render --model` labels `FRONT -Z`.
- **One object per flat-factor material.** A `kind: mesh` document carries no base colour, so each
  object becomes one entity whose `Mesh.color`/`roughness`/`metallic`/`emissive` come from its
  material's factors. A multi-material object fans out into child entities instead.
- **Every material:** backface culling on for closed meshes, off for sheets (`use_backface_culling =
  False` exports `doubleSided`, which imports as `double_sided: true`); Principled `Metallic` and
  `Roughness` set explicitly, `Alpha` 1.0 unless cutting holes. Do not set `mat.use_nodes` —
  deprecated in 5.x, it only warns.
- **Cutout holes:** the image's `Alpha` output feeds the Principled `Alpha` input through a threshold
  (`LESS_THAN` against the cutoff, then `1 −` that), which the exporter reads as glTF `MASK` with that
  `alphaCutoff`; pick a cutoff exact in f32 (`0.25`) so the document writes it unchanged. Plain alpha
  blend is `BLEND`, which is `L1202`. `examples/import-cutout/make-fixture.py` is the working script.
- **Emissive:** any Emission Color works. A glTF `emissiveFactor` equal to `k × baseColorFactor` maps
  straight to the scalar `Mesh.emissive`; any other factor is baked into an emissive map by the
  import (needs UVs; every base channel the glow uses must be ≥ 1/255 — a black base channel under a
  coloured glow is `L1202`). A strength above 1 moves into `KHR_materials_emissive_strength`, which
  the import multiplies into `Mesh.emissive`. For the proportional path, keep `k × base ≤ 1` per
  channel and choose dyadic values (`0.0625, 0.3125, 0.09375` × 3) so the product is exact in f32; the
  import then writes `emissive: 3.0`. For a textured mesh the glow is `Mesh.color × emissive × emissive map`
  (glTF's rule — the base map is never read), so the emissive map must carry the glow colour: a grey
  mask glows white.

## Gotchas

- **Backface culling is off by default in Blender**, so a closed mesh exports `doubleSided` and draws
  unculled unless you turn it on. A thin surface (a wing membrane, a leaf) keeps it off and needs no
  thickened shell.
- **`BLEND` is `L1202`.** Use `MASK` for holes.
- **A material with no metallic factor imports fully metallic** (the glTF default is 1.0).
- **A referenced JPEG image is `L1202`.**
- **`bmesh.ops.create_uvsphere` and `bmesh.ops.solidify` break the headless replay.** Their face order
  changes from process to process (the idol's glb and imported indices differed run to run with the
  same vertices and the same triangle set), while repeated exports inside one session are identical —
  so the defect hides in an interactive loop. Build spheres and thickened sheets vertex by vertex;
  `create_cube` and `bevel` were stable. Verify with two headless runs and `cmp` on the two glbs.
  After the fix the idol replays byte-identical: glb, and the imported tree.
- **Smooth shading across a deliberately thickened mesh** blends front, back and rim normals into dark
  streaks at the edges. Tag the rim faces and keep them flat (`face.smooth = smooth and not face.tag`).
- **Tube frames twist** when the reference vector is swapped mid-curve. Pick the first frame from a
  fixed axis and parallel-transport it (project the previous `u` onto each new normal plane).
- **`render --model` shows one mesh document, uncoloured** — a flat-factor asset's colour and glow
  live in the scene that instances `model.prefab.yaml` (`ludion docs prefabs`). Judge shape on the sheet; judge colour and the emissive on a scene with a
  camera and lights (`scene-lit.yaml` beside the import, hand-authored, outside the derived files).
- **A scene camera with 180° yaw needs a positive pitch to look down** (`rotation: [8, 180, 0]`).
- **Budget:** each 16 × 10 ellipsoid costs ~288 triangles and 7 short strand tubes ~800; overlapping
  primitives keep their hidden interior faces. `L1112` warns at 10 000 vertices, not triangles.

## Next

- `ludion docs materials` — what the imported `Mesh` factors and maps do to the render.
- `ludion docs generation-craft-mesh` — the ComfyUI route to a textured mesh.
- `ludion docs sculpt-authoring` — the in-engine implicit-surface route.
