"""skin-mannequin: the humanoid skinned importer fixture.

A ~1.1 m low-poly mannequin in an A-pose, one mesh object skinned to an 11-bone rig: pelvis (root),
torso, head, upper/lower arms and legs, `_l`/`_r`. Weights are scripted per ring: rings at a joint
blend the two bones 50/50, the rest are rigid, so no vertex has more than two influences.
One opaque culled material, one skin, JOINTS_0/WEIGHTS_0 only, no animations, no morph targets.
The figure faces Blender +Y (Ludion -Z), so its left side is Blender -X.

Run headless:  <pinned blender> -b --factory-startup --python skin-mannequin.py -- skin-mannequin.glb
Run over MCP:  set sys.argv = ["blender", "--", "<out.glb>"], then exec this file.
skin-mannequin-clips.py reuses the build above (SKIN_MANNEQUIN_BUILD_ONLY skips the export).
"""
import bpy

if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"skin-mannequin.py is pinned to Blender 5.2, this is {bpy.app.version_string}")

import math
import sys

from mathutils import Vector

if bpy.app.background:
    bpy.ops.wm.read_factory_settings(use_empty=True)
else:
    for block in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.armatures):
        for item in list(block):
            block.remove(item)
    for coll in list(bpy.data.collections):
        bpy.data.collections.remove(coll)

scene = bpy.context.scene
scene.name = "skin-mannequin"
export = bpy.data.collections.new("export")
scene.collection.children.link(export)

# ---- joint positions (Blender Z up) ---------------------------------------------------------
HIP_Z, WAIST_Z, NECK_Z, TOP_Z = 0.52, 0.62, 0.86, 1.10
ARM_DOWN = math.radians(45.0)  # A-pose: arms 45 degrees below horizontal
UPPER_ARM, LOWER_ARM = 0.20, 0.19


def side_points(sign):
    """Shoulder, elbow, wrist, hip, knee, ankle for one side (sign -1 = left, +1 = right)."""
    shoulder = Vector((sign * 0.15, 0.0, 0.82))
    d = Vector((sign * math.cos(ARM_DOWN), 0.0, -math.sin(ARM_DOWN)))
    hip = Vector((sign * 0.08, 0.0, HIP_Z))
    return {
        "shoulder": shoulder, "arm_dir": d,
        "elbow": shoulder + d * UPPER_ARM, "wrist": shoulder + d * (UPPER_ARM + LOWER_ARM),
        "hip": hip, "knee": Vector((sign * 0.08, 0.0, 0.28)), "ankle": Vector((sign * 0.08, 0.0, 0.04)),
    }


SIDES = {"l": side_points(-1.0), "r": side_points(1.0)}

# ---- armature ---------------------------------------------------------------------------------
arm_data = bpy.data.armatures.new("skin-mannequin-rig")
arm = bpy.data.objects.new("skin-mannequin-rig", arm_data)
export.objects.link(arm)
bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="EDIT")
bones = {}


def bone(name, head, tail, parent=None, connect=False):
    b = arm_data.edit_bones.new(name)
    b.head, b.tail = head, tail
    if parent is not None:
        b.parent, b.use_connect = bones[parent], connect
    bones[name] = b


bone("pelvis", (0.0, 0.0, HIP_Z), (0.0, 0.0, WAIST_Z))
bone("torso", (0.0, 0.0, WAIST_Z), (0.0, 0.0, NECK_Z), "pelvis", True)
bone("head", (0.0, 0.0, NECK_Z), (0.0, 0.0, TOP_Z), "torso", True)
for s in ("l", "r"):
    p = SIDES[s]
    bone(f"upper_arm_{s}", p["shoulder"], p["elbow"], "torso")
    bone(f"lower_arm_{s}", p["elbow"], p["wrist"], f"upper_arm_{s}", True)
for s in ("l", "r"):
    p = SIDES[s]
    bone(f"upper_leg_{s}", p["hip"], p["knee"], "pelvis")
    bone(f"lower_leg_{s}", p["knee"], p["ankle"], f"upper_leg_{s}", True)
BONE_NAMES = list(bones)
bpy.ops.object.mode_set(mode="OBJECT")

# ---- mesh, built vertex by vertex so face order is fixed ---------------------------------------
verts, faces, weights = [], [], []


def tube(points, radii, ring_weights, sides, depth=1.0):
    """A capped tube through `points`. Rings lie in the plane spanned by Y and tangent x Y, which
    never twists because every limb lies in the XZ plane. `depth` scales the Y extent."""
    y = Vector((0.0, 1.0, 0.0))
    first = len(verts)
    n = len(points)
    for i, (c, r, w) in enumerate(zip(points, radii, ring_weights)):
        t = (points[min(i + 1, n - 1)] - points[max(i - 1, 0)]).normalized()
        x = y.cross(t)
        for k in range(sides):
            a = 2.0 * math.pi * k / sides
            verts.append(tuple(c + r * math.cos(a) * x + r * depth * math.sin(a) * y))
            weights.append(w)
    for i in range(n - 1):
        for k in range(sides):
            a, b = first + i * sides + k, first + i * sides + (k + 1) % sides
            faces.append((a, b, b + sides, a + sides))
    bottom, top = len(verts), len(verts) + 1
    verts.extend([tuple(points[0]), tuple(points[-1])])
    weights.extend([ring_weights[0], ring_weights[-1]])
    last = first + (n - 1) * sides
    for k in range(sides):
        faces.append((bottom, first + (k + 1) % sides, first + k))
        faces.append((top, last + k, last + (k + 1) % sides))


