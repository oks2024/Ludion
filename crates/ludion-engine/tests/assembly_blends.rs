//! Runtime proof for blend spaces on voxel assemblies (`Plan/animation-layers.md` decisions 31-34,
//! Neon Shroud request 0007 A2): an assembly's `Animation.clip` (or an `AnimationLayer.clip`) names a
//! 1D or 2D blend of its `blends:` key, driven by `params`, its point clips advancing on one shared
//! phase held in a hashed `BlendClock`, and the footstep events firing once per stride at every speed.
//!
//! - **Cadence.** A parameter sweep fires `foot_l` and `foot_r` alternately, once per stride.
//! - **Pose.** Params on a point pose the parts exactly as that clip alone at the mapped time; a 2D
//!   blend poses between its points.
//! - **Neutral.** Extraction moves no hash; a plain-clip assembly never gets a `BlendClock`.
//! - **Deterministic.** Hash traces and matrices match at 1, 2 and 8 threads; a snapshot restored
//!   mid-sweep reproduces the phase, the matrices and the event log.
//! - **Crossfade, layers, ragdoll.** A fade idle to blend and back keeps events single; a blend layer
//!   on an arm mask moves only the masked parts; a ragdoll switch spawns at the blended pose.
//! - **Refused.** An unbound point clip and a missing `sync` event are `L1138`.
//!
//! The fixture is a humanoid-ish assembly in a scratch directory.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::math::glam::Mat4;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Animation, AnimationEvents, BlendClock, ClipEvent, Engine, Mesh, QualityParams,
    animation_events, blend_clocks, extract_at, load_assembly, switch_to_ragdoll,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};

const LEG: usize = 0;
const TORSO: usize = 1;
const HEAD: usize = 2;
const ARM: usize = 3;
const FOOT: usize = 4;
const PARTS: usize = 5;

const ASSEMBLY_REL: &str = "models/humanoid.assembly.yaml";

/// The humanoid with `idle` (2 s), `walk` (1 s) and `run` (0.5 s) looping clips, each with `foot_l`
/// and `foot_r` events half a stride apart, a 1D `locomotion` blend and a 2D `cross` blend. The leg
/// is authored before its parent torso.
fn assembly(blends: &str) -> String {
    format!(
        "format: 1
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
  idle: anims/idle.anim.yaml
  run: anims/run.anim.yaml
  walk: anims/walk.anim.yaml
masks:
  arm_only:
    joints:
      - arm
{blends}"
    )
}

const BLENDS: &str = "blends:
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

/// One track: `target`'s `channel` keyed `from` at `t = 0` and `to` at `t = duration`.
fn track(target: &str, channel: &str, from: [f64; 3], to: [f64; 3], duration: f64) -> String {
    let keys = |t: f64, v: [f64; 3]| {
        format!(
            "      - t: {t:?}\n        v:\n          - {:?}\n          - {:?}\n          - {:?}\n",
            v[0], v[1], v[2]
        )
    };
    format!(
        "  {target}:\n    {channel}:\n{}{}",
        keys(0.0, from),
        keys(duration, to)
    )
}

/// A looping clip of `duration` seconds with `foot_l` at a quarter and `foot_r` at three quarters.
fn clip(name: &str, duration: f64, tracks: &str, events: bool) -> String {
    let events = if events {
        format!(
            "events:\n  - t: {:?}\n    name: foot_l\n  - t: {:?}\n    name: foot_r\n",
            duration * 0.25,
            duration * 0.75
        )
    } else {
        String::new()
    };
    format!(
        "format: 1\nkind: animation\nname: {name}\nduration: {duration:?}\nloop: true\n{events}tracks:\n{tracks}"
    )
}

