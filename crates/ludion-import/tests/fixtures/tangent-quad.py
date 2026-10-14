"""tangent-quad: the independent oracle for generated-tangent handedness.

One mesh of three disconnected quads in Blender's XY plane (normal +Z, which exports as +Y), each with
a different UV layout: A identity, B mirrored in u, C rotated 90 degrees. The material feeds a Normal
Map node from a small packed PNG and the export writes `TANGENT` (`export_tangents=True`), so the
GLB carries the frame Blender's glTF exporter (MikkTSpace) produces: `cross(n, t) * w` points
image-up. The importer's own generator is tested against it.

Run headless:  <pinned blender> -b --factory-startup --python tangent-quad.py -- tangent-quad.glb
Run over MCP:  set sys.argv = ["blender", "--", "<out.glb>"], then exec this file.
"""
import bpy

if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"tangent-quad.py is pinned to Blender 5.2, this is {bpy.app.version_string}")

import sys

if bpy.app.background:
    bpy.ops.wm.read_factory_settings(use_empty=True)
else:
    for block in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.images):
        for item in list(block):
            block.remove(item)
    for coll in list(bpy.data.collections):
        bpy.data.collections.remove(coll)

scene = bpy.context.scene
scene.name = "tangent-quad"
export = bpy.data.collections.new("export")
scene.collection.children.link(export)

# ---- mesh: three quads, each two triangles, UVs set per loop ----------------------------------
QUAD_UVS = {
    "A": [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
    "B": [(1.0, 0.0), (0.0, 0.0), (0.0, 1.0), (1.0, 1.0)],
    "C": [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
}
verts, faces, loop_uvs = [], [], []
for i, name in enumerate(QUAD_UVS):
    x = 2.0 * i
    base = len(verts)
    verts += [(x, 0.0, 0.0), (x + 1.0, 0.0, 0.0), (x + 1.0, 1.0, 0.0), (x, 1.0, 0.0)]
    faces.append((base, base + 1, base + 2, base + 3))
    loop_uvs += QUAD_UVS[name]

mesh = bpy.data.meshes.new("tangent-quad")
mesh.from_pydata(verts, [], faces)
uv_layer = mesh.uv_layers.new(name="UVMap")
for loop, uv in zip(uv_layer.data, loop_uvs):
    loop.uv = uv

# ---- material: Normal Map node over a packed PNG ----------------------------------------------
SIZE = 4
image = bpy.data.images.new("tangent-quad-normal", SIZE, SIZE, alpha=False)
image.colorspace_settings.name = "Non-Color"
pixels = []
for y in range(SIZE):
    for x in range(SIZE):
        pixels += [0.5 + 0.2 * (x - 1.5) / 1.5, 0.5 + 0.2 * (y - 1.5) / 1.5, 1.0, 1.0]
image.pixels = pixels
image.file_format = "PNG"
image.pack()

mat = bpy.data.materials.new("quads")
mat.use_backface_culling = True  # otherwise the exporter writes doubleSided (L1202)
nodes, links = mat.node_tree.nodes, mat.node_tree.links
bsdf = nodes["Principled BSDF"]
bsdf.inputs["Base Color"].default_value = (0.5, 0.5, 0.5, 1.0)
bsdf.inputs["Metallic"].default_value = 0.0
bsdf.inputs["Roughness"].default_value = 0.75
bsdf.inputs["Alpha"].default_value = 1.0
tex = nodes.new("ShaderNodeTexImage")
tex.image = image
normal_map = nodes.new("ShaderNodeNormalMap")
links.new(tex.outputs["Color"], normal_map.inputs["Color"])
links.new(normal_map.outputs["Normal"], bsdf.inputs["Normal"])
mesh.materials.append(mat)

obj = bpy.data.objects.new("tangent-quad", mesh)
export.objects.link(obj)

for img in bpy.data.images:
    assert img.file_format == "PNG", f"{img.name} is {img.file_format}, not PNG"
print(f"tangent-quad: {len(verts)} vertices, {2 * len(faces)} triangles")

# ---- export ----------------------------------------------------------------------------------
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(argv) != 1:
    raise SystemExit("usage: blender -b --factory-startup --python tangent-quad.py -- <out.glb>")
layer = bpy.context.view_layer
layer.active_layer_collection = layer.layer_collection.children["export"]
bpy.ops.export_scene.gltf(
    filepath=argv[0], export_format="GLB", export_yup=True,
    use_active_collection=True, use_active_collection_with_nested=False,
    export_apply=True, export_image_format="AUTO", export_materials="EXPORT",
    export_texcoords=True, export_normals=True, export_tangents=True,
    export_vertex_color="NONE", export_attributes=False, export_cameras=False,
    export_lights=False, export_extras=False, export_animations=False,
    export_skins=False, export_morph=False, export_gpu_instances=False,
    export_draco_mesh_compression_enable=False, export_meshopt_compression_enable=False)
print(f"tangent-quad: wrote {argv[0]}")
