//! Runtime proof for `AnimationLayers` and character bone masks (`Plan/animation-layers.md` W1,
//! Neon Shroud request 0007): extract composes override and additive layers over the base
//! `Animation` pose on the skin's bone palette, and the simulation never notices.
//!
//! - **Neutral.** A zero-weight layer poses exactly like no component; extraction moves no world
//!   hash and writes nothing into a recording; palettes are byte-identical at 1, 2 and 8 threads.
//! - **Masked.** An upper-body override moves the arms and leaves the legs bit-identical.
//! - **Steered.** A weight ramp is between its endpoints mid-fade and lands on the target; a
//!   snapshot restored mid-fade reproduces the palette; a socket follows the layered pose.
//! - **Refused.** A mask naming a joint the mesh lacks is `L1138`.
//!
//! Runs over a scratch copy of the gallery mannequin in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::math::glam::Mat4;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};

const JOINTS: usize = 11;
/// Joint indices in the mannequin's skin order.
const HEAD: usize = 2;
const UPPER_ARM_L: usize = 3;
const LOWER_ARM_L: usize = 4;
const LEGS: std::ops::Range<usize> = 7..11;

/// The mannequin plus an `upper_body` mask (the torso and everything under it).
const LAYERED_CHARACTER: &str = "\
format: 1
kind: assembly
name: layered
mesh: \"../meshes/skin-mannequin.mesh.yaml\"
clips:
  idle:
    clip: \"../clips/idle.anim.yaml\"
  jump:
    clip: \"../clips/jump.anim.yaml\"
  run:
    clip: \"../clips/run.anim.yaml\"
masks:
  upper_body:
    joints:
      - torso
    descendants: true
";

const BAD_MASK_CHARACTER: &str = "\
format: 1
kind: assembly
name: bad
mesh: \"../meshes/skin-mannequin.mesh.yaml\"
clips:
  run:
    clip: \"../clips/run.anim.yaml\"
masks:
  upper_body:
    joints:
      - torsoo
";

/// One mannequin playing `run`, plus whatever `extra` components it carries.
fn scene(extra: &str) -> String {
    format!(
        "format: 1
seed: 1
entities:
  - name: runner
    components:
      Mesh:
        shape:
          assembly: characters/layered.assembly.yaml
      Animation:
        clip: run
{extra}"
    )
}

