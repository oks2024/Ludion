//! Runtime proof for animation-layer blends, references and events (`Plan/animation-layers.md`
//! W2b, Neon Shroud request 0007 §2 and §3).
//!
//! - **Events.** A layer with `events: true` fires its clip's events tagged with its name, after the
//!   base events; `events: false` and weight `0` fire nothing; a scaled game clock fires each once.
//! - **Blend layers.** A layer naming a blend is driven by `AnimationLayer.params`, keeps a
//!   `BlendClock.layers` entry that restarts on a new clip, and fires one foot per half stride.
//! - **Pose.** Params on one point pose as a plain-clip layer; an additive `reference` changes the
//!   pose and stays thread-count invariant.
//! - **Neutral.** A layer scene without blends never gets a `BlendClock`, and extraction moves no hash.
//! - **Deterministic.** A snapshot restored mid-sweep reproduces the phase, palette and event log.
//!
//! Runs over a scratch copy of the gallery mannequin in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::time::{GameClock, SimTime, TickRate};
use ludion_engine::{
    Animation, AnimationEvents, AnimationLayers, BlendClock, ClipEvent, Engine, Mesh,
    QualityParams, animation_events, blend_clocks, extract_at, play_layer, set_layer_params,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};

const JOINTS: usize = 11;

/// The mannequin with `walk`, `run` and `idle` carrying `foot_l`/`foot_r` overlay events, and the
/// 1D `locomotion` blend and a 2D `cross` blend over them.
const CHARACTER: &str = "\
format: 1
kind: assembly
name: blended
mesh: \"../meshes/skin-mannequin.mesh.yaml\"
clips:
  idle:
    clip: \"../clips/idle.anim.yaml\"
    events:
      - t: 0.5
        name: foot_l
      - t: 1.5
        name: foot_r
  run:
    clip: \"../clips/run.anim.yaml\"
    speed: 1.2
    events:
      - t: 0.2
        name: foot_l
      - t: 0.6
        name: foot_r
  walk:
    clip: \"../clips/run.anim.yaml\"
    speed: 0.6
    events:
      - t: 0.2
        name: foot_l
      - t: 0.6
        name: foot_r
blends:
  cross:
    sync: foot_l
    points:
      - clip: idle
        at:
          - 0.0
          - 0.0
      - clip: walk
        at:
          - 2.0
          - 0.0
      - clip: run
        at:
          - 0.0
          - 2.0
  locomotion:
    sync: foot_l
    points:
      - clip: idle
        at: 0.0
      - clip: walk
        at: 2.0
      - clip: run
        at: 5.0
";

/// A `runner` on the blended character: `base` fields of `Animation`, then the `layers` list.
fn scene(base: &str, layers: &str) -> String {
    format!(
        "format: 1
seed: 1
entities:
  - name: runner
    components:
      Mesh:
        shape:
          character: characters/blended.character.yaml
      Animation:
{base}      AnimationLayers:
        layers:
{layers}"
    )
}

/// One full-weight layer `name` on `clip`, with `fields` (already indented 12 spaces) appended.
fn layer(name: &str, clip: &str, fields: &str) -> String {
    format!("          - name: {name}\n            clip: {clip}\n            weight: 1.0\n{fields}")
}

