//! `SkinBone`: the spawn pose (`skin × bone rest`, an authored `Transform` wins), the round trip
//! through `scene_text()`, the four diagnostics (both through `load_inline_scene` and directly
//! through `scene_skin_bone_diagnostics`), and thread-count invariance under physics.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{DMat4, DQuat, DVec3, Mat4, Quat, Vec3};
use ludion_core::par;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, Transform, extract_at};
use ludion_protocol::{Command, command};
use ludion_scene::{JointRest, MeshDocument, MeshJoint, MeshSkin};

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-skin-bone-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    dir
}

fn identity16() -> [f64; 16] {
    let mut m = [0.0; 16];
    for i in 0..4 {
        m[i * 5] = 1.0;
    }
    m
}

/// A quaternion `[x, y, z, w]` for a rotation of `degrees` about `+Z`, built by hand — the mesh
/// document's own rest rotation, not the engine's Euler-XYZ convention. `ludion_core::math::f64`'s
/// pure-Rust libm, never `f64::sin`/`cos` directly: platform libm differs across OSes.
fn quat_deg_z(degrees: f64) -> [f64; 4] {
    use ludion_core::math::f64::{cos, sin};
    let half = degrees.to_radians() / 2.0;
    [0.0, 0.0, sin(half), cos(half)]
}