/// A layer over the upper body playing `idle`, with `fields` appended to the layer.
fn idle_layer(fields: &str) -> String {
    format!(
        "      AnimationLayers:
        layers:
          - name: upper
            clip: idle
            mask: upper_body
{fields}"
    )
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-animation-layers-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["characters", "meshes", "clips"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for file in [
        "meshes/skin-mannequin.mesh.yaml",
        "clips/idle.anim.yaml",
        "clips/jump.anim.yaml",
        "clips/run.anim.yaml",
    ] {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    std::fs::write(
        dir.join("characters/layered.assembly.yaml"),
        LAYERED_CHARACTER,
    )
    .unwrap();
    std::fs::write(dir.join("characters/bad.assembly.yaml"), BAD_MASK_CHARACTER).unwrap();
    dir
}

fn engine_at(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn extract(engine: &Engine, tick: u64) -> ludion_renderer::RenderScene {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
}

/// Every skinned instance's palette slice at `tick`, in extract order.
fn palettes_at(engine: &Engine, tick: u64) -> Vec<Vec<Mat4>> {
    let scene = extract(engine, tick);
    scene
        .meshes
        .iter()
        .filter(|mesh| mesh.bone_offset > 0)
        .map(|mesh| {
            let start = mesh.bone_offset as usize - 1;
            scene.bone_palette[start..start + JOINTS].to_vec()
        })
        .collect()
}

fn close(a: &Mat4, b: &Mat4) -> bool {
    a.abs_diff_eq(*b, 1e-4)
}

#[test]
fn a_zero_weight_layer_poses_exactly_like_no_component() {
    let root = fixture_project("zero");
    let plain = engine_at(&root, &scene(""));
    let layered = engine_at(&root, &scene(&idle_layer("")));
    for tick in [0, 7, 31] {
        assert_eq!(
            palettes_at(&plain, tick),
            palettes_at(&layered, tick),
            "tick {tick}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_upper_body_override_moves_the_arms_and_not_the_legs() {
    let root = fixture_project("upper");
    let plain = engine_at(&root, &scene(""));
    let layered = engine_at(&root, &scene(&idle_layer("            weight: 1.0\n")));
    let (base, over) = (&palettes_at(&plain, 12)[0], &palettes_at(&layered, 12)[0]);
    for joint in LEGS {
        assert_eq!(base[joint], over[joint], "leg joint {joint} is untouched");
    }
    assert!(
        !close(&base[UPPER_ARM_L], &over[UPPER_ARM_L]),
        "the arm moved"
    );
    assert!(!close(&base[HEAD], &over[HEAD]), "the head moved");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_weight_ramp_is_between_its_endpoints_and_lands_on_the_target() {
    let root = fixture_project("ramp");
    let at = |weight: &str, ramp: &str| {
        engine_at(
            &root,
            &scene(&idle_layer(&format!(
                "            weight: {weight}\n{ramp}"
            ))),
        )
    };
    let ramped = at("1.0", "            fade: 1.0\n");
    let (zero, half, one) = (at("0.0", ""), at("0.5", ""), at("1.0", ""));
    // Half a second into a one-second ramp the weight is a half.
    assert!(
        palettes_at(&ramped, 30)[0]
            .iter()
            .zip(&palettes_at(&half, 30)[0])
            .all(|(a, b)| close(a, b))
    );
    let (lo, mid, hi) = (
        palettes_at(&zero, 30)[0][UPPER_ARM_L],
        palettes_at(&ramped, 30)[0][UPPER_ARM_L],
        palettes_at(&one, 30)[0][UPPER_ARM_L],
    );
    assert!(
        !close(&lo, &mid) && !close(&mid, &hi),
        "mid-ramp is between"
    );
    // After the fade the layer is the plain weight-1 layer, bit for bit.
    assert_eq!(palettes_at(&ramped, 90), palettes_at(&one, 90));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_snapshot_restored_mid_fade_reproduces_the_palette() {
    let root = fixture_project("restore");
    let text = scene(&idle_layer(
        "            weight: 1.0\n            fade: 1.0\n",
    ));
    let mut engine = engine_at(&root, &text);
    step(&mut engine, 20);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    step(&mut engine, 25);
    let want = palettes_at(&engine, engine.frame().as_u64());
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    step(&mut engine, 25);
    assert_eq!(palettes_at(&engine, engine.frame().as_u64()), want);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extracting_a_layered_character_changes_neither_its_hashes_nor_its_recording() {
    let root = fixture_project("hash-neutral");
    let text = scene(&idle_layer(
        "            weight: 0.7\n            fade: 0.5\n",
    ));
    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_at(&root, &text);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");
        let mut trace = Vec::new();
        for _ in 0..40 {
            let step = Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            });
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step])
                    .expect("the session drives")
                    .into_iter()
                    .skip(1),
            );
            if extract_each_tick {
                assert_eq!(palettes_at(&engine, engine.frame().as_u64()).len(), 1);
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap())
    };
    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);
    assert_eq!(quiet_trace.len(), 40);
    assert_eq!(quiet_trace, drawn_trace, "extraction moved a world hash");
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extraction wrote into the recording"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 240 mannequins x 11 joints is over the parallel threshold, so 2 and 8 threads really run wide.
#[test]
fn layered_palettes_are_byte_identical_at_1_2_8_threads() {
    let root = fixture_project("threads");
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    for i in 0..240 {
        text.push_str(&format!(
            "  - name: m{i}\n    components:\n      Mesh:\n        shape:\n          assembly: characters/layered.assembly.yaml\n      Animation:\n        clip: run\n"
        ));
        match i % 3 {
            0 => text.push_str(&idle_layer("            weight: 1.0\n")),
            1 => text.push_str(&idle_layer(
                "            mode: additive\n            weight: 0.6\n            fade: 2.0\n",
            )),
            _ => {}
        }
    }
    let mut engine = engine_at(&root, &text);
    step(&mut engine, 10);
    let runs: Vec<Vec<u32>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                extract(&engine, 10)
                    .bone_palette
                    .iter()
                    .flat_map(|m| m.to_cols_array().map(f32::to_bits))
                    .collect()
            })
        })
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 2 threads");
    assert_eq!(runs[0], runs[2], "1 vs 8 threads");
    let _ = std::fs::remove_dir_all(&root);
}

/// A `ParentJoint` prop on `lower_arm_l` draws at the layered pose of that joint.
#[test]
fn a_socket_follows_the_layered_pose() {
    let root = fixture_project("socket");
    let socket = "  - name: prop
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Parent:
        index: 0
        generation: 0
      ParentJoint:
        joint: lower_arm_l
";
    let layered_text = format!(
        "{}{socket}",
        scene(&idle_layer("            weight: 1.0\n"))
    );
    let plain_text = format!("{}{socket}", scene(""));
    let inverse_bind = {
        let source = std::fs::read_to_string(root.join("meshes/skin-mannequin.mesh.yaml")).unwrap();
        let joints = ludion_scene::read_mesh_joints(&source).unwrap().unwrap();
        Mat4::from_cols_array(&joints[LOWER_ARM_L].inverse_bind.map(|v| v as f32))
    };
    let drawn = |text: &str| {
        let engine = engine_at(&root, text);
        let scene = extract(&engine, 12);
        let skin = &scene.meshes[0];
        let start = skin.bone_offset as usize - 1;
        let palette = scene.bone_palette[start + LOWER_ARM_L];
        let joint_world = skin.model * palette * ludion_core::math::mat4_inverse(inverse_bind);
        (scene.meshes[1].model, joint_world)
    };
    let (layered_model, layered_joint) = drawn(&layered_text);
    let (plain_model, _) = drawn(&plain_text);
    assert!(
        layered_model.abs_diff_eq(layered_joint, 1e-4),
        "{layered_model:?} vs {layered_joint:?}"
    );
    assert!(
        !layered_model.abs_diff_eq(plain_model, 1e-4),
        "the layered pose differs from the base pose"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_mask_naming_a_missing_joint_is_l1200() {
    let root = fixture_project("bad-mask");
    let errors = ludion_engine::load_character(
        ludion_assets::AssetRoot::content(&root),
        "characters/bad.assembly.yaml",
    )
    .expect_err("the mask names a joint the mesh lacks");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/masks/upper_body/joints/0")),
        "{errors:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
