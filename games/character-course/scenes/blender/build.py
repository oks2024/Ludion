"""The character course: static greybox geometry, the two moving-part paths, the lane path and the camera rail.

    blender -b --factory-startup --python build.py -- course.glb

Geometry is authored in Ludion axes (x along the lane, y up, z across) and converted per vertex to
Blender's (x, -z, y); the glTF export's +Y-up conversion then returns it unchanged. Every solid is a
prism over an (x, y) profile, so each face is written by hand and the output never depends on a
bmesh operator's face order. The `collision` object is the same solids with no bevels. Next to the
glb it writes `../paths/platform.path.yaml`, `../paths/press.path.yaml` and `../paths/lane.path.yaml` and `../paths/camera-behind.path.yaml` (every coordinate on a
1/64 grid, so Python's `repr` equals the canonical writer's shortest round trip).
"""
import bpy
if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"character-course/build.py is pinned to Blender 5.2, this is {bpy.app.version_string}")
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
out_glb = argv[0] if argv else os.path.join(HERE, "course.glb")

if bpy.app.background:
    bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.name = "course"
export = bpy.data.collections.new("export")
scene.collection.children.link(export)

BOTTOM = -0.5            # every slab's underside
Z_LANE = 1.5             # the lane is z in [-1.5, 1.5]
STEP = 0.24              # the first step, just under the hero step_height of 0.25
RAMP_TOP = 1.328125      # 3 m run at 20 degrees above the step, on the 1/64 grid
TOP = RAMP_TOP           # the lane's raised level
CLEAR = 0.75             # tunnel headroom: a 0.9 m capsule is blocked, a 0.6 m crouch fits

HIGH = TOP + 1.09375     # the lane section's upper level, on the 1/64 grid
Z_SECTION = 4.0          # the lane section is z in [-4, 4]

solids = {"floor": [], "walls": []}


def prism(group, profile, z0, z1):
    solids[group].append((profile, z0, z1))


def box(group, x0, x1, y0, y1, z0, z1):
    prism(group, [(x0, y0), (x1, y0), (x1, y1), (x0, y1)], z0, z1)


# --- the main lane (z in [-1.5, 1.5]) ---------------------------------------------------------
# spawn pad (wings join both side rows), a 0.2 step, a 20 degree ramp, the raised run with the
# tunnel, a 1.5 m pit, a landing pad, a belt gap (the conveyor is a scene entity), the end pad.
prism("floor", [(-2.0, BOTTOM), (15.0, BOTTOM), (15.0, TOP), (9.0, TOP), (6.0, STEP), (4.0, STEP), (4.0, 0.0), (-2.0, 0.0)],
      -Z_LANE, Z_LANE)
box("floor", -2.0, 4.0, BOTTOM, 0.0, Z_LANE, 5.5)
box("floor", -2.0, 4.0, BOTTOM, 0.0, -5.5, -Z_LANE)
box("floor", 16.5, 19.0, BOTTOM, TOP, -Z_LANE, Z_LANE)
box("floor", 25.0, 29.0, BOTTOM, TOP, -Z_LANE, Z_LANE)
# the lane section past the end pad: flat, a ramp up, a raised run, a pit, a raised run, a drop, the
# belt gap (a scene entity), the exit pad
prism("floor", [(29.0, BOTTOM), (38.5, BOTTOM), (38.5, HIGH), (36.0, HIGH), (33.0, TOP), (29.0, TOP)],
      -Z_SECTION, Z_SECTION)
prism("floor", [(40.0, BOTTOM), (45.0, BOTTOM), (45.0, TOP), (42.0, TOP), (42.0, HIGH), (40.0, HIGH)],
      -Z_SECTION, Z_SECTION)
box("floor", 48.0, 52.0, BOTTOM, TOP, -Z_SECTION, Z_SECTION)
# the tunnel over x in [11.5, 14.5]: a ceiling slab and two walls
box("walls", 11.5, 14.5, TOP + CLEAR, TOP + CLEAR + 0.25, -Z_LANE, Z_LANE)
box("walls", 11.5, 14.5, TOP, TOP + CLEAR + 0.25, -Z_LANE, -Z_LANE + 0.25)
box("walls", 11.5, 14.5, TOP, TOP + CLEAR + 0.25, Z_LANE - 0.25, Z_LANE)

