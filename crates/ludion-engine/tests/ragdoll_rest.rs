//! `switch_to_ragdoll_with` and `LimitsFrom::Rest` (Neon Shroud request 0008 W1): a posed voxel switch
//! measures joint limits, `JointMotor` targets and `JointPoses` from the rig rest pose when asked,
//! however the body fell; the zero is a stored `JointZero` that survives a snapshot; the default
//! options are `switch_to_ragdoll` exactly; and the switch samples a layered assembly's drawn pose.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::{Mat4, Quat};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::{Component, World};
use ludion_engine::{
    AssemblyData, Engine, LimitsFrom, QualityParams, RagdollSwitch, Transform, extract_at,
    load_assembly, switch_to_ragdoll, switch_to_ragdoll_with,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Collider, Joint, JointMotor, JointPoses, JointZero, PoseBlend};

const ASSEMBLY_REL: &str = "models/figure.assembly.yaml";

/// A torso (the root) and an `arm` hinged to it about +Z, limited to [-45, 45] degrees. Parts hold a
/// single voxel, so the arm is light against the torso.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: figure
size:
  - 5
  - 6
  - 2
palette:
  A:
    color:
      - 0.8
      - 0.2
      - 0.2
      - 1.0
parts:
  - name: torso
    file: parts/torso.part.yaml
    place:
      - 2
      - 2
      - 0
  - name: arm
    file: parts/limb.part.yaml
    parent: torso
    place:
      - 4
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
joints:
  arm:
    kind: hinge
    axis:
      - 0.0
      - 0.0
      - 1.0
    limits:
      - -45.0
      - 45.0
clips:
  bend: anims/bend.anim.yaml
masks:
  arm_only:
    joints:
      - arm
";

const TORSO: &str = "\
format: 1
kind: part
size:
  - 2
  - 2
  - 2
slices:
  -
    - \"AA\"
    - \"AA\"
  -
    - \"AA\"
    - \"AA\"
";

const LIMB: &str = "\
format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - \"A\"
";

/// Bends the arm from rest to 80 degrees about +Z over one looping second: 40 degrees at tick 30.
const BEND_CLIP: &str = "\
format: 1
kind: animation
name: bend
duration: 1.0
loop: true
tracks:
  arm:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - 80.0
";

/// The switch instant: mid-clip, the arm 40 degrees past rest.
const MID_CLIP: u64 = 30;

