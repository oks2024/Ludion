//! Runtime proof for `AnimationLayers` on voxel assemblies (`Plan/animation-layers.md` decisions
//! 26-29, Neon Shroud request 0007 A1): part masks, override and additive layers composing over the
//! base clip in extract's assembly path, and layer events.
//!
//! - **Neutral.** No layers, or a layer with no weight, poses bit for bit like the plain assembly;
//!   extraction moves no hash.
//! - **Override.** A mask restricts it to its parts; a channel the layer clip does not key keeps the
//!   base; `descendants` inherits through a parent authored *after* its child.
//! - **Additive.** Adds the change from rest, or from a `reference` clip's first frame.
//! - **Events.** A layer with `events: true` fires once per cycle, tagged with its name.
//! - **Deterministic.** A snapshot restored mid-fade reproduces the matrices; 1, 2 and 8 threads agree;
//!   a layer composes over a crossfading base.
//!
//! The fixture is a humanoid-ish assembly in a scratch directory, parts authored `[leg, torso, head,
//! arm, foot]`: the leg comes before its parent, so ancestors are never in document order.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Animation, AnimationEvents, BlendClock, ClipEvent, Engine, Mesh, QualityParams,
    animation_events, extract_at,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};

const LEG: usize = 0;
const TORSO: usize = 1;
const HEAD: usize = 2;
const ARM: usize = 3;
const FOOT: usize = 4;
const PARTS: usize = 5;

const ASSEMBLY: &str = "format: 1
kind: assembly
name: humanoid
size:
  - 3
  - 3
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: leg
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 0
      - 0
      - 0
  - name: torso
    file: parts/cell.part.yaml
    place:
      - 1
      - 1
      - 0
  - name: head
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 2
      - 0
  - name: arm
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 2
      - 1
      - 0
  - name: foot
    file: parts/cell.part.yaml
    parent: leg
    place:
      - 0
      - 0
      - 0
clips:
  walk: anims/walk.anim.yaml
  walkwave: anims/walkwave.anim.yaml
  wave: anims/wave.anim.yaml
  flail: anims/flail.anim.yaml
  sway: anims/sway.anim.yaml
  swayref: anims/swayref.anim.yaml
  lean: anims/lean.anim.yaml
  raise: anims/raise.anim.yaml
masks:
  arm_only:
    joints:
      - arm
  no_leg:
    joints:
      - torso
      - leg
    descendants: true
    weights:
      - 1.0
      - 0.0
";

/// A looping one-second clip over `tracks` (already indented 2 spaces), `events` appended.
fn clip(name: &str, tracks: &str, events: &str) -> String {
    format!(
        "format: 1\nkind: animation\nname: {name}\nduration: 1.0\nloop: true\n{events}tracks:\n{tracks}"
    )
}

/// One track: `target` with each `(channel, from, to)` keyed at `t = 0` and `t = 1` (xyz in degrees
/// or voxel units).
fn channel(target: &str, name: &str, from: [f64; 3], to: [f64; 3]) -> String {
    track(target, &[(name, from, to)])
}

fn track(target: &str, channels: &[(&str, [f64; 3], [f64; 3])]) -> String {
    let keys = |t: f64, v: [f64; 3]| {
        format!(
            "      - t: {t:?}\n        v:\n          - {:?}\n          - {:?}\n          - {:?}\n",
            v[0], v[1], v[2]
        )
    };
    let mut out = format!("  {target}:\n");
    for (name, from, to) in channels {
        let (first, last) = (keys(0.0, *from), keys(1.0, *to));
        out.push_str(&format!("    {name}:\n{first}{last}"));
    }
    out
}

const HIT: &str = "events:\n  - t: 0.5\n    name: hit\n";

