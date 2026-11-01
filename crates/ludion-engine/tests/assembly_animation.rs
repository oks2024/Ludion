//! Runtime proof for assembly clip playback (`Plan/voxel-animation.md` wave 2, decisions 2, 5-8):
//! an `Animation` component selects a clip, `extract()` poses the assembly's parts from it, and the
//! simulation never notices.
//!
//! Two properties are asserted, and they pull in opposite directions on purpose:
//!
//! - **The sim is untouched.** An animated run's per-frame `WorldHash` trace and its recorded `.lrp`
//!   are byte-identical whether or not anything extracts a frame — pose is a pure function of
//!   (clip, sim time), computed at the render boundary and written back nowhere. That is decision 2
//!   as an executable statement.
//! - **The evaluator actually runs.** The same scene extracted at different sim times produces
//!   *different* part matrices, and an entity whose clip name the assembly does not bind produces
//!   exactly the rest pose. Without these the first property would be satisfied by an evaluator that
//!   does nothing at all.
//!
//! Everything here goes through the ordinary public surface — a scene document, `Engine`,
//! `drive_hashes`, `extract_at` — over a fixture project written to a temp directory, because an
//! assembly is a multi-file asset and the resolution rules (parts and clips relative to the assembly
//! file) are part of what is under test.

use std::path::{Path, PathBuf};

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};
use ludion_protocol::{Command, command};
use ludion_renderer::MeshInstance;

/// A two-part assembly (`body` with a child `arm`) binding one clip under the short name `walk`.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: fixture
size:
  - 4
  - 2
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 2
      - 1
      - 0
clips:
  walk: anims/walk.anim.yaml
  raise: anims/raise.anim.yaml
";

/// The `walk` clip: `arm` sweeps 0° → 90° about +X across one looping second. Block-style
/// keyframes — the only spelling the strict-YAML subset accepts.
const CLIP: &str = "\
format: 1
kind: animation
name: walk
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
          - 90.0
          - 0.0
          - 0.0
";

/// The `raise` clip: `arm` holds 60° about +X, the incoming side of the crossfade fixture.
const RAISE_CLIP: &str = "format: 1
kind: animation
name: raise
duration: 1.0
loop: true
tracks:
  arm:
    rotation:
      - t: 0.0
        v:
          - 60.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 60.0
          - 0.0
          - 0.0
";

/// A one-second crossfade from `walk` into `raise` beginning at tick 0, beside the two plain
/// entities it blends between: `[fading body, fading arm, walk body, walk arm, raise body, raise arm]`.
const FADE_SCENE: &str = "format: 1
seed: 1
entities:
  - name: fading
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: raise
        from: walk
        fade: 1.0
  - name: walk
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: walk
  - name: raise
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: raise
";

/// Three entities over one assembly: one playing `walk`, one playing a clip the assembly does not
/// bind, and one with no `Animation` at all. Extract walks entities in ascending slot order and
/// pushes an assembly's parts as a contiguous run, so the mesh instances come out
/// `[walk body, walk arm, unknown body, unknown arm, rest body, rest arm]`.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: walker
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: walk
  - name: unknown-clip
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: sprint
  - name: still
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
";

/// How many parts one fixture assembly contributes to `RenderScene.meshes`.
const PARTS: usize = 2;