fn fixture_project(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("ludion-ragdoll-rest-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in [
        (ASSEMBLY_REL, ASSEMBLY),
        ("models/parts/torso.part.yaml", TORSO),
        ("models/parts/limb.part.yaml", LIMB),
        ("models/anims/bend.anim.yaml", BEND_CLIP),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn assembly_data(root: &Path) -> AssemblyData {
    load_assembly(ludion_assets::AssetRoot::content(root), ASSEMBLY_REL)
        .expect("the fixture assembly loads")
}

/// A free-gravity physics scene with a far-away floor and the figure, which plays `bend` and carries
/// `layers` (an `AnimationLayers` block, or none).
fn scene(animated: bool, layers: &str) -> String {
    let animation = if animated {
        "      Animation:\n        clip: bend\n"
    } else {
        ""
    };
    format!(
        "format: 1\nseed: 7\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -1000.0\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 50.0\n              - 0.5\n              - 50.0\n        body: static\n  - name: figure\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 5.0\n          - 0.0\n      Mesh:\n        shape:\n          assembly: {ASSEMBLY_REL}\n{animation}{layers}"
    )
}

fn engine_for(root: &Path, text: &str) -> Engine {
    let mut engine = Engine::builder()
        .project_root(root.to_path_buf())
        .build()
        .expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

/// `floor` is document position 0, `figure` 1.
const FIGURE: EntityId = EntityId::new(1, 0);

fn get<T: Component + Clone>(world: &World, id: EntityId) -> Option<T> {
    world.get::<T>(id).ok().flatten().cloned()
}

fn at(tick: u64) -> SimTime {
    SimTime::from_ticks(tick, TickRate::DEFAULT)
}

/// A drive stiff enough to settle the light arm within a second, and no stiffer.
fn drive() -> JointMotor {
    JointMotor {
        target: vec![0.0],
        stiffness: 400.0,
        damping: 60.0,
        max_force: None,
    }
}

/// The arm's hinge angle about +Z relative to the torso, in degrees.
fn hinge_degrees(world: &World, spawned: &[EntityId]) -> f32 {
    let quat = |id: EntityId| {
        let t = get::<Transform>(world, id).expect("a part is alive");
        Quat::from_array(euler_xyz_degrees_to_quat(t.rotation))
    };
    let relative = quat(spawned[0]).inverse() * quat(spawned[1]);
    let relative = if relative.w < 0.0 {
        -relative
    } else {
        relative
    };
    (2.0 * ludion_core::math::atan2(relative.z, relative.w)).to_degrees()
}

fn advance(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

/// Switches the animated figure at `tick` (with `opts`, or through `switch_to_ragdoll` when `None`),
/// optionally drives the arm to its zero, and falls `ticks`.
struct Run {
    engine: Engine,
    spawned: Vec<EntityId>,
    hashes: Vec<u64>,
    root: PathBuf,
}

fn run(name: &str, opts: Option<RagdollSwitch>, tick: u64, motor: bool, ticks: usize) -> Run {
    let root = fixture_project(name);
    let data = assembly_data(&root);
    let mut engine = engine_for(&root, &scene(true, ""));
    let spawned = match opts {
        Some(opts) => switch_to_ragdoll_with(engine.world_mut(), FIGURE, &data, at(tick), opts),
        None => switch_to_ragdoll(engine.world_mut(), FIGURE, &data, at(tick)),
    }
    .expect("a valid switch");
    if motor {
        engine.world_mut().insert(spawned[1], drive()).unwrap();
    }
    let hashes = advance(&mut engine, ticks);
    Run {
        engine,
        spawned,
        hashes,
        root,
    }
}

fn rest() -> Option<RagdollSwitch> {
    let mut opts = RagdollSwitch::default();
    opts.limits_from = LimitsFrom::Rest;
    Some(opts)
}

#[test]
fn default_options_are_switch_to_ragdoll_exactly() {
    let plain = run("default-plain", None, MID_CLIP, false, 90);
    let with = run(
        "default-with",
        Some(RagdollSwitch::default()),
        MID_CLIP,
        false,
        90,
    );
    for (&a, &b) in plain.spawned.iter().zip(&with.spawned) {
        assert_eq!(
            get::<Transform>(plain.engine.world(), a),
            get::<Transform>(with.engine.world(), b)
        );
        assert_eq!(
            get::<Collider>(plain.engine.world(), a),
            get::<Collider>(with.engine.world(), b)
        );
        assert_eq!(
            get::<Joint>(plain.engine.world(), a).map(|j| (j.kind, j.anchor)),
            get::<Joint>(with.engine.world(), b).map(|j| (j.kind, j.anchor))
        );
        assert!(get::<JointZero>(with.engine.world(), b).is_none());
    }
    assert_eq!(plain.hashes, with.hashes, "the fall hash sequence differs");
    for r in [plain, with] {
        let _ = std::fs::remove_dir_all(r.root);
    }
}

#[test]
fn a_rest_switch_stores_a_zero_on_each_joint_carrier_only() {
    let r = run("zeros", rest(), MID_CLIP, false, 0);
    assert!(get::<JointZero>(r.engine.world(), r.spawned[0]).is_none());
    assert_eq!(
        get::<JointZero>(r.engine.world(), r.spawned[1]),
        Some(JointZero::default())
    );
    let _ = std::fs::remove_dir_all(r.root);
}

#[test]
fn a_mid_clip_rest_switch_drives_the_hinge_back_to_rest() {
    let r = run("rest-motor", rest(), MID_CLIP, true, 300);
    let angle = hinge_degrees(r.engine.world(), &r.spawned);
    assert!(
        angle.abs() <= 3.0,
        "the arm settled at {angle} degrees, expected about 0"
    );
    let _ = std::fs::remove_dir_all(r.root);
}

#[test]
fn a_mid_clip_switch_pose_switch_holds_the_clip_angle() {
    let r = run("pose-motor", None, MID_CLIP, true, 300);
    let angle = hinge_degrees(r.engine.world(), &r.spawned);
    assert!(
        (angle - 40.0).abs() <= 3.0,
        "the arm held {angle} degrees, expected about 40"
    );
    let _ = std::fs::remove_dir_all(r.root);
}

/// A named pose is measured from the rig rest zero: 20 degrees whether the figure fell from the
/// rest pose or from 40 degrees into the clip.
#[test]
fn a_joint_pose_gives_the_same_angle_from_a_mid_clip_and_a_rest_switch() {
    let settle = |name: &str, tick: u64| {
        let root = fixture_project(name);
        let data = assembly_data(&root);
        let mut engine = engine_for(&root, &scene(true, ""));
        let spawned =
            switch_to_ragdoll_with(engine.world_mut(), FIGURE, &data, at(tick), rest().unwrap())
                .expect("a valid switch");
        let mut poses = JointPoses {
            group: Some(spawned[0]),
            stiffness: 400.0,
            damping: 60.0,
            max_force: None,
            poses: Default::default(),
        };
        poses.poses.insert("raised".to_string(), vec![20.0]);
        engine.world_mut().insert(spawned[1], poses).unwrap();
        engine
            .world_mut()
            .insert(
                spawned[0],
                PoseBlend {
                    pose: "raised".to_string(),
                    weight: 1.0,
                },
            )
            .unwrap();
        advance(&mut engine, 300);
        let angle = hinge_degrees(engine.world(), &spawned);
        let _ = std::fs::remove_dir_all(root);
        angle
    };
    let mid = settle("pose-mid", MID_CLIP);
    let at_rest = settle("pose-rest", 0);
    assert!((mid - 20.0).abs() <= 3.0, "mid-clip settled at {mid}");
    assert!((at_rest - 20.0).abs() <= 3.0, "rest settled at {at_rest}");
    assert!((mid - at_rest).abs() <= 2.0, "{mid} vs {at_rest}");
}

fn snapshot_of(engine: &mut Engine) -> serde_json::Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: serde_json::Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

#[test]
fn a_snapshot_mid_fall_keeps_the_zeros_and_the_hash_sequence() {
    let full = run("snap-full", rest(), MID_CLIP, true, 120);

    let mut head = run("snap-head", rest(), MID_CLIP, true, 40);
    let snapshot = snapshot_of(&mut head.engine);
    let root = fixture_project("snap-tail");
    let mut resumed = engine_for(&root, &scene(true, ""));
    restore(&mut resumed, snapshot);
    assert_eq!(
        get::<JointZero>(resumed.world(), head.spawned[1]),
        Some(JointZero::default()),
        "the restored carrier lost its zero"
    );
    let tail = advance(&mut resumed, 80);
    let mut joined = head.hashes.clone();
    joined.extend(tail);
    assert_eq!(
        joined, full.hashes,
        "a resumed fall differs from the uninterrupted one"
    );
    let angle = hinge_degrees(resumed.world(), &head.spawned);
    assert!(
        angle.abs() <= 8.0,
        "the restored arm is heading to rest: {angle}"
    );
    for dir in [full.root, head.root, root] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn the_rest_fall_is_identical_at_1_2_8_threads() {
    let traces: Vec<Vec<u64>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let r = run(&format!("threads-{n}"), rest(), MID_CLIP, true, 90);
                let _ = std::fs::remove_dir_all(&r.root);
                r.hashes
            })
        })
        .collect();
    assert_eq!(traces[0], traces[1], "1 vs 2 threads");
    assert_eq!(traces[0], traces[2], "1 vs 8 threads");
}

fn model_matrices(world: &World, root: &Path, now: SimTime) -> Vec<Mat4> {
    extract_at(
        world,
        (64, 64),
        ludion_assets::AssetRoot::content(root),
        now,
        QualityParams::FULL.particle_density,
    )
    .expect("extraction succeeds")
    .meshes
    .iter()
    .map(|m| m.model)
    .collect()
}

/// A masked override layer bends the arm, with no base clip: the ragdoll spawns at the drawn pose, not
/// the layerless one.
#[test]
fn a_layered_assembly_switches_at_the_drawn_pose() {
    let layers = "      AnimationLayers:\n        layers:\n          - name: bend\n            clip: bend\n            mask: arm_only\n            weight: 1.0\n";
    for (label, options) in [("default", None), ("rest", rest())] {
        let root = fixture_project(&format!("layered-{label}"));
        let data = assembly_data(&root);
        let mut engine = engine_for(&root, &scene(false, layers));
        let now = at(MID_CLIP);

        let layerless = {
            let plain = engine_for(&root, &scene(false, ""));
            model_matrices(plain.world(), &root, now)
        };
        let pre = model_matrices(engine.world(), &root, now);
        assert_eq!(pre.len(), 2);
        assert!(
            !pre[1].abs_diff_eq(layerless[1], 1e-3),
            "{label}: the layer must move the arm for this test to mean anything"
        );

        match options {
            Some(opts) => switch_to_ragdoll_with(engine.world_mut(), FIGURE, &data, now, opts),
            None => switch_to_ragdoll(engine.world_mut(), FIGURE, &data, now),
        }
        .expect("a valid switch");
        let post = model_matrices(engine.world(), &root, now);
        assert_eq!(post.len(), 2, "{label}");
        for (index, (a, b)) in pre.iter().zip(&post).enumerate() {
            assert!(
                a.abs_diff_eq(*b, 1e-4),
                "{label} part {index}: {a:?} vs {b:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