const RUN: &str = "        clip: run\n";
const IDLE: &str = "        clip: idle\n";
/// An empty base clip: the rest pose, so a layer's pose is all there is.
const NO_BASE: &str = "        clip: \"\"\n";
const EVENTS: &str = "            events: true\n";

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn fixture_project(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-layer-events-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["characters", "meshes", "clips"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for file in [
        "meshes/skin-mannequin.mesh.yaml",
        "clips/idle.anim.yaml",
        "clips/run.anim.yaml",
    ] {
        std::fs::copy(gallery().join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("characters/blended.character.yaml"), CHARACTER).unwrap();
    dir
}

/// The events every tick fired, `(tick, event)`, in firing order.
#[derive(Default)]
struct EventLog(Vec<(u64, ClipEvent)>);

fn record(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let fired: Vec<ClipEvent> = world
        .resource::<AnimationEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<EventLog>().is_none() {
        world.insert_resource(EventLog::default());
    }
    let log = world.resource_mut::<EventLog>().expect("just inserted");
    log.0
        .extend(fired.into_iter().map(|event| (ctx.frame.as_u64(), event)));
}

type Driver = fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx);

/// The game: an optional driver before `blend_clocks`, then the two systems and the event log.
fn game(root: &Path, driver: Option<Driver>) -> ludion_engine::EngineBuilder {
    let mut builder = Engine::builder().project_root(root);
    if let Some(driver) = driver {
        builder = builder
            .system("driver", driver)
            .query::<(&mut AnimationLayers,)>();
    }
    builder
        .system("blend_clocks", blend_clocks)
        .query::<(&Animation, &Mesh)>()
        .query::<(&mut BlendClock,)>()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&BlendClock,)>()
        .system("record", record)
}

/// Writes the first layer's `params = [tick / 60, 0]` (0 to 5 over 300 ticks).
fn sweep(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let x = (ctx.game_frame.as_u64() as f32 / 300.0 * 5.0).min(5.0);
    for (_, (layers,)) in world.query_ids::<(&mut AnimationLayers,)>() {
        let name = layers.layers[0].name.clone();
        set_layer_params(layers, &name, [x, 0.0]);
    }
}

/// Sweeps like [`sweep`], and at frame 100 switches the layer to the `cross` blend.
fn sweep_then_switch(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    sweep(world, ctx);
    if ctx.frame.as_u64() == 100 {
        for (_, (layers,)) in world.query_ids::<(&mut AnimationLayers,)>() {
            let name = layers.layers[0].name.clone();
            assert!(play_layer(layers, &name, "cross", ctx.time));
        }
    }
}

/// Slows the game to half speed on every frame.
fn half_speed(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    world.resource_mut::<GameClock>().unwrap().scale(0.5);
}

fn engine_for(name: &str, text: &str, driver: Option<Driver>) -> (Engine, PathBuf) {
    let root = fixture_project(name);
    let mut engine = game(&root, driver).build().expect("the game builds");
    engine.load_inline_scene(text).expect("the scene loads");
    (engine, root)
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("steps");
}

fn log(engine: &Engine) -> Vec<(u64, ClipEvent)> {
    engine
        .world()
        .resource::<EventLog>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

/// The names the layer `layer` fired, in order.
fn layer_names<'a>(events: &'a [(u64, ClipEvent)], layer: &str) -> Vec<&'a str> {
    events
        .iter()
        .filter(|(_, e)| e.layer == layer)
        .map(|(_, e)| e.name.as_str())
        .collect()
}

