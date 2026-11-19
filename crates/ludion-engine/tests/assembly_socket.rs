//! Part sockets (`Plan/animation-layers.md` decisions 41-44): a `ParentJoint` under a voxel assembly
//! names a part. Sim reads (`world_transform`, colliders, hashes) compose the part's **rest** pivot
//! frame, a function of the committed documents only; the draw composes the part's **final drawn
//! pose** (clip, crossfade, layers, IK) through the one helper the part itself draws with, and
//! `posed_joint_world` reads that same pose from presentation.
//!
//! Runs over a scratch copy of the gallery's `voxels/` directory in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at, posed_joint_world, world_transform};
use ludion_protocol::{Command, command};

const ASSEMBLY: &str = "voxels/humanoid.assembly.yaml";
const PART: &str = "lower-arm-r";
/// `PART`'s index in the humanoid's part list, and so among the first instances `extract` emits.
const PART_INDEX: usize = 5;
const PARTS: usize = 10;

const LAYERS: &str = "      AnimationLayers:
        layers:
          - name: carry
            clip: carry
            mask: upper_body
            weight: 1.0
";
const IK: &str = "      IkTargets:
        targets:
          hand_r:
            point:
              - 3.0
              - 12.0
              - 6.0
            weight: 1.0
";

/// The humanoid, a socket cube on `joint` (with `body` appended to its components), and a
/// grandchild under the cube. `transform` is the humanoid's `Transform` body.
fn scene(joint: &str, transform: &str, animation: &str, body: &str) -> String {
    format!(
        "\
format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Transform:
{transform}
      Mesh:
        shape:
          assembly: {ASSEMBLY}
{animation}
  - name: socket
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
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
        joint: {joint}
{body}  - name: grandchild
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
"
    )
}

const PLACED: &str = "        translation:
          - 2.0
          - 0.0
          - 1.0
        rotation:
          - 0.0
          - 40.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125";
const PLAIN: &str = "        translation:
          - 2.0
          - 0.0
          - 1.0";
const WALK: &str = "      Animation:\n        clip: walk\n";
const COLLIDER: &str = "      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body: static
";

fn local() -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(0.5),
        Quat::from_rotation_y(30f32.to_radians()),
        Vec3::new(1.0, 2.0, 3.0),
    )
}

fn grandchild_local() -> Mat4 {
    Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-assembly-socket-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&gallery().join("voxels"), &dir.join("voxels"));
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

fn assert_close(actual: Mat4, expected: Mat4, what: &str) {
    for (a, e) in actual.to_cols_array().iter().zip(expected.to_cols_array()) {
        assert!(
            (a - e).abs() <= 1e-4 * e.abs().max(1.0),
            "{what}: {actual:?} vs {expected:?}"
        );
    }
}

fn world_mat(pose: ludion_engine::WorldPose) -> Mat4 {
    let [x, y, z, t] = pose.trs.to_affine().cols.map(Vec3::from);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
}

fn humanoid_of(engine: &Engine) -> ludion_core::ids::EntityId {
    engine
        .world()
        .entities()
        .next()
        .expect("the humanoid spawned")
}

fn socket_of(engine: &Engine) -> ludion_core::ids::EntityId {
    engine
        .world()
        .entities()
        .nth(1)
        .expect("the socket spawned")
}

/// The part's rest socket in the assembly frame: `offset + pivot`.
fn rest_socket(root: &Path) -> Vec3 {
    let data = ludion_engine::load_assembly(ludion_assets::AssetRoot::content(root), ASSEMBLY)
        .expect("the humanoid loads");
    let part = &data.parts[PART_INDEX];
    assert_eq!(part.name, PART);
    Vec3::from(part.offset) + Vec3::from(part.pivot)
}

fn pivot(root: &Path) -> Vec3 {
    let data = ludion_engine::load_assembly(ludion_assets::AssetRoot::content(root), ASSEMBLY)
        .expect("the humanoid loads");
    Vec3::from(data.parts[PART_INDEX].pivot)
}

fn step(engine: &mut Engine, ticks: u64) {
    ludion_engine::drive_hashes(
        engine,
        vec![Command::SimStep(command::SimStepParams {
            ticks,
            hash: false,
        })],
    )
    .expect("the session steps");
}

const VARIANTS: [(&str, &str); 3] = [("walk", ""), ("layers", LAYERS), ("ik", IK)];