fn clips() -> Vec<(&'static str, String)> {
    let walk_tracks = format!(
        "{}{}",
        channel("leg", "rotation", [0.0; 3], [30.0, 0.0, 0.0]),
        track(
            "arm",
            &[
                ("rotation", [0.0; 3], [45.0, 0.0, 0.0]),
                ("translation", [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
            ]
        ),
    );
    let walkwave_tracks = format!(
        "{}{}",
        channel("leg", "rotation", [0.0; 3], [30.0, 0.0, 0.0]),
        track(
            "arm",
            &[
                ("rotation", [0.0; 3], [0.0, 0.0, 90.0]),
                ("translation", [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
            ]
        ),
    );
    let wave_tracks = format!(
        "{}{}",
        channel("arm", "rotation", [0.0; 3], [0.0, 0.0, 90.0]),
        channel("head", "rotation", [0.0; 3], [0.0, 45.0, 0.0]),
    );
    let flail_tracks = format!(
        "{}{}{}",
        channel("leg", "rotation", [0.0; 3], [0.0, 0.0, 60.0]),
        channel("foot", "rotation", [0.0; 3], [0.0, 0.0, 60.0]),
        channel("arm", "rotation", [0.0; 3], [0.0, 0.0, 60.0]),
    );
    vec![
        ("walk", clip("walk", &walk_tracks, "")),
        ("walkwave", clip("walkwave", &walkwave_tracks, "")),
        ("wave", clip("wave", &wave_tracks, HIT)),
        ("flail", clip("flail", &flail_tracks, "")),
        (
            "sway",
            clip(
                "sway",
                &channel("torso", "rotation", [0.0, 0.0, 10.0], [0.0, 0.0, 30.0]),
                "",
            ),
        ),
        (
            "swayref",
            clip(
                "swayref",
                &channel("torso", "rotation", [0.0, 0.0, -10.0], [0.0, 0.0, 10.0]),
                "",
            ),
        ),
        (
            "lean",
            clip(
                "lean",
                &channel("torso", "rotation", [0.0, 0.0, 20.0], [0.0, 0.0, 20.0]),
                "",
            ),
        ),
        (
            "raise",
            clip(
                "raise",
                &channel("arm", "rotation", [60.0, 0.0, 0.0], [60.0, 0.0, 0.0]),
                "",
            ),
        ),
    ]
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-assembly-layers-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "models/humanoid.assembly.yaml", ASSEMBLY);
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    for (stem, text) in clips() {
        write(&dir, &format!("models/anims/{stem}.anim.yaml"), &text);
    }
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// One `humanoid` entity: `base` fields of `Animation` (already indented 8), then `layers` (a block of
/// list items indented 10) as its `AnimationLayers`, or none.
fn scene(base: &str, layers: &str) -> String {
    let layered = if layers.is_empty() {
        String::new()
    } else {
        format!("      AnimationLayers:\n        layers:\n{layers}")
    };
    format!(
        "format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Mesh:
        shape:
          assembly: models/humanoid.assembly.yaml
      Animation:
{base}{layered}"
    )
}

/// A layer `name` on `clip` with `fields` (indented 12) appended.
fn layer(name: &str, clip: &str, fields: &str) -> String {
    format!("          - name: {name}\n            clip: {clip}\n{fields}")
}

const WEIGHT_1: &str = "            weight: 1.0\n";
const WALK: &str = "        clip: walk\n";
const NO_BASE: &str = "        clip: \"\"\n";

fn engine_at(root: &Path, text: &str) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
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

fn close(a: &Mat4, b: &Mat4) -> bool {
    a.abs_diff_eq(*b, 1e-5)
}

#[test]
fn a_weightless_layer_poses_bit_for_bit_like_no_component() {
    let root = fixture_project("zero");
    let plain = engine_at(&root, &scene(WALK, ""));
    let zero = engine_at(&root, &scene(WALK, &layer("w", "wave", "")));
    let faded = engine_at(
        &root,
        &scene(WALK, &layer("w", "wave", "            fade: 1.0\n")),
    );
    let unbound = engine_at(&root, &scene(WALK, &layer("w", "nope", WEIGHT_1)));
    for tick in [0, 7, 31] {
        let want = posed_at(&plain, tick);
        assert_eq!(want, posed_at(&zero, tick), "weight 0, tick {tick}");
        assert_eq!(want, posed_at(&unbound, tick), "unbound clip, tick {tick}");
    }
    // A ramp from weight 0 to 0 is no layer either.
    assert_eq!(posed_at(&plain, 9), posed_at(&faded, 9));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_masked_override_moves_only_the_masked_part_and_keeps_unkeyed_channels() {
    let root = fixture_project("override");
    let plain = engine_at(&root, &scene(WALK, ""));
    let masked = engine_at(
        &root,
        &scene(
            WALK,
            &layer(
                "w",
                "wave",
                &format!("{WEIGHT_1}            mask: arm_only\n"),
            ),
        ),
    );
    let unmasked = engine_at(&root, &scene(WALK, &layer("w", "wave", WEIGHT_1)));
    // The same pose authored as one base clip: the `wave` clip's arm rotation over the walk's arm
    // translation (the layer does not key the arm's translation, so the base's stays).
    let authored = engine_at(&root, &scene("        clip: walkwave\n", ""));
    for tick in [12, 30] {
        let (base, over) = (posed_at(&plain, tick), posed_at(&masked, tick));
        for part in [LEG, TORSO, HEAD, FOOT] {
            assert_eq!(base[part], over[part], "part {part} at tick {tick}");
        }
        assert!(!close(&base[ARM], &over[ARM]), "the arm moved");
        assert!(
            close(&over[ARM], &posed_at(&authored, tick)[ARM]),
            "wave rotation over the walk's translation at tick {tick}"
        );
        // Without the mask the `wave` clip's head track applies too.
        let all = posed_at(&unmasked, tick);
        assert_eq!(over[ARM], all[ARM]);
        assert!(!close(&base[HEAD], &all[HEAD]), "the head moved unmasked");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_partial_weight_lands_between_the_base_and_the_layer() {
    let root = fixture_project("half");
    let base = engine_at(&root, &scene(WALK, ""));
    let half = engine_at(
        &root,
        &scene(WALK, &layer("w", "wave", "            weight: 0.5\n")),
    );
    let full = engine_at(&root, &scene(WALK, &layer("w", "wave", WEIGHT_1)));
    let (b, h, f) = (
        posed_at(&base, 20),
        posed_at(&half, 20),
        posed_at(&full, 20),
    );
    assert!(!close(&b[ARM], &h[ARM]) && !close(&h[ARM], &f[ARM]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn descendants_inherit_through_a_parent_authored_after_the_child() {
    let root = fixture_project("descendants");
    let plain = engine_at(&root, &scene(WALK, ""));
    let layered = engine_at(
        &root,
        &scene(
            WALK,
            &layer(
                "f",
                "flail",
                &format!("{WEIGHT_1}            mask: no_leg\n"),
            ),
        ),
    );
    let (base, over) = (posed_at(&plain, 18), posed_at(&layered, 18));
    // `leg` is listed at weight 0 and `foot` inherits it: both keep the walk's pose even though the
    // layer clip keys them, and `leg` is authored before its parent `torso`.
    assert_eq!(base[LEG], over[LEG]);
    assert_eq!(base[FOOT], over[FOOT]);
    assert!(
        !close(&base[ARM], &over[ARM]),
        "the arm inherits the torso's 1.0"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_additive_layer_adds_its_change_from_rest_or_from_the_reference() {
    let root = fixture_project("additive");
    // No base: the additive sway on the rest pose is the sway itself.
    let sway_only = engine_at(&root, &scene("        clip: sway\n", ""));
    let additive = engine_at(
        &root,
        &scene(
            NO_BASE,
            &layer(
                "s",
                "sway",
                &format!("            mode: additive\n{WEIGHT_1}"),
            ),
        ),
    );
    // Measured from `lean`'s first frame (20 degrees), the sway lands 20 degrees lower.
    let referenced = engine_at(
        &root,
        &scene(
            NO_BASE,
            &layer(
                "s",
                "sway",
                &format!("            mode: additive\n            reference: lean\n{WEIGHT_1}"),
            ),
        ),
    );
    let lowered = engine_at(&root, &scene("        clip: swayref\n", ""));
    for tick in [0, 15, 45] {
        assert!(
            close(
                &posed_at(&sway_only, tick)[TORSO],
                &posed_at(&additive, tick)[TORSO]
            ),
            "from rest at tick {tick}"
        );
        assert!(
            close(
                &posed_at(&lowered, tick)[TORSO],
                &posed_at(&referenced, tick)[TORSO]
            ),
            "from the reference at tick {tick}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_layer_composes_over_a_crossfading_base() {
    let root = fixture_project("crossfade");
    let fading = "        clip: raise\n        from: walk\n        fade: 1.0\n";
    let plain = engine_at(&root, &scene(fading, ""));
    let layered = engine_at(
        &root,
        &scene(
            fading,
            &layer(
                "w",
                "wave",
                &format!("{WEIGHT_1}            mask: arm_only\n"),
            ),
        ),
    );
    for tick in [10, 30] {
        let (base, over) = (posed_at(&plain, tick), posed_at(&layered, tick));
        for part in [LEG, TORSO, HEAD, FOOT] {
            assert_eq!(base[part], over[part], "part {part} at tick {tick}");
        }
        assert!(!close(&base[ARM], &over[ARM]));
    }
    // After the fade the layer composes over the plain incoming clip.
    let settled = engine_at(
        &root,
        &scene(
            "        clip: raise\n",
            &layer(
                "w",
                "wave",
                &format!("{WEIGHT_1}            mask: arm_only\n"),
            ),
        ),
    );
    assert!(close(
        &posed_at(&layered, 90)[ARM],
        &posed_at(&settled, 90)[ARM]
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_snapshot_restored_mid_fade_reproduces_the_matrices() {
    let root = fixture_project("restore");
    let text = scene(
        WALK,
        &layer(
            "w",
            "wave",
            "            weight: 1.0\n            fade: 1.0\n",
        ),
    );
    let mut engine = engine_at(&root, &text);
    step(&mut engine, 20);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    step(&mut engine, 25);
    let want = posed_at(&engine, engine.frame().as_u64());
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    step(&mut engine, 25);
    assert_eq!(posed_at(&engine, engine.frame().as_u64()), want);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn extracting_a_layered_assembly_changes_neither_its_hashes_nor_its_recording() {
    let root = fixture_project("hash-neutral");
    let text = scene(
        WALK,
        &layer(
            "w",
            "wave",
            "            weight: 0.7\n            fade: 0.5\n",
        ),
    );
    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = root.join(format!("{tag}.lrp"));
        let mut engine = engine_at(&root, &text);
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");
        let mut trace = Vec::new();
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(
                    &mut engine,
                    vec![Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    })],
                )
                .expect("the session drives")
                .into_iter()
                .skip(1),
            );
            if extract_each_tick {
                assert_eq!(posed_at(&engine, engine.frame().as_u64()).len(), PARTS);
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap())
    };
    let (quiet_trace, quiet_bytes) = run("quiet", false);
    let (drawn_trace, drawn_bytes) = run("drawn", true);
    assert_eq!(quiet_trace.len(), 40);
    assert_eq!(quiet_trace, drawn_trace, "extraction moves no world hash");
    assert_eq!(quiet_bytes, drawn_bytes, "extraction writes nothing");
    let _ = std::fs::remove_dir_all(&root);
}

/// 240 humanoids x 5 parts: over the parallel threshold, so 2 and 8 threads really run wide.
#[test]
fn layered_matrices_are_byte_identical_at_1_2_8_threads() {
    let root = fixture_project("threads");
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    for i in 0..240 {
        text.push_str(&format!(
            "  - name: h{i}\n    components:\n      Mesh:\n        shape:\n          assembly: models/humanoid.assembly.yaml\n      Animation:\n        clip: walk\n"
        ));
        match i % 3 {
            0 => text.push_str(&format!(
                "      AnimationLayers:\n        layers:\n{}",
                layer("w", "wave", &format!("{WEIGHT_1}            mask: arm_only\n"))
            )),
            1 => text.push_str(&format!(
                "      AnimationLayers:\n        layers:\n{}",
                layer(
                    "s",
                    "sway",
                    "            mode: additive\n            weight: 0.6\n            reference: lean\n"
                )
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
                posed_at(&engine, 10)
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

// --- Events -------------------------------------------------------------------------------------

#[derive(Default)]
struct EventLog(Vec<ClipEvent>);

fn record(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let fired: Vec<ClipEvent> = world
        .resource::<AnimationEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<EventLog>().is_none() {
        world.insert_resource(EventLog::default());
    }
    world
        .resource_mut::<EventLog>()
        .expect("just inserted")
        .0
        .extend(fired);
}

fn event_engine(root: &Path, text: &str) -> Engine {
    let mut engine = Engine::builder()
        .project_root(root)
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&BlendClock,)>()
        .system("record", record)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn fired(engine: &Engine) -> Vec<ClipEvent> {
    engine
        .world()
        .resource::<EventLog>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

#[test]
fn an_events_layer_fires_once_per_cycle_tagged_with_its_name() {
    let root = fixture_project("events");
    // The base `walk` has no events; the baseless variant has no base at all.
    for (tag, base) in [("based", WALK), ("baseless", NO_BASE)] {
        let text = scene(
            base,
            &layer(
                "w",
                "wave",
                &format!("{WEIGHT_1}            events: true\n"),
            ),
        );
        let mut engine = event_engine(&root, &text);
        step(&mut engine, 120);
        let events = fired(&engine);
        assert_eq!(events.len(), 2, "{tag}: two one-second cycles in 120 ticks");
        assert!(
            events
                .iter()
                .all(|e| e.name == "hit" && e.layer == "w" && e.clip == "wave"),
            "{tag}: {events:?}"
        );
    }
    let quiet = scene(WALK, &layer("w", "wave", WEIGHT_1));
    let mut engine = event_engine(&root, &quiet);
    step(&mut engine, 120);
    assert!(fired(&engine).is_empty(), "events: false fires nothing");
    let _ = std::fs::remove_dir_all(&root);
}
