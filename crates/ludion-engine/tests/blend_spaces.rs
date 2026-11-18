//! Runtime proof for blend spaces (`Plan/animation-layers.md` W2a, Neon Shroud request 0007 piece
//! 3): a character's `Animation.clip` names a 1D or 2D blend driven by `Animation.params`, its point
//! clips advance on one shared phase held in a hashed `BlendClock`, and the footstep events fire
//! once per stride at every speed.
//!
//! - **Cadence.** A parameter sweep fires `foot_l` and `foot_r` alternately, once per stride.
//! - **Pose.** Params on a point pose exactly as that clip alone.
//! - **Neutral.** Extraction moves no hash; a plain-clip entity never gets a `BlendClock`.
//! - **Deterministic.** Hash traces and palettes match at 1, 2 and 8 threads; a snapshot restored
//!   mid-sweep reproduces the phase, the palette and the event log.
//! - **Refused.** An unbound point clip, a missing `sync` event and a name clash are `L1200`.
//!
//! Runs over a scratch copy of the gallery mannequin in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::math::glam::Mat4;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Animation, AnimationEvents, BlendClock, ClipEvent, Engine, Mesh, QualityParams,
    animation_events, blend_clocks, extract_at,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};

const JOINTS: usize = 11;

/// The mannequin with `walk` (speed 0.6) and `run` (speed 1.2) on the one run clip, plus `idle`,
/// each with `foot_l`/`foot_r` overlay events half a stride apart, and two blends.
const CHARACTER: &str = "\
format: 1
kind: character
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
      - clip: walk
        at:
          - -2.0
          - 0.0
      - clip: run
        at:
          - 0.0
          - -2.0
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

fn character_with(blends: &str) -> String {
    let head = CHARACTER.split("blends:").next().expect("a blends key");
    format!("{head}blends:\n{blends}")
}

fn scene(animation: &str) -> String {
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
{animation}"
    )
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn fixture_project(name: &str, character: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-blend-spaces-{}-{name}", std::process::id()));
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
    std::fs::write(dir.join("characters/blended.character.yaml"), character).unwrap();
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
            .query::<(&mut Animation,)>();
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

/// Writes `params = [tick / 60, 0]` (0 to 5 over 300 ticks).
fn sweep(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let x = (ctx.game_frame.as_u64() as f32 / 300.0 * 5.0).min(5.0);
    for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        animation.params = [x, 0.0];
    }
}

/// Idle, then a 0.5 s crossfade into `locomotion` at 30 and back to `idle` at 150.
fn there_and_back(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let frame = ctx.game_frame.as_u64();
    for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        match frame {
            30 => {
                animation.params = [5.0, 0.0];
                ludion_engine::crossfade(animation, "locomotion", 0.5, ctx.game_frame);
            }
            150 => ludion_engine::crossfade(animation, "idle", 0.5, ctx.game_frame),
            _ => {}
        }
    }
}

