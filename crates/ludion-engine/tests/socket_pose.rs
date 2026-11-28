//! Posed sockets (`Plan/display-paced-present.md` decision 15, Clockout 0028): a `ParentJoint` child
//! draws at `skin world × POSED joint model × local`, `posed_joint_world` reads what extract draws,
//! and under present interpolation the child takes its skin's blend key.
//!
//! Runs over a scratch copy of the gallery mannequin in the system temp directory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ludion_core::codes::code;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::SystemCtx;
use ludion_engine::{Engine, QualityParams, extract_at, posed_joint_world, world_transform};
use ludion_protocol::{Command, command};

const JOINT: &str = "lower_arm_l";
/// `JOINT`'s index in the mannequin's joint list.
const JOINT_INDEX: usize = 4;
const JOINTS: usize = 11;

/// Ascending entity order: the mannequin, a socket child, the child's own child, a plain child.
fn scene(animation: &str) -> String {
    format!(
        "\
format: 1
seed: 1
entities:
  - name: skin
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 1.0
        rotation:
          - 0.0
          - 40.0
          - 0.0
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
{animation}
  - name: socket
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.1
          - 0.2
          - 0.3
        rotation:
          - 0.0
          - 30.0
          - 0.0
        scale:
          - 0.5
          - 0.5
          - 0.5
      Parent:
        index: 0
        generation: 0
      ParentJoint:
        joint: {JOINT}
  - name: grandchild
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 1
        generation: 0
  - name: plain
    components:
      Mesh:
        shape: cube
      Parent:
        index: 0
        generation: 0
"
    )
}

fn animation(extra: &str) -> String {
    format!("      Animation:\n        clip: run\n{extra}")
}

const FADE: &str = "        from: jump\n        fade: 1.0\n";
const HOLD: &str = "        hold: 3\n";

fn local() -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(0.5),
        Quat::from_rotation_y(30f32.to_radians()),
        Vec3::new(0.1, 0.2, 0.3),
    )
}

