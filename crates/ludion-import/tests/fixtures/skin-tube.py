"""skin-tube: the minimal skinned importer fixture.

A 2 m capped cylinder along +Y (8 rings of 12 vertices), skinned to two bones, `lower` (root) and
`upper` (child of `lower`). The four middle rings blend both bones; the outer rings are rigid.
One skin, JOINTS_0/WEIGHTS_0 only, no animations, no morph targets.

Run headless:  <pinned blender> -b --factory-startup --python skin-tube.py -- skin-tube.glb
Run over MCP:  set sys.argv = ["blender", "--", "<out.glb>"], then exec this file.
"""
import bpy

if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"skin-tube.py is pinned to Blender 5.2, this is {bpy.app.version_string}")

import math
import sys

if bpy.app.background:
    bpy.ops.wm.read_factory_settings(use_empty=True)
else:
    for block in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.armatures):
        for item in list(block):
            block.remove(item)
    for coll in list(bpy.data.collections):
        bpy.data.collections.remove(coll)

scene = bpy.context.scene
scene.name = "skin-tube"
export = bpy.data.collections.new("export")
scene.collection.children.link(export)

HEIGHT, RADIUS, RINGS, SIDES = 2.0, 0.2, 8, 12

# ---- armature: Blender +Z becomes Ludion +Y ---------------------------------------------------
arm_data = bpy.data.armatures.new("skin-tube-rig")
arm = bpy.data.objects.new("skin-tube-rig", arm_data)
export.objects.link(arm)
bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="EDIT")
lower = arm_data.edit_bones.new("lower")
lower.head, lower.tail = (0.0, 0.0, 0.0), (0.0, 0.0, HEIGHT / 2)
upper = arm_data.edit_bones.new("upper")
upper.head, upper.tail = (0.0, 0.0, HEIGHT / 2), (0.0, 0.0, HEIGHT)
upper.parent, upper.use_connect = lower, True
bpy.ops.object.mode_set(mode="OBJECT")

# ---- mesh, built vertex by vertex so face order is fixed ---------------------------------------
verts, faces, upper_w = [], [], []
for r in range(RINGS):
    z = HEIGHT * r / (RINGS - 1)
    t = min(max((z - 0.5) / 1.0, 0.0), 1.0)  # rigid below 0.5 m and above 1.5 m
    for s in range(SIDES):
        a = 2.0 * math.pi * s / SIDES
        verts.append((RADIUS * math.cos(a), RADIUS * math.sin(a), z))
        upper_w.append(t * t * (3.0 - 2.0 * t))
for r in range(RINGS - 1):
    for s in range(SIDES):
        a, b = r * SIDES + s, r * SIDES + (s + 1) % SIDES
        faces.append((a, b, b + SIDES, a + SIDES))
bottom, top = len(verts), len(verts) + 1
verts += [(0.0, 0.0, 0.0), (0.0, 0.0, HEIGHT)]
upper_w += [0.0, 1.0]
top_ring = (RINGS - 1) * SIDES
for s in range(SIDES):
    faces.append((bottom, (s + 1) % SIDES, s))
    faces.append((top, top_ring + s, top_ring + (s + 1) % SIDES))

mesh = bpy.data.meshes.new("skin-tube")
mesh.from_pydata(verts, [], faces)
mesh.shade_smooth()

mat = bpy.data.materials.new("tube")
mat.use_backface_culling = True  # otherwise the exporter writes doubleSided (L1202)
bsdf = mat.node_tree.nodes["Principled BSDF"]
bsdf.inputs["Base Color"].default_value = (0.5, 0.5, 0.5, 1.0)
bsdf.inputs["Metallic"].default_value = 0.0
bsdf.inputs["Roughness"].default_value = 0.75
bsdf.inputs["Alpha"].default_value = 1.0
mesh.materials.append(mat)

obj = bpy.data.objects.new("skin-tube", mesh)
export.objects.link(obj)
obj.parent = arm
groups = {name: obj.vertex_groups.new(name=name) for name in ("lower", "upper")}
for i, w in enumerate(upper_w):
    if w < 1.0:
        groups["lower"].add([i], 1.0 - w, "REPLACE")
    if w > 0.0:
        groups["upper"].add([i], w, "REPLACE")
mod = obj.modifiers.new("rig", "ARMATURE")
mod.object = arm

tris = sum(len(p.vertices) - 2 for p in mesh.polygons)
print(f"skin-tube: {len(verts)} vertices, {tris} triangles")

# ---- export ----------------------------------------------------------------------------------
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(argv) != 1:
    raise SystemExit("usage: blender -b --factory-startup --python skin-tube.py -- <out.glb>")
layer = bpy.context.view_layer
layer.active_layer_collection = layer.layer_collection.children["export"]
bpy.ops.export_scene.gltf(
    filepath=argv[0],
    export_format="GLB",
    export_yup=True,
    use_active_collection=True,
    use_active_collection_with_nested=False,
    export_apply=True,
    export_image_format="AUTO",
    export_materials="EXPORT",
    export_texcoords=True,
    export_normals=True,
    export_tangents=False,
    export_vertex_color="NONE",
    export_attributes=False,
    export_cameras=False,
    export_lights=False,
    export_extras=False,
    export_animations=False,
    export_skins=True,
    export_influence_nb=4,
    export_all_influences=False,
    export_def_bones=False,
    export_leaf_bone=False,
    export_hierarchy_flatten_bones=False,
    export_armature_object_remove=False,
    export_rest_position_armature=True,
    export_morph=False,
    export_gpu_instances=False,
    export_draco_mesh_compression_enable=False,
    export_meshopt_compression_enable=False,
)
print(f"skin-tube: wrote {argv[0]}")