fn engine_for(name: &str, text: &str, driver: Option<Driver>) -> (Engine, PathBuf) {
    let root = fixture_project(name, CHARACTER);
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

const SWEEP_SCENE: &str = "        clip: locomotion\n";

#[test]
fn a_sweep_fires_each_foot_once_per_stride() {
    let (mut engine, root) = engine_for("sweep", &scene(SWEEP_SCENE), Some(sweep));
    step(&mut engine, 300);
    let events = log(&engine);
    let phase = clock(&engine).expect("the runner carries a clock").phase;
    assert!(phase > 3.0, "the sweep covers several strides: {phase}");
    let names: Vec<&str> = events.iter().map(|(_, e)| e.name.as_str()).collect();
    assert!(
        names.chunks(2).all(|pair| pair[0] == "foot_l")
            && names.windows(2).all(|pair| pair[0] != pair[1]),
        "feet alternate: {names:?}"
    );
    assert!(events.iter().all(|(_, e)| e.clip == "locomotion"));
    let left = names.iter().filter(|n| **n == "foot_l").count();
    let right = names.len() - left;
    // `foot_l` sits at phase 0, 1, 2, ...; `foot_r` at 0.5, 1.5, ...
    assert_eq!(left, phase.floor() as usize + 1);
    assert_eq!(right, (phase - 0.5).floor() as usize + 1);
    // The sweep speeds up, so the gap between left feet never grows by more than a tick.
    let lefts: Vec<u64> = events
        .iter()
        .filter(|(_, e)| e.name == "foot_l")
        .map(|(tick, _)| *tick)
        .collect();
    let gaps: Vec<u64> = lefts.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        gaps.windows(2).all(|g| g[1] <= g[0] + 1),
        "cadence: {gaps:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn params_on_a_point_pose_as_that_clip_alone() {
    let blended = |params: &str| format!("        clip: locomotion\n        params:\n{params}");
    let (a, root_a) = engine_for(
        "point-blend",
        &scene(&blended("          - 5.0\n          - 0.0\n")),
        None,
    );
    let (b, root_b) = engine_for("point-plain", &scene("        clip: run\n"), None);
    // Phase 0 maps the sync event (0.2 s of `run`) to the start: clip time 0.2 s. The plain `run`
    // binding (speed 1.2) reaches 0.2 s after ten ticks.
    for (blend_tick, plain_tick) in [(0, 10), (6, 16)] {
        let (p, q) = (
            &palettes_at(&a, blend_tick)[0],
            &palettes_at(&b, plain_tick)[0],
        );
        for (m, n) in p.iter().zip(q) {
            assert!(m.abs_diff_eq(*n, 1e-6), "tick {blend_tick}: {m:?} vs {n:?}");
        }
    }
    let _ = (
        std::fs::remove_dir_all(&root_a),
        std::fs::remove_dir_all(&root_b),
    );
}

#[test]
fn a_two_d_blend_poses_between_its_points() {
    let text = |params: &str| scene(&format!("        clip: cross\n        params:\n{params}"));
    let (point, root_a) = engine_for(
        "cross-point",
        &text("          - 2.0\n          - 0.0\n"),
        None,
    );
    let (mid, root_b) = engine_for(
        "cross-mid",
        &text("          - 1.0\n          - 0.0\n"),
        None,
    );
    let (idle, root_c) = engine_for(
        "cross-idle",
        &text("          - 0.0\n          - 0.0\n"),
        None,
    );
    let (p, m, i) = (
        palettes_at(&point, 3)[0][3],
        palettes_at(&mid, 3)[0][3],
        palettes_at(&idle, 3)[0][3],
    );
    assert!(
        !p.abs_diff_eq(m, 1e-5) && !m.abs_diff_eq(i, 1e-5),
        "the midpoint is its own pose"
    );
    for root in [root_a, root_b, root_c] {
        let _ = std::fs::remove_dir_all(root);
    }
}

fn trace_of(
    engine: &mut Engine,
    ticks: u64,
    extract_each_tick: bool,
) -> Vec<(ludion_core::time::FrameIndex, ludion_sim::WorldHash)> {
    let mut trace = Vec::new();
    for _ in 0..ticks {
        let one = Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        });
        trace.extend(
            ludion_engine::drive_hashes(engine, vec![one])
                .expect("the session drives")
                .into_iter()
                .skip(1),
        );
        if extract_each_tick {
            assert_eq!(palettes_at(engine, engine.frame().as_u64()).len(), 1);
        }
    }
    trace
}

#[test]
fn extracting_a_blended_character_moves_no_hash() {
    let run = |tag: &str, extract_each_tick: bool| {
        let (mut engine, root) = engine_for(tag, &scene(SWEEP_SCENE), Some(sweep));
        let trace = trace_of(&mut engine, 60, extract_each_tick);
        let _ = std::fs::remove_dir_all(&root);
        trace
    };
    let (quiet, drawn) = (run("quiet", false), run("drawn", true));
    assert_eq!(quiet.len(), 60);
    assert_eq!(quiet, drawn, "extraction moved a world hash");
}