fn grandchild_local() -> Mat4 {
    Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn fixture_project(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-socket-pose-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["characters", "meshes", "clips"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for file in [
        "characters/mannequin.character.yaml",
        "meshes/skin-mannequin.mesh.yaml",
        "clips/idle.anim.yaml",
        "clips/jump.anim.yaml",
        "clips/run.anim.yaml",
        "clips/spin.anim.yaml",
    ] {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    dir
}

fn engine_at(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn at(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

fn extract(engine: &Engine, tick: u64) -> ludion_renderer::RenderScene {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        at(tick),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
}

/// The joint's inverse bind matrix, from the mesh document.
fn inverse_bind(root: &Path) -> Mat4 {
    let source = std::fs::read_to_string(root.join("meshes/skin-mannequin.mesh.yaml")).unwrap();
    let joints = ludion_scene::read_mesh_joints(&source).unwrap().unwrap();
    assert_eq!(joints[JOINT_INDEX].name, JOINT);
    Mat4::from_cols_array(&joints[JOINT_INDEX].inverse_bind.map(|v| v as f32))
}

fn assert_close(actual: Mat4, expected: Mat4, what: &str) {
    for (a, e) in actual.to_cols_array().iter().zip(expected.to_cols_array()) {
        assert!(
            (a - e).abs() <= 1e-5 * e.abs().max(1.0),
            "{what}: {actual:?} vs {expected:?}"
        );
    }
}

/// The joint's world matrix as the skin's own palette shows it in `scene`.
fn palette_joint_world(scene: &ludion_renderer::RenderScene, root: &Path) -> Mat4 {
    let skin = &scene.meshes[0];
    let start = skin.bone_offset as usize - 1;
    let palette = scene.bone_palette[start..start + JOINTS][JOINT_INDEX];
    skin.model * palette * ludion_core::math::mat4_inverse(inverse_bind(root))
}

/// The drawn socket child and grandchild at `tick` against the skin's own palette entry.
fn assert_drawn_follows_palette(engine: &Engine, root: &Path, tick: u64) -> Mat4 {
    let scene = extract(engine, tick);
    let child = palette_joint_world(&scene, root) * local();
    assert_close(
        scene.meshes[1].model,
        child,
        &format!("child at tick {tick}"),
    );
    assert_close(
        scene.meshes[2].model,
        child * grandchild_local(),
        &format!("grandchild at tick {tick}"),
    );
    scene.meshes[1].model
}

fn world_mat(pose: ludion_engine::WorldPose) -> Mat4 {
    let [x, y, z, t] = pose.trs.to_affine().cols.map(Vec3::from);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
}

fn skin_of(engine: &Engine) -> ludion_core::ids::EntityId {
    engine.world().entities().next().expect("the skin spawned")
}

#[test]
fn a_socket_child_and_its_descendants_draw_at_the_posed_joint() {
    let root = fixture_project("drawn");
    for (name, extra, ticks) in [
        ("run", "", vec![0, 7, 30]),
        ("crossfade", FADE, vec![7, 30, 60]),
        ("hold", HOLD, vec![3, 4, 6]),
    ] {
        let engine = engine_at(&root, &scene(&animation(extra)));
        let drawn: Vec<Mat4> = ticks
            .iter()
            .map(|&tick| assert_drawn_follows_palette(&engine, &root, tick))
            .collect();
        assert_ne!(drawn[0], drawn[2], "{name}: the socket moves with the clip");
        if name == "hold" {
            assert_eq!(drawn[0], drawn[1], "tick 4 holds tick 3");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn posed_joint_world_matches_the_draw() {
    let root = fixture_project("read");
    for extra in ["", FADE, HOLD] {
        let engine = engine_at(&root, &scene(&animation(extra)));
        let skin = skin_of(&engine);
        for tick in [7, 30, 31] {
            let pose = posed_joint_world(engine.world(), skin, JOINT, at(tick))
                .expect("a host read is allowed")
                .expect("the joint resolves");
            let scene = extract(&engine, tick);
            let what = format!("{extra:?} tick {tick}");
            if pose.sheared {
                // A crossfade blends scales down the chain, so the joint carries a shear a `Trs`
                // cannot hold: the read then keeps the joint's origin and reports `sheared`.
                assert_eq!(extra, FADE, "{what}: only a crossfade shears the joint");
                assert_close(
                    Mat4::from_translation(world_mat(pose).w_axis.truncate()),
                    Mat4::from_translation(palette_joint_world(&scene, &root).w_axis.truncate()),
                    &what,
                );
            } else {
                assert_close(world_mat(pose) * local(), scene.meshes[1].model, &what);
            }
        }
        assert!(
            posed_joint_world(engine.world(), skin, "no_such_joint", at(0))
                .unwrap()
                .is_none()
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_skinned_assembly_poses_its_socket_like_the_character_spelling() {
    let root = fixture_project("assembly-spelling");
    let character = scene(&animation(""));
    let assembly = character.replace("character: ", "assembly: ");
    let [character, assembly] = [character, assembly].map(|text| {
        let engine = engine_at(&root, &text);
        let drawn = assert_drawn_follows_palette(&engine, &root, 7);
        let pose = posed_joint_world(engine.world(), skin_of(&engine), JOINT, at(7))
            .expect("a host read is allowed")
            .expect("the joint resolves");
        let child = engine.world().entities().nth(1).unwrap();
        let sim = world_mat(world_transform(engine.world(), child).unwrap());
        (drawn, world_mat(pose), sim)
    });
    assert_eq!(character, assembly);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_a_clip_the_read_and_the_draw_are_the_rest_pose() {
    let root = fixture_project("rest");
    let engine = engine_at(&root, &scene(""));
    let skin = skin_of(&engine);
    let child = engine.world().entities().nth(1).unwrap();
    let pose = posed_joint_world(engine.world(), skin, JOINT, at(5))
        .unwrap()
        .expect("the joint resolves");
    let sim = world_mat(world_transform(engine.world(), child).unwrap());
    assert_close(world_mat(pose) * local(), sim, "read against the sim rest");
    assert_close(
        extract(&engine, 5).meshes[1].model,
        sim,
        "draw against the sim rest",
    );
    let _ = std::fs::remove_dir_all(&root);
}

static SEEN: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

fn probe(label: &'static str, world: &ludion_ecs::World, ctx: &SystemCtx) {
    let skin = world.entities().next().expect("the skin spawned");
    let outcome = match posed_joint_world(world, skin, JOINT, ctx.time) {
        Ok(Some(_)) => "some".to_string(),
        Ok(None) => "none".to_string(),
        Err(diagnostic) => diagnostic.code.to_string(),
    };
    SEEN.lock().unwrap().push((label, outcome));
}

#[test]
fn a_sim_system_is_refused_and_a_presentation_system_reads() {
    let root = fixture_project("refusal");
    let mut engine = Engine::builder()
        .project_root(&root)
        .system("sim_probe", |world, ctx| probe("sim", world, ctx))
        .presentation_system("presentation_probe", |world, ctx| {
            probe("presentation", world, ctx);
        })
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(&scene(&animation("")))
        .expect("the scene loads");
    ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        })],
    )
    .expect("the session drives");
    let seen = SEEN.lock().unwrap().clone();
    assert!(
        seen.contains(&("sim", code::PRESENTATION_BOUNDARY.to_string())),
        "{seen:?}"
    );
    assert!(
        seen.contains(&("presentation", "some".to_string())),
        "{seen:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_socket_child_and_its_descendants_take_the_skins_blend_key() {
    let root = fixture_project("blend");
    let mut engine = engine_at(&root, &scene(&animation("")));
    let ids: Vec<_> = engine.world().entities().collect();
    let (_, blend) = engine.extract_present_scene().expect("the scene extracts");
    assert_eq!(
        blend.meshes,
        [Some(ids[0]), Some(ids[0]), Some(ids[0]), Some(ids[3])]
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A second mannequin (index 4) and a cube `rider` (index 5) under the socket that follows one of
/// its bones.
const RIDER: &str = r"  - name: skin2
    components:
      Transform:
        translation:
          - -3.0
          - 0.0
          - 0.0
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
  - name: rider
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Parent:
        index: 1
        generation: 0
      SkinBone:
        skin:
          index: 4
          generation: 0
        bone: torso
";

/// A root `driver` (index 4) that follows `upper_arm_l` of the skin at index 0.
const DRIVER: &str = r"  - name: driver
    components:
      Transform:
        translation:
          - 4.0
          - 3.0
          - -2.0
      SkinBone:
        skin:
          index: 0
          generation: 0
        bone: upper_arm_l
";

#[test]
fn a_skin_bone_part_under_a_socket_child_draws_at_the_posed_joint() {
    let root = fixture_project("rider");
    let engine = engine_at(&root, &format!("{}{RIDER}", scene(&animation(""))));
    for tick in [0, 7, 30] {
        let child = assert_drawn_follows_palette(&engine, &root, tick);
        let scene = extract(&engine, tick);
        assert_close(
            scene.meshes[5].model,
            child * Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            &format!("rider at tick {tick}"),
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_part_following_a_bone_moves_the_socket_through_the_posed_palette() {
    let root = fixture_project("driver");
    let driven = engine_at(&root, &format!("{}{DRIVER}", scene("")));
    let rest = engine_at(&root, &scene(""));
    let drawn = assert_drawn_follows_palette(&driven, &root, 0);
    assert_ne!(
        drawn,
        extract(&rest, 0).meshes[1].model,
        "the follower moved the joint"
    );
    let _ = std::fs::remove_dir_all(&root);
}