fn clips(events: bool) -> Vec<(&'static str, String)> {
    let idle = format!(
        "{}{}",
        track("torso", "rotation", [0.0; 3], [0.0, 0.0, 6.0], 2.0),
        track("arm", "rotation", [0.0; 3], [5.0, 0.0, 0.0], 2.0)
    );
    let walk = format!(
        "{}{}{}",
        track("leg", "rotation", [0.0; 3], [30.0, 0.0, 0.0], 1.0),
        track("arm", "rotation", [0.0; 3], [45.0, 0.0, 0.0], 1.0),
        track("head", "translation", [0.0; 3], [0.0, 1.0, 0.0], 1.0)
    );
    let run = format!(
        "{}{}{}",
        track("leg", "rotation", [0.0; 3], [60.0, 0.0, 0.0], 0.5),
        track("arm", "rotation", [0.0; 3], [90.0, 0.0, 0.0], 0.5),
        track("torso", "rotation", [0.0; 3], [0.0, 20.0, 0.0], 0.5)
    );
    vec![
        ("idle", clip("idle", 2.0, &idle, events)),
        ("walk", clip("walk", 1.0, &walk, events)),
        ("run", clip("run", 0.5, &run, events)),
    ]
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn project(name: &str, assembly: &str, events: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-assembly-blends-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, ASSEMBLY_REL, assembly);
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    for (stem, text) in clips(events) {
        write(&dir, &format!("models/anims/{stem}.anim.yaml"), &text);
    }
    dir
}

/// One `runner` entity with `animation` (indented 8) and optional `layers` (list items indented 10).
fn scene_with(animation: &str, layers: &str) -> String {
    let layered = if layers.is_empty() {
        String::new()
    } else {
        format!("      AnimationLayers:\n        layers:\n{layers}")
    };
    format!(
        "format: 1
seed: 1
entities:
  - name: runner
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 2.0
      Mesh:
        shape:
          assembly: {ASSEMBLY_REL}
      Animation:
{animation}{layered}"
    )
}

fn scene(animation: &str) -> String {
    scene_with(animation, "")
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
    let root = project(name, &assembly(BLENDS), true);
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

fn matrices_at(engine: &Engine, tick: u64) -> Vec<Mat4> {
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

fn bits(matrices: &[Mat4]) -> Vec<[u32; 16]> {
    matrices
        .iter()
        .map(|m| m.to_cols_array().map(f32::to_bits))
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
    let blended = "        clip: locomotion\n        params:\n          - 2.0\n          - 0.0\n";
    let (a, root_a) = engine_for("point-blend", &scene(blended), None);
    let (b, root_b) = engine_for("point-plain", &scene("        clip: walk\n"), None);
    // Phase 0 maps the sync event (a quarter of `walk`) to the start: clip time 0.25 s, which the
    // plain `walk` reaches after fifteen ticks.
    assert_eq!(
        bits(&matrices_at(&a, 0)),
        bits(&matrices_at(&b, 15)),
        "bit for bit at the sync instant"
    );
    // Six ticks later both have advanced the same 0.1 s.
    for (m, n) in matrices_at(&a, 6).iter().zip(&matrices_at(&b, 21)) {
        assert!(m.abs_diff_eq(*n, 1e-5), "{m:?} vs {n:?}");
    }
    assert_eq!(matrices_at(&a, 0).len(), PARTS);
    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
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
        matrices_at(&point, 3)[ARM],
        matrices_at(&mid, 3)[ARM],
        matrices_at(&idle, 3)[ARM],
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
            assert_eq!(matrices_at(engine, engine.frame().as_u64()).len(), PARTS);
        }
    }
    trace
}

#[test]
fn extracting_a_blended_assembly_moves_no_hash() {
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
fn traces_and_matrices_match_at_1_2_8_threads() {
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    for i in 0..120 {
        text.push_str(&format!(
            "  - name: m{i}\n    components:\n      Mesh:\n        shape:\n          assembly: {ASSEMBLY_REL}\n      Animation:\n        clip: {}\n        params:\n          - {}\n          - 0.5\n",
            if i % 2 == 0 { "locomotion" } else { "cross" },
            f64::from(i % 7)
        ));
    }
    let runs: Vec<(Vec<_>, Vec<[u32; 16]>)> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let (mut engine, root) = engine_for(&format!("threads{n}"), &text, None);
                let trace = trace_of(&mut engine, 20, false);
                let matrices = bits(&matrices_at(&engine, 20));
                let _ = std::fs::remove_dir_all(&root);
                (trace, matrices)
            })
        })
        .collect();
    assert_eq!(runs[0], runs[1], "1 vs 2 threads");
    assert_eq!(runs[0], runs[2], "1 vs 8 threads");
}