def sphere(center, radius, segs, rings, w):
    """A UV sphere by hand: bmesh.ops.create_uvsphere's face order is not stable across runs."""
    first = len(verts)
    for i in range(1, rings):
        phi = math.pi * i / rings - math.pi / 2.0
        for k in range(segs):
            a = 2.0 * math.pi * k / segs
            verts.append((center[0] + radius * math.cos(phi) * math.cos(a),
                          center[1] + radius * math.cos(phi) * math.sin(a),
                          center[2] + radius * math.sin(phi)))
            weights.append(w)
    for i in range(rings - 2):
        for k in range(segs):
            a, b = first + i * segs + k, first + i * segs + (k + 1) % segs
            faces.append((a, b, b + segs, a + segs))
    bottom, top = len(verts), len(verts) + 1
    verts.extend([(center[0], center[1], center[2] - radius),
                  (center[0], center[1], center[2] + radius)])
    weights.extend([w, w])
    last = first + (rings - 2) * segs
    for k in range(segs):
        faces.append((bottom, first + (k + 1) % segs, first + k))
        faces.append((top, last + k, last + (k + 1) % segs))


def blend(a, b):
    return {a: 0.5, b: 0.5}


# Torso: hips to neck, a little flatter front to back.
tube([Vector((0.0, 0.0, z)) for z in (0.44, HIP_Z, WAIST_Z, 0.72, 0.82, 0.90)],
     [0.11, 0.12, 0.11, 0.12, 0.13, 0.05],
     [{"pelvis": 1.0}, {"pelvis": 1.0}, blend("pelvis", "torso"), {"torso": 1.0}, {"torso": 1.0},
      blend("torso", "head")],
     sides=8, depth=0.7)
sphere((0.0, 0.0, 0.99), 0.11, segs=8, rings=6, w={"head": 1.0})
for s in ("l", "r"):
    p = SIDES[s]
    d, up, lo = p["arm_dir"], f"upper_arm_{s}", f"lower_arm_{s}"
    tube([p["shoulder"] - d * 0.03, p["shoulder"], p["shoulder"] + d * (UPPER_ARM / 2), p["elbow"],
          p["elbow"] + d * (LOWER_ARM / 2), p["wrist"], p["wrist"] + d * 0.06],
         [0.045, 0.045, 0.04, 0.035, 0.033, 0.03, 0.035],
         [blend("torso", up), {up: 1.0}, {up: 1.0}, blend(up, lo), {lo: 1.0}, {lo: 1.0}, {lo: 1.0}],
         sides=6)
for s in ("l", "r"):
    p = SIDES[s]
    up, lo = f"upper_leg_{s}", f"lower_leg_{s}"
    down = Vector((0.0, 0.0, -1.0))
    tube([p["hip"] - down * 0.03, p["hip"], (p["hip"] + p["knee"]) / 2, p["knee"],
          (p["knee"] + p["ankle"]) / 2, p["ankle"], p["ankle"] + down * 0.04],
         [0.06, 0.06, 0.055, 0.045, 0.042, 0.038, 0.04],
         [blend("pelvis", up), {up: 1.0}, {up: 1.0}, blend(up, lo), {lo: 1.0}, {lo: 1.0}, {lo: 1.0}],
         sides=6)

mesh = bpy.data.meshes.new("skin-mannequin")
mesh.from_pydata(verts, [], faces)
mesh.shade_smooth()

mat = bpy.data.materials.new("mannequin")
mat.use_backface_culling = True  # otherwise the exporter writes doubleSided (L1202)
bsdf = mat.node_tree.nodes["Principled BSDF"]
bsdf.inputs["Base Color"].default_value = (0.6, 0.45, 0.35, 1.0)
bsdf.inputs["Metallic"].default_value = 0.0  # glTF default is 1.0: always explicit
bsdf.inputs["Roughness"].default_value = 0.8
bsdf.inputs["Alpha"].default_value = 1.0
mesh.materials.append(mat)

obj = bpy.data.objects.new("skin-mannequin", mesh)
export.objects.link(obj)
obj.parent = arm
groups = {name: obj.vertex_groups.new(name=name) for name in BONE_NAMES}
for i, w in enumerate(weights):
    for name, value in w.items():
        groups[name].add([i], value, "REPLACE")
mod = obj.modifiers.new("rig", "ARMATURE")
mod.object = arm

tris = sum(len(p.vertices) - 2 for p in mesh.polygons)
print(f"skin-mannequin: {len(verts)} vertices, {tris} triangles, {len(BONE_NAMES)} bones")

# ---- export ----------------------------------------------------------------------------------
if not globals().get("SKIN_MANNEQUIN_BUILD_ONLY", False):
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(argv) != 1:
        raise SystemExit("usage: blender -b --factory-startup --python skin-mannequin.py -- <out.glb>")
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
    print(f"skin-mannequin: wrote {argv[0]}")