fn clock(engine: &Engine) -> Option<BlendClock> {
    let world = engine.world();
    world
        .entities()
        .find_map(|id| world.component::<BlendClock>(id).cloned())
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

#[test]
fn an_events_layer_fires_each_foot_once_per_cycle_after_the_base_events() {
    let text = scene(RUN, &layer("step", "run", EVENTS));
    let (mut engine, root) = engine_for("plain", &text, None);
    step(&mut engine, 120);
    let events = log(&engine);
    let names = layer_names(&events, "step");
    // `run` plays at 1.2x: a 0.667 s cycle, three in two seconds.
    assert_eq!(
        names.iter().filter(|n| **n == "foot_l").count(),
        3,
        "{names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "foot_r").count(),
        3,
        "{names:?}"
    );
    assert!(names.chunks(2).all(|pair| pair == ["foot_l", "foot_r"]));
    assert!(
        events
            .iter()
            .filter(|(_, e)| e.layer == "step")
            .all(|(_, e)| e.clip == "run")
    );
    // The base fires the same names with an empty `layer`; on a shared tick it comes first.
    assert_eq!(layer_names(&events, "").len(), names.len());
    let mut shared = 0;
    for tick in events.iter().map(|(tick, _)| *tick) {
        let order: Vec<&str> = events
            .iter()
            .filter(|(t, _)| *t == tick)
            .map(|(_, e)| e.layer.as_str())
            .collect();
        if order.len() > 1 {
            shared += 1;
            assert_eq!(order, ["", "step"], "tick {tick}");
        }
    }
    assert!(shared > 0, "the base and the layer share a tick");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_layer_without_events_and_a_weightless_one_fire_nothing() {
    let quiet = layer("quiet", "run", "");
    let weightless = "          - name: zero\n            clip: run\n            events: true\n";
    let text = scene(RUN, &format!("{quiet}{weightless}"));
    let (mut engine, root) = engine_for("quiet", &text, None);
    step(&mut engine, 120);
    let events = log(&engine);
    assert!(!layer_names(&events, "").is_empty(), "the base still fires");
    for name in ["quiet", "zero"] {
        assert!(layer_names(&events, name).is_empty(), "{name}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_layer_blend_swept_over_params_alternates_feet_and_keeps_a_phase_entry() {
    let text = scene(IDLE, &layer("loco", "locomotion", EVENTS));
    let (mut engine, root) = engine_for("sweep", &text, Some(sweep));
    step(&mut engine, 300);
    let events = log(&engine);
    let clock = clock(&engine).expect("a layer blend gives the runner a clock");
    assert!(clock.blend.is_empty(), "the base is no blend: {clock:?}");
    let [entry] = clock.layers.as_slice() else {
        panic!("one layer entry: {clock:?}");
    };
    assert_eq!(
        (entry.name.as_str(), entry.blend.as_str()),
        ("loco", "locomotion")
    );
    assert!(
        entry.phase > 3.0,
        "the sweep covers several strides: {}",
        entry.phase
    );
    let names = layer_names(&events, "loco");
    assert!(
        names.chunks(2).all(|pair| pair[0] == "foot_l") && names.windows(2).all(|w| w[0] != w[1]),
        "feet alternate: {names:?}"
    );
    let left = names.iter().filter(|n| **n == "foot_l").count();
    assert_eq!(left, entry.phase.floor() as usize + 1);
    assert!(
        events
            .iter()
            .filter(|(_, e)| e.layer == "loco")
            .all(|(_, e)| e.clip == "locomotion")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn play_layer_restarts_the_layer_blend_entry() {
    let text = scene(IDLE, &layer("loco", "locomotion", EVENTS));
    let (mut engine, root) = engine_for("restart", &text, Some(sweep_then_switch));
    step(&mut engine, 100);
    let before = clock(&engine).expect("a clock").layers[0].clone();
    assert_eq!(
        (before.blend.as_str(), before.started.as_u64()),
        ("locomotion", 0)
    );
    step(&mut engine, 1);
    let after = clock(&engine).expect("a clock").layers[0].clone();
    assert_eq!(
        (after.blend.as_str(), after.started.as_u64()),
        ("cross", 100)
    );
    assert_eq!(after.prev, -1.0, "a restart opens the window at phase 0");
    step(&mut engine, 20);
    let later = clock(&engine).expect("a clock").layers[0].clone();
    assert!(later.phase > after.phase && later.prev >= 0.0, "{later:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scaled_clock_fires_the_same_layer_events_once_each() {
    let text = scene(RUN, &layer("step", "run", EVENTS));
    let (mut full, root_a) = engine_for("full", &text, None);
    step(&mut full, 120);
    let (mut slow, root_b) = engine_for("slow", &text, Some(half_speed));
    step(&mut slow, 240);
    let want = layer_names(&log(&full), "step").join(",");
    assert_eq!(want, "foot_l,foot_r,foot_l,foot_r,foot_l,foot_r");
    assert_eq!(layer_names(&log(&slow), "step").join(","), want);
    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_snapshot_restored_mid_sweep_reproduces_phase_palette_and_events() {
    let text = scene(IDLE, &layer("loco", "locomotion", EVENTS));
    let (mut engine, root) = engine_for("restore", &text, Some(sweep));
    step(&mut engine, 100);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    step(&mut engine, 120);
    let since = |engine: &Engine| -> Vec<(u64, ClipEvent)> {
        log(engine)
            .into_iter()
            .filter(|(tick, _)| *tick >= 100)
            .collect()
    };
    let (want_clock, want_events) = (clock(&engine), since(&engine));
    let want_palette = palettes_at(&engine, engine.frame().as_u64());
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    engine.world_mut().remove_resource::<EventLog>();
    step(&mut engine, 120);
    assert_eq!(clock(&engine), want_clock);
    assert_eq!(palettes_at(&engine, engine.frame().as_u64()), want_palette);
    assert_eq!(since(&engine), want_events);
    assert!(layer_names(&want_events, "loco").len() > 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn params_on_one_point_pose_as_a_plain_clip_layer() {
    let blended = layer(
        "l",
        "locomotion",
        "            params:\n              - 5.0\n              - 0.0\n",
    );
    let (a, root_a) = engine_for("pose-blend", &scene(NO_BASE, &blended), None);
    let (b, root_b) = engine_for("pose-plain", &scene(NO_BASE, &layer("l", "run", "")), None);
    // Phase 0 maps the sync event (0.2 s of `run`) to the start; the plain `run` binding
    // (speed 1.2) reaches 0.2 s after ten ticks.
    for (blend_tick, plain_tick) in [(0, 10), (6, 16)] {
        let (p, q) = (
            &palettes_at(&a, blend_tick)[0],
            &palettes_at(&b, plain_tick)[0],
        );
        for (m, n) in p.iter().zip(q) {
            assert!(m.abs_diff_eq(*n, 1e-6), "tick {blend_tick}: {m:?} vs {n:?}");
        }
    }
    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
}

fn additive_scene(entities: usize, reference: &str) -> String {
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    for i in 0..entities {
        text.push_str(&format!(
            "  - name: m{i}\n    components:\n      Mesh:\n        shape:\n          character: characters/blended.character.yaml\n      Animation:\n        clip: idle\n      AnimationLayers:\n        layers:\n          - name: add\n            clip: run\n            mode: additive\n            weight: 0.{}\n{reference}",
            1 + i % 9
        ));
    }
    text
}

#[test]
fn an_additive_reference_changes_the_pose_identically_at_1_2_8_threads() {
    let referenced = additive_scene(1, "            reference: idle\n");
    let (a, root_a) = engine_for("ref-with", &referenced, None);
    let (b, root_b) = engine_for("ref-without", &additive_scene(1, ""), None);
    let (with, without) = (palettes_at(&a, 20), palettes_at(&b, 20));
    assert!(
        with[0]
            .iter()
            .zip(&without[0])
            .any(|(m, n)| !m.abs_diff_eq(*n, 1e-5)),
        "the reference clip moves the additive delta"
    );
    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }

    let many = additive_scene(120, "            reference: idle\n");
    let runs: Vec<Vec<u32>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (engine, root) = engine_for(&format!("ref-threads{n}"), &many, None);
                let palette = extract(&engine, 20)
                    .bone_palette
                    .iter()
                    .flat_map(|m| m.to_cols_array().map(f32::to_bits))
                    .collect();
                let _ = std::fs::remove_dir_all(&root);
                palette
            })
        })
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 2 threads");
    assert_eq!(runs[0], runs[2], "1 vs 8 threads");
}

#[test]
fn a_plain_clip_layer_scene_never_gets_a_clock_and_extraction_moves_no_hash() {
    let text = scene(RUN, &layer("upper", "idle", ""));
    let run = |tag: &str, extract_each_tick: bool| {
        let (mut engine, root) = engine_for(tag, &text, None);
        let mut trace = Vec::new();
        for _ in 0..60 {
            let one = Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            });
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![one])
                    .expect("the session drives")
                    .into_iter()
                    .skip(1),
            );
            if extract_each_tick {
                assert_eq!(palettes_at(&engine, engine.frame().as_u64()).len(), 1);
            }
        }
        assert!(clock(&engine).is_none(), "no layer blend, no clock");
        let _ = std::fs::remove_dir_all(&root);
        trace
    };
    let (quiet, drawn) = (run("hash-quiet", false), run("hash-drawn", true));
    assert_eq!(quiet.len(), 60);
    assert_eq!(quiet, drawn, "extraction moved a world hash");
}
