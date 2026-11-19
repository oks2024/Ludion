//! Ground feet on a voxel assembly (`Plan/animation-layers.md` decisions 45-49): an `ik:` chain marked
//! `ground: true` puts its sole on the static ground under it, in extract, on the composed pose, and
//! `ground: { root_drop }` can lower the whole assembly so the lower sole reaches. Presentation only:
//! flat ground at the floor and a scene with no collider pose bit for bit as without `ground`.
//!
//! Runs over a scratch copy of the gallery's `voxels/` directory in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at, posed_joint_world, world_transform};
use ludion_protocol::{Command, command};

const PLAIN: &str = "voxels/humanoid.assembly.yaml";
const GROUNDED: &str = "voxels/humanoid-grounded.assembly.yaml";
const PARTS: usize = 10;
const UPPER_LEG_R: usize = 7;
const LOWER_LEG_L: usize = 8;
const LOWER_LEG_R: usize = 9;
/// One voxel in metres under the fixture's Transform scale.
const VOXEL: f32 = 0.125;

const SCALE: &str = "        scale:
          - 0.125
          - 0.125
          - 0.125";
const WALK: &str = "      Animation:\n        clip: walk\n";

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
        "ludion-assembly-ground-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&gallery().join("voxels"), &dir.join("voxels"));
    dir
}

/// A static box collider entity with no mesh: `centre` and `half` in world metres.
fn block(name: &str, centre: [f32; 3], half: [f32; 3]) -> String {
    format!(
        "  - name: {name}
    components:
      Transform:
        translation:
          - {}
          - {}
          - {}
      Collider:
        shape:
          box:
            half_extents:
              - {}
              - {}
              - {}
        body: static
",
        centre[0], centre[1], centre[2], half[0], half[1], half[2]
    )
}

/// A floor whose top is at y = 0.
fn floor() -> String {
    block("floor", [0.0, -0.5, 0.0], [3.0, 0.5, 3.0])
}

/// The humanoid at the origin (entity 0, its parts the first meshes), `body` appended to its
/// components, then `extras` (whole entities).
fn scene(assembly: &str, body: &str, extras: &str) -> String {
    // Physics is declared only with colliders, so an empty `extras` is the scene with no physics.
    let physics = if extras.is_empty() {
        ""
    } else {
        "physics:\n  space: free\n"
    };
    format!(
        "\
format: 1
seed: 1
{physics}entities:
  - name: humanoid
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
{SCALE}
      Mesh:
        shape:
          assembly: {assembly}
{body}{extras}"
    )
}

