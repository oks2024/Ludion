//! Runtime proof for character clip playback (`Plan/animation-clips.md` W2): an `Animation`
//! component names a clip the character binds, `extract_at` samples it onto the skin's bone palette,
//! and the simulation never notices.
//!
//! - **The sim is untouched.** An animated run's per-frame `WorldHash` trace and recorded `.lrp` are
//!   byte-identical whether or not frames are extracted — the pose is render-side only.
//! - **The sampler actually runs.** The same scene extracted at different sim times gives different
//!   palettes; an unknown clip name gives exactly the rest palette; the character's overlay speed
//!   multiplies the `Animation` speed; a scale channel composes down the joint chain.
//!
//! Runs over a scratch copy of the gallery mannequin in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};
use ludion_protocol::{Command, command};

const JOINTS: usize = 11;

/// The gallery mannequin with `run` at its overlay `speed: 1.2`, and a copy without the overlay.
const PLAIN_CHARACTER: &str = "\
format: 1
kind: character
name: plain
mesh: \"../meshes/skin-mannequin.mesh.yaml\"
clips:
  run:
    clip: \"../clips/run.anim.yaml\"
  jump:
    clip: \"../clips/jump.anim.yaml\"
";

/// Ascending entity order: `runner` (overlay `run`), `unknown` (a clip the character does not bind),
/// `still` (no `Animation`), `plain` (`run` without the overlay).
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: runner
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: run
  - name: unknown
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: sprint
  - name: still
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
  - name: plain
    components:
      Mesh:
        shape:
          character: characters/plain.character.yaml
      Animation:
        clip: run
";

/// A one-second crossfade from `jump` into `run` beginning at tick 0, beside the two plain entities it
/// blends between: `fading`, `jump` (outgoing), `run` (incoming).
const FADE_SCENE: &str = "format: 1
seed: 1
entities:
  - name: fading
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: run
        from: jump
        fade: 1.0
  - name: outgoing
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: jump
  - name: incoming
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: run
";

const JUMP_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: jumper
    components:
      Mesh:
        shape:
          character: characters/mannequin.character.yaml
      Animation:
        clip: jump
";

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-character-anim-{}-{name}",
        std::process::id()
    ));
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
    std::fs::write(dir.join("characters/plain.character.yaml"), PLAIN_CHARACTER).unwrap();
    dir
}