/// A fresh, empty project directory unique to this process + `name`, carrying the fixture assembly,
/// its two parts, and its clip.
fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-assembly-anim-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "models/fixture.assembly.yaml", ASSEMBLY);
    write(
        &dir,
        "models/parts/body.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"AA\"\n",
    );
    write(
        &dir,
        "models/parts/arm.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    write(&dir, "models/anims/walk.anim.yaml", CLIP);
    write(&dir, "models/anims/raise.anim.yaml", RAISE_CLIP);
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn engine_at(root: &Path) -> Engine {
    engine_with(root, SCENE)
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

/// The mesh instances of `engine`'s world posed at `tick` — the whole visual layer, through the
/// public extract entry point.
fn posed_at(engine: &Engine, tick: u64) -> Vec<MeshInstance> {
    extract_at(
        engine.world(),
        (320, 180),
        engine.project_root(),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes
}

/// The sim half of decision 2, both ways at once: the per-frame `WorldHash` trace and the recorded
/// `.lrp` bytes of an animated run are identical whether or not frames are extracted along the way.
///
/// The extracting run really does pose the model (`a_mid_clip_time_moves_the_child_part` proves the
/// matrices move), so this is not vacuous: it says the render boundary is a read, and animation adds
/// nothing to the record stream, nothing to the world, and nothing to a replay.
#[test]
fn extracting_an_animated_run_changes_neither_its_hashes_nor_its_recording() {
    assert_extraction_is_hash_neutral("hash-neutral", SCENE);
}

/// The same neutrality with a crossfade in progress for most of the run.
#[test]
fn extracting_a_crossfading_run_changes_neither_its_hashes_nor_its_recording() {
    assert_extraction_is_hash_neutral("hash-neutral-fade", FADE_SCENE);
}

fn assert_extraction_is_hash_neutral(name: &str, scene: &str) {
    let root = fixture_project(name);

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_with(&root, scene);
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

/// The evaluator half: the same world posed at three points of a one-second clip gives three
/// different transforms for the animated child part, while its parent (which the clip does not
/// animate) never moves. Frame 0 is the clip's first keyframe, so it *is* the rest pose.
#[test]
fn a_mid_clip_time_moves_the_child_part() {
    let root = fixture_project("mid-clip");
    let engine = engine_at(&root);

    let (start, quarter, half) = (
        posed_at(&engine, 0),
        posed_at(&engine, 15),
        posed_at(&engine, 30),
    );

    assert_ne!(start[1].model, quarter[1].model, "a quarter second in");
    assert_ne!(quarter[1].model, half[1].model, "and again by the half");
    assert_eq!(
        start[0].model, half[0].model,
        "the parent has no track, so it never moves"
    );

    // The clip is authored on `arm` alone, so the *rest* entity is unmoved at every time — the pose
    // is per entity, not per assembly.
    let rest = 2 * PARTS;
    assert_eq!(start[rest + 1].model, half[rest + 1].model);

    let _ = std::fs::remove_dir_all(&root);
}

/// A clip name the assembly does not bind is a silent degrade to the rest pose — byte-identical to
/// the entity that carries no `Animation` at all, and never a diagnostic (the extract above would
/// have returned `Err`). Clip names are chosen at runtime, so an unknown one cannot be an error.
#[test]
fn an_unknown_clip_name_poses_at_rest() {
    let root = fixture_project("unknown-clip");
    let engine = engine_at(&root);

    let posed = posed_at(&engine, 45);
    let (unknown, rest) = (PARTS, 2 * PARTS);
    for part in 0..PARTS {
        assert_eq!(
            posed[unknown + part].model,
            posed[rest + part].model,
            "part {part} of the unknown-clip entity must rest exactly where the un-animated one does"
        );
        assert_eq!(
            posed[unknown + part].normal_matrix,
            posed[rest + part].normal_matrix
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// Editing the clip file and reloading it re-poses the model without touching the sim: the world
/// hash before and after an `asset.reload` is the same value, and the pose at a mid-clip time has
/// changed. This is the authoring loop the arc is for — nudge a keyframe, see it, with the run still
/// going.
#[test]
fn reloading_an_edited_clip_re_poses_a_live_run() {
    let root = fixture_project("clip-reload");
    let mut engine = engine_at(&root);
    ludion_engine::drive_hashes(&mut engine, vec![step(30)]).expect("the session drives");

    let before = posed_at(&engine, 30);
    let hash_before = engine.world_hash().expect("the world hashes");

    write(
        &root,
        "models/anims/walk.anim.yaml",
        &CLIP.replace("90.0", "10.0"),
    );
    engine
        .execute(Command::AssetReload(command::AssetReloadParams {
            path: Some("models/anims/walk.anim.yaml".to_string()),
        }))
        .expect("the clip reloads");

    // `extract_at` decodes through a fresh cache of its own, not the engine's, so it sees the edit
    // either way; the reload command is what the *live* stream needs, and this asserts it is
    // accepted and sim-invisible.
    let after = posed_at(&engine, 30);
    assert_ne!(
        before[1].model, after[1].model,
        "a re-read clip must change the pose it produces"
    );
    assert_eq!(
        hash_before,
        engine.world_hash().expect("the world hashes"),
        "an asset reload is invisible to the simulation"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The sine of the arm's turn about +X, read off its model matrix (the parent has no track, so the
/// arm's rotation is the whole upper 3x3).
fn arm_sine(instance: &MeshInstance) -> f32 {
    instance.model.y_axis.z
}

/// Mid-crossfade the arm sits strictly between the outgoing `walk` pose and the incoming `raise`
/// pose; at and after the fade's end the entity is byte-identical to one that only ever played
/// `raise`, so the blend leaves no residue.
#[test]
fn a_crossfade_blends_then_lands_exactly_on_the_incoming_clip() {
    let root = fixture_project("crossfade");
    let engine = engine_with(&root, FADE_SCENE);
    let (fading, outgoing, incoming) = (0, PARTS, 2 * PARTS);

    let mid = posed_at(&engine, 30);
    let (blend, from, to) = (
        arm_sine(&mid[fading + 1]),
        arm_sine(&mid[outgoing + 1]),
        arm_sine(&mid[incoming + 1]),
    );
    assert!(
        from < blend && blend < to,
        "mid-fade {blend} must lie strictly between {from} and {to}"
    );
    assert_eq!(
        mid[fading].model, mid[incoming].model,
        "the parent has no track on either side"
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

/// A mid-crossfade entity and a plain clip entity, both `hold: 5`, and a third on `hold: 3`:
/// `[fading body, fading arm, walk body, walk arm, raise body, raise arm]`.
const HOLD_SCENE: &str = "format: 1
seed: 1
entities:
  - name: fading
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: raise
        from: walk
        fade: 1.0
        hold: 5
  - name: walk
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: walk
        hold: 5
  - name: raise
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: raise
        hold: 3
";

fn models(posed: &[MeshInstance]) -> Vec<ludion_core::math::glam::Mat4> {
    posed.iter().map(|instance| instance.model).collect()
}

/// `hold: 5` poses on a global grid: ticks 10..=14 are one pose, tick 15 the next — for a plain
/// clip and for a crossfade (whose weight steps with the clocks).
#[test]
fn a_hold_steps_the_pose_on_a_global_grid() {
    let root = fixture_project("hold-grid");
    let engine = engine_with(&root, HOLD_SCENE);
    // Only the two hold-5 entities; the hold-3 one steps on its own grid.
    let fives = |tick| models(&posed_at(&engine, tick))[..2 * PARTS].to_vec();
    let held = fives(10);
    for tick in 11..=14 {
        assert_eq!(fives(tick), held, "tick {tick}");
    }
    let next = fives(15);
    for (entity, name) in [(0, "fading"), (PARTS, "walk")] {
        assert_ne!(
            held[entity + 1],
            next[entity + 1],
            "{name} must step at tick 15"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `hold: 1` and an absent field are the same world: identical poses and an identical hash.
#[test]
fn hold_one_is_the_absent_field() {
    let root = fixture_project("hold-one");
    let with = HOLD_SCENE.replace("hold: 5", "hold: 1");
    let without = HOLD_SCENE.replace("        hold: 5\n", "");
    let (mut with, mut without) = (engine_with(&root, &with), engine_with(&root, &without));
    for tick in [0, 7, 13, 30] {
        assert_eq!(
            models(&posed_at(&with, tick)),
            models(&posed_at(&without, tick))
        );
    }
    assert_eq!(with.world_hash().unwrap(), without.world_hash().unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extracting_a_held_run_changes_neither_its_hashes_nor_its_recording() {
    assert_extraction_is_hash_neutral("hash-neutral-hold", HOLD_SCENE);
}

/// On the wire, `hold: 0` is `L1317` and inserts nothing; `hold: 5` sets cleanly.
#[test]
fn component_set_rejects_a_zero_hold() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let entity = engine.world_mut().spawn();
    let set = |engine: &mut Engine, value| {
        ludion_engine::dispatch(
            engine,
            Command::ComponentSet(command::ComponentSetParams {
                entity,
                component: "Animation".to_string(),
                value,
            }),
        )
    };
    let error = set(&mut engine, serde_json::json!({ "hold": 0 }))
        .expect_err("a zero hold must be rejected");
    assert_eq!(error.code.as_str(), "L1317");
    assert!(
        !ludion_engine::builtin_registry()
            .get("Animation")
            .unwrap()
            .has(engine.world(), entity),
        "a rejected component.set must not insert the component"
    );
    set(&mut engine, serde_json::json!({ "hold": 5 })).expect("a positive hold sets cleanly");
}