fn engine_at(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn at(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

fn extract(engine: &Engine, root: &Path, tick: u64) -> ludion_renderer::RenderScene {
    let _ = root;
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        at(tick),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
}

fn parts_of(engine: &Engine, root: &Path, tick: u64) -> Vec<Mat4> {
    let scene = extract(engine, root, tick);
    scene.meshes[..PARTS].iter().map(|m| m.model).collect()
}

/// A foot's effector in its lower part's mesh frame.
fn effector(root: &Path, name: &str) -> Vec3 {
    let data = ludion_engine::load_assembly(ludion_assets::AssetRoot::content(root), GROUNDED)
        .expect("the grounded humanoid loads");
    let entry = data
        .ik
        .iter()
        .find(|e| e.name == name)
        .expect("the chain exists");
    let ludion_engine::AssemblyIkKind::Chain { effector, .. } = &entry.kind else {
        panic!("a chain");
    };
    Vec3::new(effector[0] as f32, effector[1] as f32, effector[2] as f32)
}

fn sole(root: &Path, models: &[Mat4], left: bool) -> Vec3 {
    let (part, name) = if left {
        (LOWER_LEG_L, "foot_l")
    } else {
        (LOWER_LEG_R, "foot_r")
    };
    models[part].transform_point3(effector(root, name))
}

fn close(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 1e-4, "{what}: {a} vs {b}");
}

/// A flat floor and the grounded humanoid with the sole positions it poses at.
struct Baseline {
    root: PathBuf,
    models: Vec<Mat4>,
    left: Vec3,
}

fn baseline(name: &str, animation: &str) -> Baseline {
    let root = fixture_project(name);
    let engine = engine_at(&root, &scene(GROUNDED, animation, &floor()));
    let models = parts_of(&engine, &root, 20);
    let left = sole(&root, &models, true);
    Baseline { root, models, left }
}

fn under_left(base: &Baseline, name: &str, top: f32, thickness: f32) -> String {
    block(
        name,
        [base.left.x, top - thickness * 0.5, base.left.z],
        [0.15, thickness * 0.5, 0.15],
    )
}

#[test]
fn flat_ground_at_the_floor_poses_bit_for_bit_as_without_ground() {
    let root = fixture_project("flat");
    for (name, animation) in [("rest", ""), ("walk", WALK)] {
        for tick in [0, 20, 33] {
            let plain = parts_of(
                &engine_at(&root, &scene(PLAIN, animation, &floor())),
                &root,
                tick,
            );
            let grounded = parts_of(
                &engine_at(&root, &scene(GROUNDED, animation, &floor())),
                &root,
                tick,
            );
            assert_eq!(grounded, plain, "{name} tick {tick}: dead band");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_any_collider_the_pose_is_the_ungrounded_pose() {
    let root = fixture_project("nocollider");
    for animation in ["", WALK] {
        let plain = parts_of(&engine_at(&root, &scene(PLAIN, animation, "")), &root, 20);
        let grounded = parts_of(
            &engine_at(&root, &scene(GROUNDED, animation, "")),
            &root,
            20,
        );
        assert_eq!(grounded, plain);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_foot_over_a_step_lands_on_it_and_the_other_leg_is_untouched() {
    let base = baseline("step", "");
    let top = 2.0 * VOXEL;
    let extras = format!("{}{}", floor(), under_left(&base, "step", top, 0.25));
    let engine = engine_at(&base.root, &scene(GROUNDED, "", &extras));
    let models = parts_of(&engine, &base.root, 20);
    close(
        sole(&base.root, &models, true).y,
        top,
        "the left sole is on the step",
    );
    close(base.left.y, 0.0, "the baseline sole is on the floor");
    for part in [UPPER_LEG_R, LOWER_LEG_R] {
        assert_eq!(models[part], base.models[part], "right leg part {part}");
    }
    let knee = |models: &[Mat4]| ludion_core::math::mat4_inverse(models[6]) * models[LOWER_LEG_L];
    let (bent, straight) = (knee(&models), knee(&base.models));
    let moved = bent
        .to_cols_array()
        .iter()
        .zip(straight.to_cols_array())
        .any(|(a, b)| (a - b).abs() > 1e-3);
    assert!(moved, "the left knee bends");
    let _ = std::fs::remove_dir_all(&base.root);
}

#[test]
fn a_step_taller_than_max_adjust_is_left_alone() {
    let base = baseline("tall", "");
    // `max_adjust` is 3 voxels; the step is 4.
    let extras = format!("{}{}", floor(), under_left(&base, "step", 4.0 * VOXEL, 0.5));
    let engine = engine_at(&base.root, &scene(GROUNDED, "", &extras));
    assert_eq!(parts_of(&engine, &base.root, 20), base.models);
    let _ = std::fs::remove_dir_all(&base.root);
}

#[test]
fn root_drop_lowers_the_assembly_so_the_lower_sole_reaches() {
    let base = baseline("drop", "");
    let dip = 2.0 * VOXEL;
    // The floor is a dip below the left foot's plateau, which keeps the left sole at y = 0.
    let extras = format!(
        "{}{}",
        block("pit", [0.0, -dip - 0.375, 0.0], [3.0, 0.375, 3.0]),
        under_left(&base, "plateau", 0.0, 0.25)
    );
    let engine = engine_at(&base.root, &scene(GROUNDED, "", &extras));
    let models = parts_of(&engine, &base.root, 20);
    for (part, model) in models.iter().enumerate().take(6) {
        close(
            model.w_axis.y,
            base.models[part].w_axis.y - dip,
            &format!("part {part} is lowered by two voxels"),
        );
    }
    close(
        sole(&base.root, &models, false).y,
        -dip,
        "the right sole is on the dip",
    );
    close(
        sole(&base.root, &models, true).y,
        0.0,
        "the left sole stays on the plateau",
    );
    let _ = std::fs::remove_dir_all(&base.root);
}

#[test]
fn an_active_goal_on_a_grounded_chain_wins_over_the_ground() {
    let base = baseline("goal", "");
    let target = base.left + Vec3::new(0.0, 0.1, 0.12);
    let goal = format!(
        "      IkTargets:
        targets:
          foot_l:
            point:
              - {}
              - {}
              - {}
            weight: 1.0
",
        target.x, target.y, target.z
    );
    let extras = format!(
        "{}{}",
        floor(),
        under_left(&base, "step", 2.0 * VOXEL, 0.25)
    );
    let engine = engine_at(&base.root, &scene(GROUNDED, &goal, &extras));
    let models = parts_of(&engine, &base.root, 20);
    let reached = sole(&base.root, &models, true);
    assert!(
        (reached - target).length() < 1e-3,
        "{reached:?} vs {target:?}"
    );
    let _ = std::fs::remove_dir_all(&base.root);
}

const SOCKET: &str = "  - name: socket
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
        joint: lower-leg-l
";

fn local() -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(0.5),
        Quat::from_rotation_y(30f32.to_radians()),
        Vec3::new(1.0, 2.0, 3.0),
    )
}

fn world_mat(pose: ludion_engine::WorldPose) -> Mat4 {
    let [x, y, z, t] = pose.trs.to_affine().cols.map(Vec3::from);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
}

fn assert_close(actual: Mat4, expected: Mat4, what: &str) {
    for (a, e) in actual.to_cols_array().iter().zip(expected.to_cols_array()) {
        assert!(
            (a - e).abs() <= 1e-4 * e.abs().max(1.0),
            "{what}: {actual:?} vs {expected:?}"
        );
    }
}

#[test]
fn a_socket_follows_the_grounded_part_while_the_sim_keeps_the_rest_socket() {
    let base = baseline("socket", "");
    let step = under_left(&base, "step", 2.0 * VOXEL, 0.25);
    let with_step = format!("{SOCKET}{}{step}", floor());
    let without = format!("{SOCKET}{}", floor());
    let engine = engine_at(&base.root, &scene(GROUNDED, "", &with_step));
    let flat = engine_at(&base.root, &scene(GROUNDED, "", &without));
    let humanoid = engine.world().entities().next().unwrap();
    let socket_id = engine.world().entities().nth(1).unwrap();

    let data =
        ludion_engine::load_assembly(ludion_assets::AssetRoot::content(&base.root), GROUNDED)
            .unwrap();
    let part = &data.parts[LOWER_LEG_L];
    assert_eq!(part.name, "lower-leg-l");
    let pivot = Mat4::from_translation(Vec3::from(part.pivot));

    let scene = extract(&engine, &base.root, 20);
    let part_model = scene.meshes[LOWER_LEG_L].model;
    let socket = scene.meshes[PARTS].model;
    assert_close(
        socket,
        part_model * pivot * local(),
        "the draw rides the grounded part",
    );
    let flat_scene = extract(&flat, &base.root, 20);
    assert_ne!(
        part_model, flat_scene.meshes[LOWER_LEG_L].model,
        "the foot moved"
    );

    let pose = posed_joint_world(engine.world(), humanoid, "lower-leg-l", at(20))
        .expect("a host read is allowed")
        .expect("the part resolves");
    assert_close(
        world_mat(pose) * local(),
        socket,
        "posed_joint_world follows the grounded part",
    );

    // Decision 42: the sim socket is the rest socket, whatever the ground does.
    let rest = Mat4::from_translation(Vec3::from(part.offset) + Vec3::from(part.pivot));
    let sim = world_transform(engine.world(), socket_id).unwrap();
    assert_close(
        world_mat(sim),
        world_mat(world_transform(engine.world(), humanoid).unwrap()) * rest * local(),
        "the sim reads the rest socket",
    );
    assert_eq!(sim, world_transform(flat.world(), socket_id).unwrap());
    let _ = std::fs::remove_dir_all(&base.root);
}

#[test]
fn extraction_is_repeatable_hash_neutral_and_thread_count_invariant() {
    let base = baseline("hash", WALK);
    let extras = format!(
        "{}{}",
        floor(),
        under_left(&base, "step", 2.0 * VOXEL, 0.25)
    );
    let text = scene(GROUNDED, WALK, &extras);

    let engine = engine_at(&base.root, &text);
    let once = parts_of(&engine, &base.root, 30);
    assert_eq!(
        once,
        parts_of(&engine, &base.root, 30),
        "the same frame twice"
    );
    for threads in [1, 2, 8] {
        let drawn = ludion_core::par::with_threads(threads, || parts_of(&engine, &base.root, 30));
        assert_eq!(drawn, once, "{threads} threads");
    }

    let run = |extract_each_tick: bool| {
        let mut engine = engine_at(&base.root, &text);
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
                let scene = extract(&engine, &base.root, engine.frame().as_u64());
                assert_eq!(scene.meshes.len(), PARTS);
            }
        }
        trace
    };
    let quiet = run(false);
    assert_eq!(quiet.len(), 40);
    assert_eq!(quiet, run(true), "extracting must not move a world hash");
    let _ = std::fs::remove_dir_all(&base.root);
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(gallery().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_gallery_files_are_canonical_and_the_scene_validates() {
    let text = read(GROUNDED);
    let doc = ludion_scene::check_assembly(&text).expect("the grounded humanoid checks");
    assert!(doc.ground.is_some());
    assert_eq!(ludion_scene::emit_assembly(&doc), text);

    let mut engine = Engine::with_project_root(gallery());
    engine
        .load_inline_scene(&read("assembly-ground.yaml"))
        .expect("the scene validates");
}
