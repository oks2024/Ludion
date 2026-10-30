# Builds examples/import-cutout/scene.gltf in Blender 5.2: a mask grating (MASK + alphaCutoff), a
# flap and a tape strip (doubleSided), and an open lamp shade (doubleSided), for the cutout and
# double-sided import path (Clockout request 0010). Regenerate with:
#
#   blender --background --factory-startup --python examples/import-cutout/make-fixture.py
#
# Deterministic inputs (no randomness, stable names); the exporter's output is not byte-gated, the
# committed `expected/` tree is.

import math
import os

import bpy

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scene.gltf")
CUTOFF = 0.25  # exact in f32, so the document writes `alpha_cutoff: 0.25`

bpy.ops.wm.read_factory_settings(use_empty=True)
for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)

bpy.context.preferences.addons["io_scene_gltf2"].preferences.allow_embedded_format = True


def make_material(name, culling):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    mat.use_backface_culling = culling
    return mat


def bars_image():
    size = 32
    img = bpy.data.images.new("grating-bars", size, size, alpha=True)
    pixels = []
    for y in range(size):
        for x in range(size):
            d = min(min(x % 8, 8 - x % 8), min(y % 8, 8 - y % 8))
            a = 1.0 if d <= 1 else 0.0
            pixels += [0.8, 0.55, 0.25, a]
    img.pixels = pixels
    img.pack()
    return img


def mask_grating(mat):
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    bsdf = nodes["Principled BSDF"]
    tex = nodes.new("ShaderNodeTexImage")
    tex.image = bars_image()
    links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    less = nodes.new("ShaderNodeMath")
    less.operation = "LESS_THAN"
    links.new(tex.outputs["Alpha"], less.inputs[0])
    less.inputs[1].default_value = CUTOFF
    sub = nodes.new("ShaderNodeMath")
    sub.operation = "SUBTRACT"
    sub.inputs[0].default_value = 1.0
    links.new(less.outputs["Value"], sub.inputs[1])
    links.new(sub.outputs["Value"], bsdf.inputs["Alpha"])


def tint(mat, rgb):
    mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (*rgb, 1.0)


def finish(obj, name, mat):
    obj.name = name
    obj.data.name = name
    obj.data.materials.append(mat)
    return obj


bpy.ops.mesh.primitive_plane_add(size=2.0, location=(-3.0, 0.0, 0.0))
grating = finish(bpy.context.active_object, "grating", make_material("grating", True))
mask_grating(grating.data.materials[0])

bpy.ops.mesh.primitive_plane_add(size=2.0, location=(0.0, 0.0, 1.0), rotation=(math.radians(35), 0.0, 0.0))
flap_mat = make_material("flap", False)
tint(flap_mat, (0.2, 0.6, 0.7))
finish(bpy.context.active_object, "flap", flap_mat)

bpy.ops.mesh.primitive_plane_add(size=1.0, location=(3.0, 0.0, 0.0))
tape = bpy.context.active_object
tape.scale = (0.15, 3.0, 1.0)
bpy.ops.object.transform_apply(scale=True)
tape_mat = make_material("tape", False)
tint(tape_mat, (0.9, 0.8, 0.2))
finish(tape, "tape", tape_mat)

bpy.ops.mesh.primitive_cylinder_add(
    vertices=12, radius=0.8, depth=1.2, end_fill_type="NOTHING", location=(6.0, 0.0, 0.0)
)
shade_mat = make_material("shade", False)
tint(shade_mat, (0.9, 0.5, 0.4))
finish(bpy.context.active_object, "shade", shade_mat)

bpy.ops.export_scene.gltf(
    filepath=OUT,
    export_format="GLTF_EMBEDDED",
    export_apply=True,
    export_cameras=False,
    export_lights=False,
    export_yup=True,
)
print("wrote", OUT)
