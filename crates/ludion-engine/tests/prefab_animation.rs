//! Runtime proof for prefab clip playback (`Plan/animation-clips.md`, the imported node clips): an
//! `Animation` on an instance's root part poses the parts of that instance the clip has tracks for,
//! `extract()` evaluates it, and the simulation never notices.
//!
//! The fixture is `examples/import-node-clips/expected`, the hinge a rigid glTF animation imports
//! to: `base` (no track), `lid` (rotation about its hinge) and `latch` (translation), with an
//! `open` and a stepped `snap` clip. It is copied into a temp project so the resolution rules (the
//! clips are relative to the prefab file) are part of what is under test.

use std::path::{Path, PathBuf};

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, Transform, extract_at};
use ludion_protocol::{Command, command};
use ludion_renderer::{BlendData, MeshInstance, RenderScene};

/// How many parts one hinge instance contributes to `RenderScene.meshes`: `[base, lid, latch]`.
const PARTS: usize = 3;
const BASE: usize = 0;
const LID: usize = 1;
const LATCH: usize = 2;

/// Three instances of the hinge: one playing `open`, one playing `snap`, one with no `Animation`.
/// The meshes come out `[open ×3, snap ×3, still ×3]`.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
  - name: snapping
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: snap
  - name: still
    prefab: hinge/model.prefab.yaml
";

/// A one-second crossfade from `open` into `snap` from tick 0, beside the two plain instances it
/// blends between: `[fading ×3, open ×3, snap ×3]`.
const FADE_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: fading
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: snap
          from: open
          fade: 1.0
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
  - name: snapping
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: snap
";

/// Two instances playing `open`, the second moved and turned: `[plain ×3, placed ×3]`.
const PLACED_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: plain
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
  - name: placed
    prefab: hinge/model.prefab.yaml
    transform:
      translation:
        - 5.0
        - 0.0
        - 2.0
      rotation:
        - 0.0
        - 90.0
        - 0.0
    overrides:
      base:
        Animation:
          clip: open
";

/// A fresh project directory unique to this process and `name`, carrying the hinge prefab under
/// `hinge/`.
fn fixture_project(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-prefab-anim-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected");
    for file in [
        "model.prefab.yaml",
        "clips/open.anim.yaml",
        "clips/snap.anim.yaml",
        "meshes/base.mesh.yaml",
        "meshes/lid.mesh.yaml",
        "meshes/latch.mesh.yaml",
    ] {
        let to = dir.join("hinge").join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(source.join(file), to).unwrap();
    }
    dir
}

fn engine_with(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

/// One `sim.step` of `ticks` ticks.
fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The mesh instances of `engine`'s world posed at `tick`, through the public extract entry point.
fn posed_at(engine: &Engine, tick: u64) -> Vec<MeshInstance> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes
}

/// The lid's turn about +X in degrees, read off its model matrix (the hinge's base sits at the
/// origin unrotated, so the lid's rotation is the whole upper 3x3).
fn lid_degrees(instance: &MeshInstance) -> f32 {
    ludion_core::math::atan2(instance.model.y_axis.z, instance.model.y_axis.y).to_degrees()
}

