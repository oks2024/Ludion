//! Runtime proof for assembly IK (`Plan/animation-layers.md` decisions 36-38, Neon Shroud request
//! 0007 W4a): an `ik:` chain and look-at posed in extract from an `IkTargets` component.
//!
//! - **Neutral.** No component, a weight of `0` and an assembly with no `ik:` pose bit for bit like
//!   the plain clip.
//! - **Chain.** A world point and a followed entity put the effector where the target is.
//! - **Look-at.** A direction turns the head and stops at the yaw limit.
//!
//! The fixture is a torso with a head and a two-part arm hanging down `-Y`, in a scratch directory.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};

const TORSO: usize = 0;
const LOWER: usize = 2;
const HEAD: usize = 3;

/// The `ik:` block shared by the fixtures that have one.
const IK: &str = "ik:
  hand_r:
    parts:
      - upper-arm-r
      - lower-arm-r
    effector:
      - 0.5
      - 0.0
      - 0.5
  look:
    aim: look
    limits_deg:
      yaw: 70.0
";

const ASSEMBLY: &str = "format: 1
kind: assembly
name: humanoid
size:
  - 3
  - 5
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: torso
    file: parts/cell.part.yaml
    place:
      - 1
      - 3
      - 0
  - name: upper-arm-r
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
  - name: lower-arm-r
    file: parts/cell.part.yaml
    parent: upper-arm-r
    place:
      - 1
      - 1
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
  - name: head
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 4
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
clips:
  sway: anims/sway.anim.yaml
masks:
  look:
    joints:
      - head
";

const SWAY: &str = "format: 1
kind: animation
name: sway
duration: 1.0
loop: true
tracks:
  upper-arm-r:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 20.0
          - 0.0
          - 0.0
";

/// The assembly-frame target of the arm: the shoulder is at `(0, 2.5, 0)`, a distance of `1.237`
/// from here, inside the arm's reach of `1.5`.
const TARGET: [f32; 3] = [0.6, 1.9, 0.9];
/// The entity's own translation, which the world target adds to.
const BASE: [f32; 3] = [5.0, 0.0, 0.0];

fn fixture_project(name: &str, ik: bool) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-assembly-ik-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assembly = if ik {
        format!("{ASSEMBLY}{IK}")
    } else {
        ASSEMBLY.to_string()
    };
    write(&dir, "models/humanoid.assembly.yaml", &assembly);
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    write(&dir, "models/anims/sway.anim.yaml", SWAY);
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The humanoid at `BASE`, playing `sway`, with `targets` (a block indented 10) as its `IkTargets`,
/// then a `ball` entity at `ball`.
fn scene(targets: &str, ball: [f32; 3]) -> String {
    let ik = if targets.is_empty() {
        String::new()
    } else {
        format!("      IkTargets:\n        targets:\n{targets}")
    };
    format!(
        "format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Transform:
        translation:
          - {:?}
          - {:?}
          - {:?}
      Mesh:
        shape:
          assembly: models/humanoid.assembly.yaml
      Animation:
        clip: sway
{ik}  - name: ball
    components:
      Transform:
        translation:
          - {:?}
          - {:?}
          - {:?}
",
        BASE[0], BASE[1], BASE[2], ball[0], ball[1], ball[2]
    )
}

fn engine_at(root: &Path, text: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn posed_at(engine: &Engine, tick: u64) -> Vec<Mat4> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes
    .iter()
    .map(|mesh| mesh.model)
    .collect()
}

/// The world position of the arm's effector: the lower part's mesh origin.
fn hand(models: &[Mat4]) -> Vec3 {
    models[LOWER].transform_point3(Vec3::ZERO)
}

fn point_target(point: [f32; 3], weight: f32) -> String {
    format!(
        "          hand_r:\n            point:\n              - {:?}\n              - {:?}\n              - {:?}\n            weight: {weight:?}\n",
        point[0], point[1], point[2]
    )
}

fn world_target() -> [f32; 3] {
    [
        TARGET[0] + BASE[0],
        TARGET[1] + BASE[1],
        TARGET[2] + BASE[2],
    ]
}