/// Writes `meshes/rig.mesh.yaml`: a two-joint skin, `root` (identity rest) then `child`, whose rest
/// carries a non-trivial translation **and** rotation — the arithmetic proof needs both to catch an
/// argument-order bug a translation-only rest cannot.
fn write_rig(dir: &Path) {
    let doc = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                MeshJoint {
                    name: "root".to_string(),
                    parent: None,
                    rest: JointRest {
                        translation: [0.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: identity16(),
                },
                MeshJoint {
                    name: "child".to_string(),
                    parent: Some(0),
                    rest: JointRest {
                        translation: [0.0, 1.0, 0.0],
                        rotation: quat_deg_z(45.0),
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: identity16(),
                },
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    };
    std::fs::write(
        dir.join("meshes/rig.mesh.yaml"),
        ludion_scene::emit_mesh(&doc),
    )
    .unwrap();
}

const SKIN_MESH: &str = "      Mesh:\n        shape:\n          asset: \"meshes/rig.mesh.yaml\"\n";

/// [`SKIN_MESH`] plus `extra` — the skin entity's components for every fixture but the "no `Mesh`
/// at all" one.
fn skinned(extra: &str) -> String {
    format!("{SKIN_MESH}{extra}")
}

/// A skin entity whose components are exactly `skin_components`, and a `part` entity whose
/// components are exactly `part_components` — every scene fixture in this file shares this shape.
fn scene(skin_components: &str, part_components: &str) -> String {
    let mut text = String::from("format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n");
    text.push_str("  - name: skin\n    components:\n");
    text.push_str(skin_components);
    text.push_str("  - name: part\n    components:\n");
    text.push_str(part_components);
    text
}

const SKIN_BONE_CHILD: &str = "      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: child\n";

fn engine_with_rig(name: &str) -> (Engine, PathBuf) {
    let dir = fresh_dir(name);
    write_rig(&dir);
    (Engine::with_project_root(&dir), dir)
}

/// The skin's authored `Transform`: translated, rotated (yaw only, so the expected math below need
/// not fight gimbal lock) and uniformly scaled.
const SKIN_TRANSFORM: &str = "      Transform:\n        translation:\n          - 2.0\n          - 0.0\n          - 0.0\n        rotation:\n          - 0.0\n          - 90.0\n          - 0.0\n        scale:\n          - 2.0\n          - 2.0\n          - 2.0\n";

/// `skin × bone rest`, composed independently of `ludion-engine`'s own (private) arithmetic, for the
/// no-authored-`Transform` proof: the skin's world matrix times the child joint's rest model matrix
/// (root is identity, so the child's rest model matrix is its own local rest).
fn expected_world() -> DMat4 {
    use ludion_core::math::f64::{cos, sin};
    // Half-angle of the skin's 90-degree Y rotation ([`SKIN_TRANSFORM`]).
    let skin_half = (45f64).to_radians();
    let skin_rotation = DQuat::from_xyzw(0.0, sin(skin_half), 0.0, cos(skin_half));
    let skin_model = DMat4::from_scale_rotation_translation(
        DVec3::new(2.0, 2.0, 2.0),
        skin_rotation,
        DVec3::new(2.0, 0.0, 0.0),
    );
    // Half-angle of the child bone's 45-degree Z rotation ([`quat_deg_z`]).
    let bone_half = (22.5f64).to_radians();
    let bone_model = DMat4::from_scale_rotation_translation(
        DVec3::new(1.0, 1.0, 1.0),
        DQuat::from_xyzw(0.0, 0.0, sin(bone_half), cos(bone_half)),
        DVec3::new(0.0, 1.0, 0.0),
    );
    skin_model * bone_model
}

fn part_transform(engine: &Engine) -> Transform {
    let id = ludion_core::ids::EntityId::new(1, 0);
    *engine
        .world()
        .get::<Transform>(id)
        .expect("the part entity is alive")
        .expect("the part carries a Transform")
}

/// A part with no authored `Transform` spawns at `skin`'s world transform times its bone's rest
/// model transform, compared to a value composed independently of the engine's own arithmetic.
#[test]
fn no_transform_lands_at_skin_times_bone_rest() {
    let (mut engine, _dir) = engine_with_rig("no-transform");
    engine
        .load_inline_scene(&scene(&skinned(SKIN_TRANSFORM), SKIN_BONE_CHILD))
        .expect("a well-formed SkinBone scene loads");

    let actual = part_transform(&engine);
    let expected = expected_world();
    let (expected_scale, expected_rotation, expected_translation) =
        expected.to_scale_rotation_translation();

    for axis in 0..3 {
        assert!(
            (actual.translation[axis] as f64 - expected_translation[axis]).abs() < 1e-5,
            "translation {:?} vs expected {:?}",
            actual.translation,
            expected_translation
        );
        assert!(
            (actual.scale[axis] as f64 - expected_scale[axis]).abs() < 1e-5,
            "scale {:?} vs expected {:?}",
            actual.scale,
            expected_scale
        );
    }
    let actual_quat = euler_xyz_degrees_to_quat(actual.rotation);
    let actual_quat = DQuat::from_xyzw(
        actual_quat[0] as f64,
        actual_quat[1] as f64,
        actual_quat[2] as f64,
        actual_quat[3] as f64,
    );
    // Same rotation can be represented by either sign of the quaternion.
    let dot = actual_quat.dot(expected_rotation).abs();
    assert!((dot - 1.0).abs() < 1e-3, "rotation mismatch: dot {dot}");
}

/// A skin and a part under a translated anchor: the part's stored `Transform` is local to its
/// `Parent`, and its world pose is still `anchor × skin × bone rest`.
#[test]
fn skin_and_part_under_a_parent_land_at_the_same_world_pose() {
    let (mut engine, _dir) = engine_with_rig("under-parent");
    let parent = "      Parent:
        index: 0
        generation: 0
";
    let text = format!(
        "format: 1
seed: 3
physics:
  space: free
entities:
  - name: anchor
    components:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
  - name: skin
    components:
{parent}{}  - name: part
    components:
{parent}      SkinBone:
        skin:
          index: 1
          generation: 0
        bone: child
",
        skinned(SKIN_TRANSFORM)
    );
    engine
        .load_inline_scene(&text)
        .expect("a SkinBone scene under a Parent loads");
    let part = ludion_core::ids::EntityId::new(2, 0);
    let local = *engine.world().get::<Transform>(part).unwrap().unwrap();
    let expected = expected_world().to_scale_rotation_translation().2;
    let world = ludion_engine::world_transform(engine.world(), part)
        .expect("the part has a Transform")
        .trs
        .translation;
    for axis in 0..3 {
        let anchor = if axis == 0 { 10.0 } else { 0.0 };
        assert!(
            (local.translation[axis] as f64 - expected[axis]).abs() < 1e-4,
            "local {:?} vs {expected:?}",
            local.translation
        );
        assert!(
            (world[axis] as f64 - (expected[axis] + anchor)).abs() < 1e-4,
            "world {world:?} vs {expected:?} + anchor"
        );
    }
}

/// A part parented to another part, listed before it: both still land at their world poses, the
/// child localized against its parent's final pose.
#[test]
fn a_part_under_another_part_lands_at_its_world_pose_whatever_the_document_order() {
    let (mut engine, _dir) = engine_with_rig("part-under-part");
    let text = format!(
        "format: 1
seed: 3
physics:
  space: free
entities:
  - name: skin
    components:
{}  - name: child_part
    components:
      Parent:
        index: 2
        generation: 0
      SkinBone:
        skin:
          index: 0
          generation: 0
        bone: child
  - name: root_part
    components:
      SkinBone:
        skin:
          index: 0
          generation: 0
        bone: root
",
        skinned(SKIN_TRANSFORM)
    );
    engine
        .load_inline_scene(&text)
        .expect("a SkinBone part under a SkinBone part loads");
    let child = ludion_core::ids::EntityId::new(1, 0);
    let root = ludion_core::ids::EntityId::new(2, 0);
    let expected = expected_world().to_scale_rotation_translation().2;
    let world = ludion_engine::world_transform(engine.world(), child)
        .expect("the part has a Transform")
        .trs
        .translation;
    let root_world = ludion_engine::world_transform(engine.world(), root)
        .unwrap()
        .trs
        .translation;
    for axis in 0..3 {
        assert!(
            (world[axis] as f64 - expected[axis]).abs() < 1e-4,
            "child world {world:?} vs {expected:?}"
        );
    }
    assert!((root_world[0] - 2.0).abs() < 1e-4, "{root_world:?}");
}

/// An authored `Transform` wins outright: the part reads back exactly what was authored, untouched
/// by the skin or bone.
#[test]
fn authored_transform_wins() {
    let (mut engine, _dir) = engine_with_rig("authored-wins");
    let mut part = SKIN_BONE_CHILD.to_string();
    part.push_str(
        "      Transform:\n        translation:\n          - 5.0\n          - 6.0\n          - \
         7.0\n        rotation:\n          - 10.0\n          - 20.0\n          - 30.0\n        \
         scale:\n          - 1.0\n          - 1.0\n          - 1.0\n",
    );
    engine
        .load_inline_scene(&scene(&skinned(SKIN_TRANSFORM), &part))
        .expect("a well-formed SkinBone scene loads");

    let actual = part_transform(&engine);
    assert_eq!(actual.translation, [5.0, 6.0, 7.0]);
    assert_eq!(actual.rotation, [10.0, 20.0, 30.0]);
    assert_eq!(actual.scale, [1.0, 1.0, 1.0]);
}

/// `scene_text()` after load round-trips: re-loading the saved text into a fresh engine at the same
/// root yields the same `WorldHash`.
#[test]
fn scene_text_round_trips_to_the_same_world_hash() {
    let (mut engine, dir) = engine_with_rig("round-trip");
    engine
        .load_inline_scene(&scene(&skinned(""), SKIN_BONE_CHILD))
        .expect("a well-formed SkinBone scene loads");
    let hash = engine.world_hash().unwrap();
    let text = engine.scene_text().unwrap();

    let mut reloaded = Engine::with_project_root(&dir);
    reloaded
        .load_inline_scene(&text)
        .expect("the saved text re-loads");
    assert_eq!(reloaded.world_hash().unwrap(), hash);
}

fn assert_raises(scene_text: &str, dir: &Path, expected: ludion_core::diagnostics::DiagnosticCode) {
    let mut engine = Engine::with_project_root(dir);
    let errors = engine
        .load_inline_scene(scene_text)
        .expect_err("the malformed SkinBone scene must fail to load");
    assert!(
        errors.iter().any(|d| d.code == expected),
        "expected {expected:?} among {errors:?}"
    );

    let checked_scene = ludion_scene::check(scene_text, &ludion_engine::builtin_registry())
        .unwrap_or_else(|errors| panic!("the fixture is schema-valid: {errors:?}"));
    let direct = ludion_engine::scene_skin_bone_diagnostics(dir, &checked_scene);
    assert!(
        direct.iter().any(|d| d.code == expected),
        "expected {expected:?} among {direct:?}"
    );
}

/// `L1194`: the named `skin` carries no `Mesh` at all.
#[test]
fn a_skin_with_no_mesh_is_l1194() {
    let (_engine, dir) = engine_with_rig("not-skinned");
    assert_raises(
        &scene(SKIN_TRANSFORM, SKIN_BONE_CHILD),
        &dir,
        code::SKIN_BONE_NOT_SKINNED,
    );
}

/// `L1195`: `bone` names no joint of the skin's mesh document; the help carries the nearest name.
#[test]
fn an_unknown_bone_is_l1195_with_the_nearest_name() {
    let (_engine, dir) = engine_with_rig("unknown-bone");
    let part = "      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        \
                bone: chil\n";
    let text = scene(&skinned(SKIN_TRANSFORM), part);
    assert_raises(&text, &dir, code::SKIN_BONE_UNKNOWN_BONE);

    let mut engine = Engine::with_project_root(&dir);
    let errors = engine
        .load_inline_scene(&text)
        .expect_err("an unknown bone must fail to load");
    let found = errors
        .iter()
        .find(|d| d.code == code::SKIN_BONE_UNKNOWN_BONE)
        .expect("L1195 among the errors");
    assert!(
        found
            .help
            .as_deref()
            .is_some_and(|help| help.contains("child")),
        "help should name the nearest joint: {found:?}"
    );
}

/// `L1196`: two parts naming the same `(skin, bone)` pair.
#[test]
fn two_parts_naming_the_same_bone_is_l1196() {
    let (_engine, dir) = engine_with_rig("duplicate");
    let mut text = scene(&skinned(SKIN_TRANSFORM), SKIN_BONE_CHILD);
    text.push_str("  - name: part2\n    components:\n");
    text.push_str(SKIN_BONE_CHILD);
    assert_raises(&text, &dir, code::SKIN_BONE_DUPLICATE);
}

/// `L1197`: the skin entity also carries an `Animation`.
#[test]
fn an_animated_skin_is_l1197() {
    let (_engine, dir) = engine_with_rig("animated-skin");
    let skin_extra = "      Animation:\n        clip: idle\n";
    assert_raises(
        &scene(&skinned(skin_extra), SKIN_BONE_CHILD),
        &dir,
        code::SKIN_BONE_ANIMATED_SKIN,
    );
}

/// An out-of-range `skin` never reaches a `SkinBone`-specific pass: `SkinBone.skin` is an
/// `EntityRefSchema` field, so `ludion_scene::check`'s generic entity-reference pass refuses it
/// first, as `L1017`.
#[test]
fn out_of_range_skin_is_l1017_through_load_inline_scene() {
    let (mut engine, _dir) = engine_with_rig("out-of-range-skin");
    let part = "      SkinBone:\n        skin:\n          index: 9\n          generation: 0\n        \
                bone: child\n";
    let errors = engine
        .load_inline_scene(&scene(&skinned(SKIN_TRANSFORM), part))
        .expect_err("an out-of-range skin must fail to load");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::SCENE_INVALID_ENTITY_REF),
        "expected L1017 among {errors:?}"
    );
}

