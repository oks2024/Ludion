"""skin-mannequin-clips: the animated mannequin, four Actions keyed on the skin-mannequin rig.

Builds the mannequin through skin-mannequin.py (its export skipped), then keys `idle` (2 s, loop),
`run` (0.8 s, loop), `jump` (0.8 s, one-shot, squash and stretch) and `spin` (0.4 s, one-shot, a full
360 degree turn) at 30 fps from frame 0, and exports one glTF animation per Action. `loop` is a
custom property on the Action; the exporter carries it to the animation's `extras`.
Rotations are Euler XYZ on every bone: a quaternion key cannot express a 360 degree turn. Bone Y
runs along the bone, so the pelvis's vertical turn is `rotation_euler[1]`.

The glb is committed beside this script (Blender exports are not byte-stable across processes);
`ludion import` of it produces examples/gallery/clips/, gated by the drift test in
crates/ludion-cli/tests/import.rs.

Run headless:  <pinned blender> -b --factory-startup --python skin-mannequin-clips.py -- skin-mannequin-clips.glb
Run over MCP:  set sys.argv = ["blender", "--", "<out.glb>"], then exec this file with __file__ set.
"""
import bpy

if bpy.app.version[:2] != (5, 2):
    raise SystemExit(f"skin-mannequin-clips.py is pinned to Blender 5.2, this is {bpy.app.version_string}")

import math
import os
import runpy
import sys

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(argv) != 1:
    raise SystemExit("usage: blender -b --factory-startup --python skin-mannequin-clips.py -- <out.glb>")

base = os.path.join(os.path.dirname(os.path.abspath(__file__)), "skin-mannequin.py")
built = runpy.run_path(base, init_globals={"SKIN_MANNEQUIN_BUILD_ONLY": True, "__file__": base})
arm, scene = built["arm"], built["scene"]

# The base build clears every datablock but actions: a re-run in one session would export `idle.001`.
for action in list(bpy.data.actions):
    bpy.data.actions.remove(action)

scene.render.fps = 30
BONES = list(arm.pose.bones)
for pb in BONES:
    pb.rotation_mode = "XYZ"
arm.animation_data_create()

rad = math.radians


PROPS = ("location", "rotation_euler", "scale")


def rest():
    for pb in BONES:
        pb.location = (0.0, 0.0, 0.0)
        pb.rotation_euler = (0.0, 0.0, 0.0)
        pb.scale = (1.0, 1.0, 1.0)


def smooth(t):
    return t * t * (3.0 - 2.0 * t)


def lerp(a, b, t):
    return a + (b - a) * t


def curve(points, t):
    """Piecewise smooth interpolation through (time, value) points, clamped at the ends."""
    if t <= points[0][0]:
        return points[0][1]
    for (t0, v0), (t1, v1) in zip(points, points[1:]):
        if t <= t1:
            return lerp(v0, v1, smooth((t - t0) / (t1 - t0)))
    return points[-1][1]


def bake(name, frames, pose, loop):
    """Key `pose(bones, t)` at frames 0..frames, t in seconds, into a new Action `name`. Only the
    (bone, property) pairs that leave rest are keyed: the exporter writes a channel for every key."""
    poses = []
    for f in range(frames + 1):
        rest()
        pose(arm.pose.bones, f / 30.0)
        poses.append({(pb.name, prop): tuple(getattr(pb, prop)) for pb in BONES for prop in PROPS})
    rest()
    rest_pose = {(pb.name, prop): tuple(getattr(pb, prop)) for pb in BONES for prop in PROPS}
    moving = [k for k in rest_pose if any(p[k] != rest_pose[k] for p in poses)]
    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    arm.animation_data.action = action
    for f, values in enumerate(poses):
        for bone_name, prop in moving:
            setattr(arm.pose.bones[bone_name], prop, values[(bone_name, prop)])
            arm.pose.bones[bone_name].keyframe_insert(prop, frame=f)
    action.use_frame_range = True
    action.frame_start, action.frame_end = 0, frames
    if loop:
        action["loop"] = True


def idle(b, t):
    w = 2.0 * math.pi * t / 2.0
    b["torso"].rotation_euler[0] = rad(1.5) * math.sin(w)
    b["head"].rotation_euler[0] = rad(0.8) * math.sin(w - 0.6)
    for s in ("l", "r"):
        b[f"upper_arm_{s}"].rotation_euler[0] = rad(1.0) * math.sin(w - 0.3)
    b["pelvis"].location[1] = 0.004 * math.sin(w)


def run(b, t):
    w = 2.0 * math.pi * t / 0.8
    b["pelvis"].location[1] = 0.01 * math.cos(2.0 * w)
    b["torso"].rotation_euler[0] = rad(8.0)
    for s, sign in (("l", 1.0), ("r", -1.0)):
        b[f"upper_leg_{s}"].rotation_euler[0] = rad(35.0) * sign * math.sin(w)
        b[f"lower_leg_{s}"].rotation_euler[0] = rad(30.0) * (1.0 + sign * math.cos(w))
        b[f"upper_arm_{s}"].rotation_euler[0] = -rad(25.0) * sign * math.sin(w)


def jump(b, t):
    sy = curve([(0.0, 1.0), (0.15, 0.85), (0.3, 1.0), (0.32, 1.15), (0.55, 1.15), (0.65, 0.95), (0.7, 0.92), (0.8, 1.0)], t)
    # The pelvis scale rescales the leg chain under it, so planted soles need drop = leg_len * (sy - 1);
    # `air` is the height above that.
    air = curve([(0.0, 0.0), (0.3, 0.0), (0.45, 0.222), (0.65, 0.0)], t)
    drop = 0.52 * (sy - 1.0) + air
    sxz = curve([(0.0, 1.0), (0.15, 1.08), (0.3, 1.0), (0.32, 0.93), (0.55, 0.93), (0.65, 1.02), (0.7, 1.04), (0.8, 1.0)], t)
    pelvis = b["pelvis"]
    pelvis.location[1] = drop
    pelvis.scale = (sxz, sy, sxz)
    arms = curve([(0.0, 0.0), (0.15, -20.0), (0.3, -20.0), (0.45, 150.0), (0.65, 40.0), (0.8, 0.0)], t)
    for s in ("l", "r"):
        b[f"upper_arm_{s}"].rotation_euler[0] = -rad(arms)


def spin(b, t):
    u = min(t / 0.4, 1.0)
    b["pelvis"].rotation_euler[1] = 2.0 * math.pi * smooth(u)
    out = math.sin(math.pi * u) * rad(30.0)
    for s, sign in (("l", 1.0), ("r", -1.0)):
        b[f"upper_arm_{s}"].rotation_euler[2] = out * sign


bake("idle", 60, idle, True)
bake("run", 24, run, True)
bake("jump", 24, jump, False)
bake("spin", 12, spin, False)
arm.animation_data.action = None
rest()

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
    export_extras=True,
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
    export_animations=True,
    export_animation_mode="ACTIONS",
    export_force_sampling=True,
    export_frame_step=1,
    export_frame_range=False,
    export_optimize_animation_size=False,
    export_optimize_animation_keep_anim_armature=False,
    export_optimize_animation_keep_anim_object=False,
    export_anim_slide_to_zero=True,
    export_negative_frame="SLIDE",
    export_bake_animation=False,
    export_merge_animation="ACTION",
    export_anim_single_armature=True,
    export_nla_strips=False,
    export_extra_animations=False,
    export_pointer_animation=False,
    export_morph_animation=False,
)
print(f"skin-mannequin-clips: wrote {argv[0]}")