fn engine_at(root: &Path, scene: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// Every skinned instance's palette slice (`JOINTS` entries) at `tick`, in extract order.
fn palettes_at(engine: &Engine, tick: u64) -> Vec<Vec<Mat4>> {
    let scene = extract_at(
        engine.world(),
        (320, 180),
        engine.project_root(),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts");
    scene
        .meshes
        .iter()
        .map(|mesh| {
            // Entry 0 is the renderer's own identity, which is not in the vec.
            let start = mesh.bone_offset as usize - 1;
            scene.bone_palette[start..start + JOINTS].to_vec()
        })
        .collect()
}

fn close(a: &[Mat4], b: &[Mat4]) -> bool {
    a.iter().zip(b).all(|(a, b)| {
        a.to_cols_array()
            .iter()
            .zip(b.to_cols_array())
            .all(|(x, y)| (x - y).abs() < 1e-4)
    })
}

#[test]
fn extracting_an_animated_character_changes_neither_its_hashes_nor_its_recording() {
    assert_extraction_is_hash_neutral("hash-neutral", SCENE, 4);
}

#[test]
fn extracting_a_crossfading_character_changes_neither_its_hashes_nor_its_recording() {
    assert_extraction_is_hash_neutral("hash-neutral-fade", FADE_SCENE, 3);
}

fn assert_extraction_is_hash_neutral(name: &str, scene: &str, entities: usize) {
    let root = fixture_project(name);

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_at(&root, scene);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    .skip(1),
            );
            if extract_each_tick {
                assert_eq!(
                    palettes_at(&engine, engine.frame().as_u64()).len(),
                    entities
                );
            }
        }
        engine.stop_recording().expect("recording stops");
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

#[test]
fn a_mid_clip_tick_poses_differently_and_an_unknown_clip_stays_at_rest() {
    let root = fixture_project("mid-clip");
    let engine = engine_at(&root, SCENE);

    let (start, mid) = (palettes_at(&engine, 0), palettes_at(&engine, 12));
    let (runner, unknown, still) = (0, 1, 2);
    assert!(!close(&start[runner], &mid[runner]), "the run cycle moved");
    assert!(
        !close(&start[runner], &start[still]) || !close(&mid[runner], &mid[still]),
        "the runner is posed, the still entity is not"
    );
    assert_eq!(
        mid[unknown], mid[still],
        "an unbound clip name is exactly the rest palette"
    );
    assert_eq!(start[unknown], start[still]);

    let _ = std::fs::remove_dir_all(&root);
}

/// `run` bound at overlay `speed: 1.2`, 50 ticks in, has consumed 1.0 s of clip — the same as the
/// overlay-less binding at tick 60.
#[test]
fn the_overlay_speed_multiplies_the_animation_speed() {
    let root = fixture_project("overlay");
    let engine = engine_at(&root, SCENE);

    let (runner, plain) = (0, 3);
    let overlaid = palettes_at(&engine, 50);
    let bare = palettes_at(&engine, 60);
    assert!(close(&overlaid[runner], &bare[plain]));
    assert!(
        !close(&palettes_at(&engine, 50)[plain], &bare[plain]),
        "and the un-overlaid binding at tick 50 is a different pose"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A `set_speed` anchor poses from the same clock the events read: half speed from `ticks: 30`
/// (0.5 s) at frame 30 has consumed 0.75 s by tick 60, which is the plain speed-1 pose at tick 45.
/// This also proves the authored anchor YAML loads.
#[test]
fn an_anchored_speed_poses_at_the_re_based_clip_time() {
    const ANCHOR_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: anchored
    components:
      Mesh:
        shape:
          character: characters/plain.character.yaml
      Animation:
        clip: run
        speed: 0.5
        anchor:
          started: 0
          frame: 30
          ticks: 30.0
  - name: plain
    components:
      Mesh:
        shape:
          character: characters/plain.character.yaml
      Animation:
        clip: run
";
    let root = fixture_project("anchor");
    let engine = engine_at(&root, ANCHOR_SCENE);

    let (anchored, plain) = (0, 1);
    assert!(close(
        &palettes_at(&engine, 60)[anchored],
        &palettes_at(&engine, 45)[plain]
    ));
    assert!(
        !close(
            &palettes_at(&engine, 60)[anchored],
            &palettes_at(&engine, 60)[plain]
        ),
        "and the un-anchored clock at tick 60 is a different pose"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `jump` squashes the pelvis (y scale ≈ 0.855 at tick 10); the torso, which the clip does not
/// track, inherits it through the joint chain.
#[test]
fn a_scale_track_composes_onto_the_joints_below() {
    let root = fixture_project("squash");
    let engine = engine_at(&root, JUMP_SCENE);

    let rest = palettes_at(&engine, 0);
    let squashed = palettes_at(&engine, 10);
    let torso = 1;
    let rest_y = rest[0][torso].y_axis.truncate().length();
    let squash_y = squashed[0][torso].y_axis.truncate().length();
    assert!((rest_y - 1.0).abs() < 1e-4, "rest is unscaled: {rest_y}");
    assert!(
        (squash_y - 0.8551).abs() < 1e-3,
        "the torso carries its parent's squash: {squash_y}"
    );
    assert!(!close(&rest[0], &squashed[0]));

    let _ = std::fs::remove_dir_all(&root);
}

/// Sum of absolute matrix-entry differences between two palettes: a distance that a blend must keep
/// strictly inside the one between its two ends.
fn distance(a: &[Mat4], b: &[Mat4]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| a.to_cols_array().into_iter().zip(b.to_cols_array()))
        .map(|(x, y)| (x - y).abs())
        .sum()
}

/// Mid-crossfade the palette sits strictly between the outgoing and incoming poses; at and after
/// the fade's end it is byte-identical to the entity that only ever played the incoming clip.
#[test]
fn a_crossfade_blends_then_lands_exactly_on_the_incoming_clip() {
    let root = fixture_project("crossfade");
    let engine = engine_at(&root, FADE_SCENE);
    let (fading, outgoing, incoming) = (0, 1, 2);

    let mid = palettes_at(&engine, 30);
    let (to_out, to_in, ends) = (
        distance(&mid[fading], &mid[outgoing]),
        distance(&mid[fading], &mid[incoming]),
        distance(&mid[outgoing], &mid[incoming]),
    );
    assert!(to_out > 1e-3 && to_in > 1e-3, "the blend is neither end");
    assert!(
        to_out < ends && to_in < ends,
        "the blend lies between its ends: {to_out}, {to_in} < {ends}"
    );

    for tick in [60, 90] {
        let posed = palettes_at(&engine, tick);
        assert_eq!(posed[fading], posed[incoming], "tick {tick}");
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// `hold: 5` on a plain clip and on a mid-crossfade character: ticks 10..=14 pose identically, tick
/// 15 is the next pose.
#[test]
fn a_hold_steps_the_skin_pose_on_a_global_grid() {
    let root = fixture_project("hold-grid");
    let scene = FADE_SCENE.replace("Animation:\n", "Animation:\n        hold: 5\n");
    let engine = engine_at(&root, &scene);
    let held = palettes_at(&engine, 10);
    for tick in 11..=14 {
        assert_eq!(palettes_at(&engine, tick), held, "tick {tick}");
    }
    let next = palettes_at(&engine, 15);
    for (entity, name) in [(0, "fading"), (1, "plain jump"), (2, "plain run")] {
        assert!(
            !close(&held[entity], &next[entity]),
            "{name} must step at tick 15"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