# --- row A (z in [2.5, 5.5]): a 0.4 step and a 55 degree slope the motor must refuse ------------
box("floor", 4.0, 14.0, BOTTOM, 0.0, 2.5, 5.5)
box("walls", 6.0, 9.0, 0.0, 0.4, 2.5, 5.5)
prism("walls", [(10.0, 0.0), (12.0, 0.0), (12.0, 2.859375)], 2.5, 5.5)

# --- row B (z in [-5.5, -2.5]): a pit crossed by a path platform, a turntable, a press ----------
box("floor", 8.0, 12.0, BOTTOM, 0.0, -5.5, -2.5)
box("floor", 15.0, 23.0, BOTTOM, 0.0, -5.5, -2.5)


def to_blender(x, y, z):
    return (x, -z, y)


def build_mesh(name, items, material):
    verts, faces = [], []
    for profile, z0, z1 in items:
        base = len(verts)
        n = len(profile)
        verts += [to_blender(x, y, z0) for x, y in profile]
        verts += [to_blender(x, y, z1) for x, y in profile]
        faces.append(tuple(base + n + i for i in range(n)))              # +z cap, profile order
        faces.append(tuple(base + i for i in reversed(range(n))))        # -z cap
        for i in range(n):
            j = (i + 1) % n
            faces.append((base + i, base + j, base + n + j, base + n + i))
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(verts, [], faces)
    mesh.update()
    mesh.materials.append(material)
    obj = bpy.data.objects.new(name, mesh)
    export.objects.link(obj)
    return obj


def material(name, rgb, roughness):
    mat = bpy.data.materials.new(name)
    mat.use_backface_culling = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*rgb, 1.0)
    bsdf.inputs["Metallic"].default_value = 0.0
    bsdf.inputs["Roughness"].default_value = roughness
    bsdf.inputs["Alpha"].default_value = 1.0
    return mat


FLOOR = material("floor", (0.30, 0.34, 0.40), 0.9)
WALLS = material("walls", (0.55, 0.42, 0.30), 0.9)
GREY = material("collision", (0.5, 0.5, 0.5), 0.9)
build_mesh("floor", solids["floor"], FLOOR)
build_mesh("walls", solids["walls"], WALLS)
build_mesh("collision", solids["floor"] + solids["walls"], GREY)


# --- the moving parts' paths (the bodies themselves are scene entities) -------------------------
def write_path(name, points, interpolation="linear"):
    lines = ["format: 1", "kind: path", f"name: {name}", "generated_from: blender/build.py",
             f"interpolation: {interpolation}", "closed: false", "points:"]
    for point in points:
        for i, c in enumerate(point):
            lines.append(("  - - " if i == 0 else "    - ") + repr(float(c)))
    with open(os.path.join(HERE, "..", "paths", f"{name}.path.yaml"), "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


write_path("platform", [(5.0, -0.125, -4.0), (7.0, -0.125, -4.0)])
write_path("press", [(19.0, 2.25, -4.0), (19.0, 0.625, -4.0)])
# the lane: straight entry and exit (both neighbours collinear), an S-bend over the ramp
write_path("lane", [(27.0, TOP, -0.25), (32.5, TOP, -0.25), (33.0, TOP, -0.25), (35.0, 2.0, 1.0),
                    (37.0, HIGH, -0.25), (41.0, HIGH, -0.25), (43.5, TOP, -0.25), (44.0, TOP, -0.25),
                    (52.0, TOP, -0.25)], "catmull_rom")
# the behind camera's rail: straight and level over the main lane, so its frame is constant
write_path("camera-behind", [(-8.0, TOP, 0.0), (56.0, TOP, 0.0)])

for image in bpy.data.images:
    assert image.file_format == "PNG", image.name
layer = bpy.context.view_layer
layer.active_layer_collection = layer.layer_collection.children["export"]
bpy.ops.export_scene.gltf(
    filepath=out_glb, export_format="GLB", export_yup=True,
    use_active_collection=True, use_active_collection_with_nested=False,
    export_apply=True, export_image_format="AUTO", export_materials="EXPORT",
    export_texcoords=True, export_normals=True, export_tangents=False,
    export_vertex_color="NONE", export_attributes=False, export_cameras=False,
    export_lights=False, export_extras=False, export_animations=False,
    export_skins=False, export_morph=False, export_gpu_instances=False,
    export_draco_mesh_compression_enable=False, export_meshopt_compression_enable=False)