#[test]
fn a_snapshot_restored_mid_sweep_reproduces_phase_matrices_and_events() {
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
    let want_matrices = bits(&matrices_at(&engine, engine.frame().as_u64()));
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
    assert_eq!(
        bits(&matrices_at(&engine, engine.frame().as_u64())),
        want_matrices
    );
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
fn a_blend_layer_on_an_arm_mask_moves_only_masked_parts() {
    let layer = "          - name: stride\n            clip: locomotion\n            weight: 1.0\n            mask: arm_only\n            params:\n              - 5.0\n              - 0.0\n";
    let base = "        clip: walk\n";
    let (mut plain, root_a) = engine_for("layer-plain", &scene(base), Some(sweep));
    let (mut layered, root_b) = engine_for("layer-on", &scene_with(base, layer), Some(sweep));
    step(&mut plain, 20);
    step(&mut layered, 20);
    let entry = clock(&layered).expect("the layer blend holds a clock");
    assert_eq!(entry.layers.len(), 1, "{entry:?}");
    assert_eq!(entry.layers[0].name, "stride");
    assert!(entry.blend.is_empty(), "no base blend: {entry:?}");
    let (a, b) = (matrices_at(&plain, 20), matrices_at(&layered, 20));
    for part in [LEG, TORSO, HEAD, FOOT] {
        assert_eq!(a[part], b[part], "part {part} keeps the base pose");
    }
    assert_ne!(a[ARM], b[ARM], "the arm takes the blend");
    for root in [root_a, root_b] {
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_plain_assembly_never_gets_a_blend_clock() {
    let (mut engine, root) = engine_for("plain", &scene("        clip: walk\n"), None);
    step(&mut engine, 30);
    assert!(clock(&engine).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_ragdoll_switch_from_a_blended_base_spawns_at_the_blended_pose() {
    let text =
        scene("        clip: locomotion\n        params:\n          - 3.0\n          - 0.0\n");
    let (mut engine, root) = engine_for("ragdoll", &text, None);
    step(&mut engine, 20);
    assert!(clock(&engine).is_some());
    let now = SimTime::from_ticks(20, TickRate::DEFAULT);
    let pre = matrices_at(&engine, 20);
    assert_eq!(pre.len(), PARTS);
    let data = load_assembly(ludion_assets::AssetRoot::content(&root), ASSEMBLY_REL)
        .expect("the assembly loads");
    let figure = engine
        .world()
        .entities()
        .find(|&id| engine.world().component::<Mesh>(id).is_some())
        .expect("the runner");
    switch_to_ragdoll(engine.world_mut(), figure, &data, now).expect("a valid switch");
    let post = matrices_at(&engine, 20);
    assert_eq!(post.len(), PARTS);
    for (index, (before, after)) in pre.iter().zip(&post).enumerate() {
        let (a, b) = (before.to_cols_array(), after.to_cols_array());
        for (x, y) in a.iter().zip(&b) {
            assert!(
                (x - y).abs() <= 1e-4,
                "part {index}: {before:?} vs {after:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn load_errors(name: &str, blends: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    let root = project(name, &assembly(blends), true);
    let errors = load_assembly(ludion_assets::AssetRoot::content(&root), ASSEMBLY_REL)
        .expect_err("the blend is refused");
    let _ = std::fs::remove_dir_all(&root);
    errors
}

#[test]
fn bad_blends_are_l1138() {
    let unbound = load_errors(
        "unbound",
        "blends:\n  b:\n    points:\n      - clip: idle\n        at: 0.0\n      - clip: sprint\n        at: 1.0\n",
    );
    assert!(
        unbound
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/blends/b/points/1/clip")),
        "{unbound:?}"
    );
    let no_sync = load_errors(
        "no-sync",
        "blends:\n  b:\n    sync: foot_x\n    points:\n      - clip: idle\n        at: 0.0\n      - clip: run\n        at: 1.0\n",
    );
    assert!(
        no_sync
            .iter()
            .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY
                && d.path.as_deref() == Some("/blends/b/sync")),
        "{no_sync:?}"
    );
}