fn near(a: Vec3, b: Vec3) {
    assert!((a - b).length() < 1e-4, "{a:?} vs {b:?}");
}

#[test]
fn no_component_a_zero_weight_and_no_ik_key_pose_like_the_plain_clip() {
    let with_ik = fixture_project("neutral", true);
    let without = fixture_project("neutral-plain", false);
    let plain = engine_at(&with_ik, &scene("", [0.0; 3]));
    let zero = engine_at(
        &with_ik,
        &scene(&point_target(world_target(), 0.0), [0.0; 3]),
    );
    let unmatched = engine_at(
        &with_ik,
        &scene(
            "          elbow:\n            point:\n              - 1.0\n              - 1.0\n              - 1.0\n            weight: 1.0\n",
            [0.0; 3],
        ),
    );
    let no_key = engine_at(
        &without,
        &scene(&point_target(world_target(), 1.0), [0.0; 3]),
    );
    for tick in [0, 7, 31] {
        let want = posed_at(&plain, tick);
        assert_eq!(want, posed_at(&zero, tick), "weight 0, tick {tick}");
        assert_eq!(
            want,
            posed_at(&unmatched, tick),
            "no such entry, tick {tick}"
        );
        assert_eq!(want, posed_at(&no_key, tick), "no ik key, tick {tick}");
    }
    let _ = std::fs::remove_dir_all(&with_ik);
    let _ = std::fs::remove_dir_all(&without);
}

#[test]
fn a_point_target_lands_the_effector_on_it() {
    let root = fixture_project("point", true);
    let engine = engine_at(&root, &scene(&point_target(world_target(), 1.0), [0.0; 3]));
    let plain = engine_at(&root, &scene("", [0.0; 3]));
    for tick in [0, 15, 31] {
        let posed = posed_at(&engine, tick);
        near(hand(&posed), Vec3::from(world_target()));
        assert_ne!(posed, posed_at(&plain, tick));
        // The torso and head are not in the chain.
        assert_eq!(posed[TORSO], posed_at(&plain, tick)[TORSO]);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_point_rewritten_on_a_later_tick_moves_the_effector() {
    let root = fixture_project("rewrite", true);
    let mut engine = engine_at(&root, &scene(&point_target(world_target(), 1.0), [0.0; 3]));
    near(hand(&posed_at(&engine, 0)), Vec3::from(world_target()));
    let moved = [5.2, 1.6, 0.7];
    for (_, (targets,)) in engine
        .world_mut()
        .query_ids::<(&mut ludion_engine::IkTargets,)>()
    {
        targets.targets.get_mut("hand_r").unwrap().point = Some(moved);
    }
    near(hand(&posed_at(&engine, 0)), Vec3::from(moved));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_direction_turns_the_head_and_stops_at_the_yaw_limit() {
    let root = fixture_project("look", true);
    let look = |yaw: f32| {
        format!(
            "          look:\n            direction:\n              yaw_deg: {yaw:?}\n            weight: 1.0\n"
        )
    };
    let facing = |models: &[Mat4]| models[HEAD].transform_vector3(Vec3::NEG_Z).normalize();
    let turned = |yaw: f32| {
        let (s, c) = ludion_core::math::sin_cos(yaw.to_radians());
        Vec3::new(-s, 0.0, -c)
    };
    let engine = engine_at(&root, &scene(&look(30.0), [0.0; 3]));
    let posed = posed_at(&engine, 0);
    assert!((facing(&posed) - turned(30.0)).length() < 1e-4);
    // The torso does not turn.
    let plain = posed_at(&engine_at(&root, &scene("", [0.0; 3])), 0);
    assert_eq!(posed[TORSO], plain[TORSO]);
    let clamped = engine_at(&root, &scene(&look(120.0), [0.0; 3]));
    assert!((facing(&posed_at(&clamped, 0)) - turned(70.0)).length() < 1e-4);
    let _ = std::fs::remove_dir_all(&root);
}