/// The child bone's rest-local transform — `write_rig`'s own `JointRest`, independently composed in
/// `f32`. The root's rest is identity, so this also equals the child's model-space rest matrix, the
/// value posing composes an unfollowed child onto, or reproduces verbatim at the spawn pose.
fn child_rest_local() -> Mat4 {
    let q = quat_deg_z(45.0);
    Mat4::from_scale_rotation_translation(
        Vec3::ONE,
        Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32),
        Vec3::new(0.0, 1.0, 0.0),
    )
}

/// Every element of `actual` and `expected` agrees within `1e-5` — the posing arithmetic's float
/// tolerance (an `f64`-composed rest entry narrowed to `f32`, then recomposed in `f32`).
fn assert_mat4_approx(actual: Mat4, expected: Mat4, msg: &str) {
    for (a, e) in actual.to_cols_array().iter().zip(expected.to_cols_array()) {
        assert!((a - e).abs() < 1e-5, "{msg}: {actual:?} vs {expected:?}");
    }
}

/// `extract_at`'s posed bone palette for `engine`'s world, at its current frame — entry `j` is
/// joint `j`'s palette slot (offset 1 in the GPU buffer's own indexing; this is the pre-identity
/// `RenderScene` list a single skinned instance fills start to end).
fn bone_palette(engine: &Engine) -> Vec<Mat4> {
    extract_at(
        engine.world(),
        (320, 180),
        engine.project_root(),
        SimTime::from_ticks(engine.frame().as_u64(), TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .bone_palette
}

/// A part naming `root` with an authored `Transform` moves that bone's palette entry to the part's
/// world pose; the unfollowed `child` composes its rest-local transform onto the moved root rather
/// than keeping its own rest entry. A part naming `child` with no authored `Transform` (the spawn
/// pose) reproduces the rest entry instead, since an unmoved part's world pose *is* the rest pose.
#[test]
fn a_moved_part_moves_its_bone_and_the_unfollowed_child_follows() {
    let (mut engine, _dir) = engine_with_rig("moved-part");
    let moved_root = "      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: root\n      Transform:\n        translation:\n          - 0.0\n          - 3.0\n          - 0.0\n        rotation:\n          - 0.0\n          - 0.0\n          - 0.0\n        scale:\n          - 1.0\n          - 1.0\n          - 1.0\n";
    engine
        .load_inline_scene(&scene(&skinned(""), moved_root))
        .expect("a well-formed SkinBone scene loads");

    let palette = bone_palette(&engine);
    assert_eq!(palette.len(), 2);
    let moved_root_world = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
    assert_mat4_approx(
        palette[0],
        moved_root_world,
        "the followed root's entry is the part's world pose",
    );
    assert_mat4_approx(
        palette[1],
        moved_root_world * child_rest_local(),
        "the unfollowed child composes its rest-local transform onto the moved root",
    );

    let (mut spawn_engine, _spawn_dir) = engine_with_rig("moved-part-spawn");
    spawn_engine
        .load_inline_scene(&scene(&skinned(SKIN_TRANSFORM), SKIN_BONE_CHILD))
        .expect("a well-formed SkinBone scene loads");
    let spawn_palette = bone_palette(&spawn_engine);
    assert_mat4_approx(
        spawn_palette[1],
        child_rest_local(),
        "a part at its spawn pose reproduces the child's rest entry",
    );
}

/// One `sim.step` of `ticks` ticks — the `assembly_animation.rs` shape.
fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The sim half of the hash-neutral proof, mirroring `assembly_animation.rs`'s
/// `extracting_an_animated_run_changes_neither_its_hashes_nor_its_recording`: a physics-driven
/// `SkinBone` part's per-frame `WorldHash` trace and recorded `.lrp` are byte-identical whether or
/// not anything extracts a frame along the way — posing the bone palette from a live part is a
/// render-boundary read, nothing written back into the world. The falling sphere really does move
/// the palette (asserted below), so the proof is not vacuous.
#[test]
fn extracting_a_skinned_ragdoll_changes_neither_its_hashes_nor_its_recording() {
    let dir = fresh_dir("hash-neutral");
    write_rig(&dir);
    let falling_root = "      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: root\n      Collider:\n        shape:\n          sphere:\n            radius: 0.4\n        body:\n          rigid:\n            mass: 1.0\n";
    let scene_text = scene(&skinned(""), falling_root);

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = dir.join(format!("{tag}.lrp"));
        let mut engine = Engine::with_project_root(&dir);
        engine
            .load_inline_scene(&scene_text)
            .expect("a well-formed SkinBone scene loads");
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        let mut first_palette = None;
        let mut last_palette = None;
        for tick in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                let palette = bone_palette(&engine);
                match tick {
                    0 => first_palette = Some(palette),
                    39 => last_palette = Some(palette),
                    _ => {}
                }
            }
        }
        engine.stop_recording().expect("recording stops");
        if extract_each_tick {
            assert_ne!(
                first_palette, last_palette,
                "the falling part must actually move the bone palette"
            );
        }
        (trace, std::fs::read(&lrp).unwrap())
    };

    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);

    assert_eq!(quiet_trace.len(), 40, "the run actually advanced 40 frames");
    assert_eq!(
        quiet_trace, drawn_trace,
        "extracting a posed frame must not move a single world hash"
    );
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extracting a posed frame must not write a byte into the recording"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The driven trace at 1, 2 and 8 threads, with a rigid `Collider` on the `SkinBone` part — the
/// `joint_motor.rs` `a_driven_chain_hashes_identically_at_every_thread_count` shape.
fn thread_trace(threads: usize) -> Vec<u64> {
    let (mut engine, _dir) = engine_with_rig(&format!("threads-{threads}"));
    let part = "      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        \
                bone: child\n      Collider:\n        shape:\n          sphere:\n            \
                radius: 0.4\n        body:\n          rigid:\n            mass: 1.0\n";
    engine
        .load_inline_scene(&scene(&skinned(""), part))
        .expect("a well-formed SkinBone scene loads");
    (0..20)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_skin_bone_part_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [1usize, 2, 8]
            .map(|n| scope.spawn(move || par::with_threads(n, || thread_trace(n))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}