/// Two instances of one prefab, one playing `open` and one `snap`, pose independently: each lid
/// leaves its rest and differs from the other's, the instance with no `Animation` draws exactly its
/// tick-0 matrices, and `base` (no track) never moves.
#[test]
fn two_instances_of_one_prefab_play_their_own_clips() {
    let root = fixture_project("independent");
    let engine = engine_with(&root, SCENE);
    let (open, snap, still) = (0, PARTS, 2 * PARTS);

    let start = posed_at(&engine, 0);
    let mid = posed_at(&engine, 18);

    let lid = |posed: &[MeshInstance], instance: usize| posed[instance + LID].model;
    assert_ne!(
        lid(&mid, open),
        lid(&start, open),
        "open's lid left its rest"
    );
    assert_ne!(
        lid(&mid, snap),
        lid(&start, snap),
        "snap's lid left its rest"
    );
    assert_ne!(
        lid(&mid, open),
        lid(&mid, snap),
        "the two clips differ at 0.3 s"
    );
    assert!((lid_degrees(&mid[open + LID]) + 30.0).abs() < 0.01);
    assert!((lid_degrees(&mid[snap + LID]) + 50.0).abs() < 0.01);

    for part in 0..PARTS {
        assert_eq!(
            mid[still + part].model,
            start[still + part].model,
            "the instance with no Animation draws its tick-0 matrices"
        );
        assert_eq!(
            mid[still + part].normal_matrix,
            start[still + part].normal_matrix
        );
    }
    for instance in [open, snap] {
        assert_eq!(
            mid[instance + BASE].model,
            mid[still + BASE].model,
            "base has no track, so it never moves"
        );
    }
    assert_ne!(
        mid[open + LATCH].model,
        mid[still + LATCH].model,
        "the latch is the lid's child and follows it"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A point light under a posed part follows the clip in the extracted scene while its stored
/// `Transform` does not move.
#[test]
fn a_point_light_under_a_posed_part_follows_the_clip() {
    let root = fixture_project("light");
    let scene = "\
format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
      latch:
        PointLight: {}
";
    let engine = engine_with(&root, scene);
    let lights_at = |tick: u64| {
        extract_at(
            engine.world(),
            (320, 180),
            ludion_assets::AssetRoot::content(engine.project_root()),
            SimTime::from_ticks(tick, TickRate::DEFAULT),
            QualityParams::FULL.particle_density,
        )
        .expect("the fixture world extracts")
        .lights
    };
    let (start, mid) = (lights_at(0), lights_at(18));
    assert_eq!(start.len(), 1);
    assert_eq!(mid.len(), 1);
    assert_ne!(
        mid[0].position, start[0].position,
        "the light rode the lid's clip"
    );
    let latch = engine
        .world()
        .entities()
        .find(|&id| {
            engine
                .world()
                .get::<ludion_engine::PointLight>(id)
                .unwrap()
                .is_some()
        })
        .expect("the latch carries the light");
    let stored = engine
        .world()
        .get::<Transform>(latch)
        .unwrap()
        .unwrap()
        .translation;
    let _ = lights_at(18);
    assert_eq!(
        engine
            .world()
            .get::<Transform>(latch)
            .unwrap()
            .unwrap()
            .translation,
        stored,
        "the stored Transform never moves"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The sim half: an animated run's per-frame `WorldHash` trace and recorded `.lrp` are identical
/// whether or not frames are extracted along the way, and no stored `Transform` changes.
#[test]
fn extracting_an_animated_prefab_run_changes_neither_its_hashes_nor_its_recording() {
    let root = fixture_project("hash-neutral");

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_with(&root, SCENE);
        let stored = |engine: &Engine| -> Vec<Transform> {
            let world = engine.world();
            world
                .entities()
                .filter_map(|id| world.get::<Transform>(id).ok().flatten().copied())
                .collect()
        };
        let before = stored(&engine);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                let posed = posed_at(&engine, engine.frame().as_u64());
                assert_eq!(posed.len(), 3 * PARTS);
            }
        }
        engine.stop_recording().expect("recording stops");
        assert_eq!(
            before,
            stored(&engine),
            "the stored part transforms never move"
        );
        (trace, std::fs::read(&lrp).unwrap())
    };

    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);

    assert_eq!(quiet_trace.len(), 40, "the run actually advanced 40 frames");
    assert_eq!(
        quiet_trace, drawn_trace,
        "extracting an animated frame must not move a single world hash"
    );
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extracting an animated frame must not write a byte into the recording"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A placed instance plays the clip relative to its placement: its lid's world matrix is the
/// placement applied to the unplaced instance's.
#[test]
fn a_placed_instance_plays_relative_to_its_placement() {
    let root = fixture_project("placed");
    let engine = engine_with(&root, PLACED_SCENE);
    let (plain, placed) = (0, PARTS);

    let placement = ludion_core::math::glam::Mat4::from_translation(
        ludion_core::math::glam::Vec3::new(5.0, 0.0, 2.0),
    ) * ludion_core::math::glam::Mat4::from_rotation_y(90f32.to_radians());
    for tick in [0, 18, 36] {
        let posed = posed_at(&engine, tick);
        for part in [BASE, LID, LATCH] {
            let want = placement * posed[plain + part].model;
            let got = posed[placed + part].model;
            assert!(
                want.abs_diff_eq(got, 1e-4),
                "tick {tick}, part {part}: {want:?} vs {got:?}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// Mid-crossfade the lid sits strictly between the outgoing `open` pose and the incoming `snap`
/// pose; at and after the fade's end the instance is byte-identical to one that only ever played
/// `snap`, so the blend leaves no residue.
#[test]
fn a_crossfade_blends_then_lands_exactly_on_the_incoming_clip() {
    let root = fixture_project("crossfade");
    let engine = engine_with(&root, FADE_SCENE);
    let (fading, outgoing, incoming) = (0, PARTS, 2 * PARTS);

    let mid = posed_at(&engine, 30);
    let (blend, from, to) = (
        lid_degrees(&mid[fading + LID]),
        lid_degrees(&mid[outgoing + LID]),
        lid_degrees(&mid[incoming + LID]),
    );
    assert!(
        to < blend && blend < from,
        "mid-fade {blend} must lie strictly between {from} and {to}"
    );

    for tick in [60, 90] {
        let posed = posed_at(&engine, tick);
        for part in 0..PARTS {
            assert_eq!(posed[fading + part].model, posed[incoming + part].model);
            assert_eq!(
                posed[fading + part].normal_matrix,
                posed[incoming + part].normal_matrix
            );
        }
    }

    let _ = std::fs::remove_dir_all(&root);
}

const HINGE: &str = "\
format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
";

/// Two consecutive presented ticks of the lone hinge, mid-clip so the lid moves between them.
fn presented_pair(engine: &mut Engine) -> [(RenderScene, BlendData); 2] {
    engine.execute(step(18)).expect("the sim steps");
    let first = engine.extract_present_scene().expect("tick extracts");
    engine.execute(step(1)).expect("the sim steps");
    let second = engine.extract_present_scene().expect("tick extracts");
    [first, second]
}

/// Under present interpolation a prefab part's clip pose takes the newer tick (display-paced-present
/// decision 15): blending two ticks leaves the lid exactly at the newer tick's pose.
#[test]
fn the_presenter_keeps_a_prefab_clip_pose_at_the_newer_tick() {
    let root = fixture_project("present-clip");
    let mut engine = engine_with(&root, HINGE);
    let [(s0, d0), (s1, d1)] = presented_pair(&mut engine);
    assert_ne!(
        lid_degrees(&s0.meshes[LID]),
        lid_degrees(&s1.meshes[LID]),
        "the clip moved the lid between the two ticks"
    );
    let mid = ludion_renderer::blend(&s0, &d0, &s1, &d1, 0.5);
    assert_eq!(mid.meshes[LID].model, s1.meshes[LID].model);
    let _ = std::fs::remove_dir_all(&root);
}

/// Root motion still blends: the instance root moved +1 in x between ticks lands at the midpoint.
#[test]
fn the_presenter_still_blends_an_animated_prefabs_root_motion() {
    let root = fixture_project("present-root");
    let mut engine = engine_with(&root, HINGE);
    engine.execute(step(18)).expect("the sim steps");
    let (s0, d0) = engine.extract_present_scene().expect("tick extracts");
    let id = d0.meshes[BASE].expect("base is keyed");
    engine
        .world_mut()
        .get_mut::<Transform>(id)
        .unwrap()
        .unwrap()
        .translation[0] += 1.0;
    engine.execute(step(1)).expect("the sim steps");
    let (s1, d1) = engine.extract_present_scene().expect("tick extracts");
    let mid = ludion_renderer::blend(&s0, &d0, &s1, &d1, 0.5);
    let (x0, x1) = (
        s0.meshes[BASE].model.w_axis.x,
        s1.meshes[BASE].model.w_axis.x,
    );
    assert!((x1 - x0 - 1.0).abs() < 1e-5, "the root moved a metre");
    assert!((mid.meshes[BASE].model.w_axis.x - (x0 + 0.5)).abs() < 1e-5);
    let _ = std::fs::remove_dir_all(&root);
}

/// `hold: 5` on a prefab root, plain and mid-crossfade: ticks 10..=14 pose identically, tick 15 is
/// the next pose.
#[test]
fn a_hold_steps_the_prefab_pose_on_a_global_grid() {
    let root = fixture_project("hold-grid");
    let scene = FADE_SCENE.replace("Animation:\n", "Animation:\n          hold: 5\n");
    let engine = engine_with(&root, &scene);
    let held = posed_at(&engine, 10);
    for tick in 11..=14 {
        let posed = posed_at(&engine, tick);
        for (a, b) in held.iter().zip(&posed) {
            assert_eq!(a.model, b.model, "tick {tick}");
        }
    }
    let next = posed_at(&engine, 15);
    for entity in 0..3 {
        let lid = entity * PARTS + LID;
        assert_ne!(
            held[lid].model, next[lid].model,
            "instance {entity} must step at tick 15"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