#[test]
fn the_sim_reads_the_rest_socket_whatever_the_part_is_doing() {
    let root = fixture_project("sim");
    let socket = Mat4::from_translation(rest_socket(&root));
    for (name, extra) in VARIANTS {
        let mut engine = engine_at(&root, &scene(PART, PLACED, &format!("{WALK}{extra}"), ""));
        let (humanoid, child) = (humanoid_of(&engine), socket_of(&engine));
        let expected =
            world_mat(world_transform(engine.world(), humanoid).unwrap()) * socket * local();
        let first = world_transform(engine.world(), child).unwrap();
        assert_close(world_mat(first), expected, name);
        step(&mut engine, 23);
        assert_eq!(
            world_transform(engine.world(), child).unwrap(),
            first,
            "{name}: the sim socket does not move with the animation"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_draw_follows_the_part_and_posed_joint_world_reads_it() {
    let root = fixture_project("drawn");
    let pivot = Mat4::from_translation(pivot(&root));
    for (name, extra) in VARIANTS {
        let engine = engine_at(&root, &scene(PART, PLACED, &format!("{WALK}{extra}"), ""));
        let humanoid = humanoid_of(&engine);
        let rest = world_mat(world_transform(engine.world(), socket_of(&engine)).unwrap());
        let mut drawn = Vec::new();
        for tick in [7, 30, 45] {
            let scene = extract(&engine, tick);
            let what = format!("{name} tick {tick}");
            let part = scene.meshes[PART_INDEX].model;
            let socket = scene.meshes[PARTS].model;
            assert_close(
                socket,
                part * pivot * local(),
                &format!("{what}: rides the part"),
            );
            assert_close(
                scene.meshes[PARTS + 1].model,
                socket * grandchild_local(),
                &format!("{what}: grandchild"),
            );
            let pose = posed_joint_world(engine.world(), humanoid, PART, at(tick))
                .expect("a host read is allowed")
                .expect("the part resolves");
            assert_close(
                world_mat(pose) * local(),
                socket,
                &format!("{what}: host read"),
            );
            drawn.push(socket);
        }
        if name == "walk" {
            // A full-weight carry layer or a solved IK target pins the arm, whatever the base clip does.
            assert_ne!(drawn[0], drawn[1], "{name}: the draw moves with the clip");
        }
        assert_ne!(drawn[1], rest, "{name}: the draw is not the rest socket");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_an_animation_the_draw_is_the_sim() {
    let root = fixture_project("rest");
    let engine = engine_at(&root, &scene(PART, PLAIN, "", ""));
    let sim = world_mat(world_transform(engine.world(), socket_of(&engine)).unwrap());
    let scene = extract(&engine, 12);
    assert_eq!(
        scene.meshes[PARTS].model, sim,
        "drawn == sim at rest, bit for bit"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_socket_with_a_collider_is_hash_neutral_to_extraction() {
    let root = fixture_project("hash");
    let text = scene(PART, PLACED, &format!("{WALK}{LAYERS}{IK}"), COLLIDER);
    let run = |extract_each_tick: bool| {
        let mut engine = engine_at(&root, &text);
        let mut trace = Vec::new();
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(
                    &mut engine,
                    vec![Command::SimStep(command::SimStepParams {
                        ticks: 1,
                        hash: false,
                    })],
                )
                .expect("the session drives")
                .into_iter()
                .skip(1),
            );
            if extract_each_tick {
                let scene = extract(&engine, engine.frame().as_u64());
                assert_eq!(scene.meshes.len(), PARTS + 2);
            }
        }
        trace
    };
    let quiet = run(false);
    assert_eq!(quiet.len(), 40);
    assert_eq!(quiet, run(true), "extracting must not move a world hash");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_drawn_socket_is_thread_count_invariant() {
    let root = fixture_project("threads");
    let engine = engine_at(
        &root,
        &scene(PART, PLACED, &format!("{WALK}{LAYERS}{IK}"), ""),
    );
    let drawn = |threads: usize| {
        ludion_core::par::with_threads(threads, || {
            let scene = extract(&engine, 30);
            (scene.meshes[PARTS].model, scene.meshes[PARTS + 1].model)
        })
    };
    let one = drawn(1);
    assert_eq!(one, drawn(2));
    assert_eq!(one, drawn(8));
    let _ = std::fs::remove_dir_all(&root);
}

fn parent_joint_diagnostics(
    dir: &Path,
    scene_text: &str,
) -> Vec<ludion_core::diagnostics::Diagnostic> {
    let checked = ludion_scene::check(scene_text, &ludion_engine::builtin_registry())
        .unwrap_or_else(|errors| panic!("the fixture is schema-valid: {errors:?}"));
    ludion_engine::scene_parent_joint_diagnostics(dir, &checked)
}

#[test]
fn load_diagnostics_name_the_part() {
    let root = fixture_project("diagnostics");

    let clean = scene(PART, PLAIN, "", "");
    assert!(parent_joint_diagnostics(&root, &clean).is_empty());
    Engine::with_project_root(&root)
        .load_inline_scene(&clean)
        .expect("an assembly part socket loads");

    let typo = scene("lower-arm-rr", PLAIN, "", "");
    let found = parent_joint_diagnostics(&root, &typo);
    let unknown = found
        .iter()
        .find(|d| d.code == code::PARENT_JOINT_UNKNOWN_JOINT)
        .unwrap_or_else(|| panic!("expected L1319 among {found:?}"));
    assert!(
        unknown
            .message
            .contains("not a part of its parent's assembly")
    );
    assert!(
        unknown
            .help
            .as_deref()
            .is_some_and(|h| h.contains("lower-arm-r")),
        "the help names the nearest part: {unknown:?}"
    );
    let errors = Engine::with_project_root(&root)
        .load_inline_scene(&typo)
        .expect_err("an unknown part fails to load");
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::PARENT_JOINT_UNKNOWN_JOINT)
    );

    let part_parent = clean.replace(
        &format!("shape:\n          assembly: {ASSEMBLY}"),
        &format!("shape:\n          assembly_part:\n            assembly: {ASSEMBLY}\n            part: torso"),
    );
    let found = parent_joint_diagnostics(&root, &part_parent);
    assert!(
        found
            .iter()
            .any(|d| d.code == code::PARENT_JOINT_NOT_SKINNED),
        "an assembly_part parent stays L1318: {found:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