#[test]
fn traces_and_palettes_match_at_1_2_8_threads() {
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    for i in 0..240 {
        text.push_str(&format!(
            "  - name: m{i}\n    components:\n      Mesh:\n        shape:\n          character: characters/blended.character.yaml\n      Animation:\n        clip: {}\n        params:\n          - {}\n          - 0.5\n",
            if i % 2 == 0 { "locomotion" } else { "cross" },
            f64::from(i % 7)
        ));
    }
    let runs: Vec<(Vec<_>, Vec<u32>)> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (mut engine, root) = engine_for(&format!("threads{n}"), &text, None);
                let trace = trace_of(&mut engine, 20, false);
                let palette = extract(&engine, 20)
                    .bone_palette
                    .iter()
                    .flat_map(|m| m.to_cols_array().map(f32::to_bits))
                    .collect();
                let _ = std::fs::remove_dir_all(&root);
                (trace, palette)
            })
        })
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 2 threads");
    assert_eq!(runs[0], runs[2], "1 vs 8 threads");
}

#[test]
fn a_snapshot_restored_mid_sweep_reproduces_phase_palette_and_events() {
    let (mut engine, root) = engine_for("restore", &scene(SWEEP_SCENE), Some(sweep));
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
    assert!(!want_events.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_crossfade_into_a_blend_and_back_keeps_events_single() {
    let (mut engine, root) =
        engine_for("fade", &scene("        clip: idle\n"), Some(there_and_back));
    step(&mut engine, 100);
    assert!(clock(&engine).is_some(), "the blend holds a clock");
    step(&mut engine, 200);
    assert!(
        clock(&engine).is_none(),
        "the clock goes once neither side is a blend"
    );
    let events = log(&engine);
    let in_blend: Vec<&(u64, ClipEvent)> = events
        .iter()
        .filter(|(_, e)| e.clip == "locomotion")
        .collect();
    assert!(!in_blend.is_empty());
    assert!(
        in_blend.iter().all(|(tick, _)| (30..=150).contains(tick)),
        "{in_blend:?}"
    );
    for clip in ["locomotion", "idle"] {
        let names: Vec<&str> = events
            .iter()
            .filter(|(_, e)| e.clip == clip)
            .map(|(_, e)| e.name.as_str())
            .collect();
        assert!(
            names.windows(2).all(|pair| pair[0] != pair[1]),
            "{clip}: {names:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_plain_clip_never_gets_a_blend_clock() {
    let (mut engine, root) = engine_for("plain", &scene("        clip: run\n"), None);
    step(&mut engine, 30);
    assert!(clock(&engine).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

fn load_errors(name: &str, blends: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    let root = fixture_project(name, &character_with(blends));
    let errors = ludion_engine::load_character(
        ludion_assets::AssetRoot::content(&root),
        "characters/blended.character.yaml",
    )
    .expect_err("the blend is refused");
    let _ = std::fs::remove_dir_all(&root);
    errors
}

#[test]
fn bad_blends_are_l1200() {
    let points =
        "    points:\n      - clip: idle\n        at: 0.0\n      - clip: run\n        at: 1.0\n";
    let unbound = load_errors(
        "unbound",
        "  b:\n    points:\n      - clip: idle\n        at: 0.0\n      - clip: sprint\n        at: 1.0\n",
    );
    assert!(
        unbound
            .iter()
            .any(|d| d.code == code::CHARACTER_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/blends/b/points/1/clip")),
        "{unbound:?}"
    );
    let no_sync = load_errors("no-sync", &format!("  b:\n    sync: foot_x\n{points}"));
    assert!(
        no_sync
            .iter()
            .any(|d| d.code == code::CHARACTER_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/blends/b/sync")),
        "{no_sync:?}"
    );
    let clash = load_errors("clash", &format!("  run:\n{points}"));
    assert!(
        clash
            .iter()
            .any(|d| d.code == code::CHARACTER_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/blends/run")),
        "{clash:?}"
    );
}
