//! Runtime proof for `AnimGraph` on a voxel assembly (`Plan/animation-layers.md` decisions 50-53, Neon
//! Shroud request 0007 §6): a three-state graph (`Idle`, `Carry` with a `carry` layer, a one-shot `Wave`)
//! writes `Animation` and `AnimationLayers` only on a transition, the one-shot leaves on its `end` tick,
//! `Interrupts` cuts into it, an `extract_at` pose shows the layer driving parts, the hash trace is
//! thread-invariant, and a missing layer is `L4290`.
//!
//! The fixture is a small humanoid in a scratch directory (`assembly_layers.rs` shape): clips `idle`
//! (loop, head only), `wave` (`loop: false`, so `end` fires) and `carry` (loop, arm), plus an
//! `arm_only` mask for the layer.

use std::path::{Path, PathBuf};

use ludion_core::math::glam::Mat4;
use ludion_core::time::{FrameIndex, SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AnimEdge, AnimGate, AnimGraph, AnimGraphState, AnimState, Animation, AnimationEvents,
    AnimationLayers, Engine, Mesh, QualityParams, animation_events, extract_at,
};

const ARM: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pose {
    Idle,
    Carry,
    Wave,
}

struct Inputs {
    carrying: bool,
    wave: bool,
}

/// What the game wants at gameplay frame `frame`: carry from 10 to 40, one wave request at 50.
fn inputs_at(frame: u64) -> Inputs {
    Inputs {
        carrying: (10..40).contains(&frame),
        wave: frame == 50,
    }
}

static GRAPH: AnimGraph<Pose, Inputs> = AnimGraph {
    states: &[
        AnimState {
            id: Pose::Idle,
            name: "idle",
            plays: "idle",
            layers: &[],
            one_shot: false,
        },
        AnimState {
            id: Pose::Carry,
            name: "carry",
            plays: "idle",
            layers: &[("carry", "carry", 1.0)],
            one_shot: false,
        },
        AnimState {
            id: Pose::Wave,
            name: "wave",
            plays: "wave",
            layers: &[],
            one_shot: true,
        },
    ],
    edges: &[
        AnimEdge {
            from: None,
            to: Pose::Wave,
            when: |i| i.wave,
            fade: 0.0,
            gate: AnimGate::Interrupts,
        },
        AnimEdge {
            from: Some(Pose::Idle),
            to: Pose::Carry,
            when: |i| i.carrying,
            fade: 0.2,
            gate: AnimGate::None,
        },
        AnimEdge {
            from: Some(Pose::Carry),
            to: Pose::Idle,
            when: |i| !i.carrying,
            fade: 0.2,
            gate: AnimGate::None,
        },
        AnimEdge {
            from: Some(Pose::Wave),
            to: Pose::Idle,
            when: |_| true,
            fade: 0.15,
            gate: AnimGate::OnEvent("end"),
        },
    ],
};

/// `(gameplay frame, state entered, whether the previous state's clip fired `end` that tick)`.
#[derive(Default)]
struct Log(Vec<(u64, Pose, bool)>);

fn graph_system(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<_> = world
        .query_ids::<(&AnimGraphState,)>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        let clip = world
            .component::<Animation>(entity)
            .map(|a| a.clip.clone())
            .unwrap_or_default();
        let ended = world
            .resource::<AnimationEvents>()
            .is_some_and(|events| events.fired(entity, &clip, "end"));
        let inputs = inputs_at(ctx.game_frame.as_u64());
        if let Some(to) = GRAPH
            .step(world, entity, &inputs, ctx)
            .expect("the graph steps")
        {
            if world.resource::<Log>().is_none() {
                world.insert_resource(Log::default());
            }
            world.resource_mut::<Log>().expect("just inserted").0.push((
                ctx.game_frame.as_u64(),
                to,
                ended,
            ));
        }
    }
}

/// Block-style YAML only: the strict subset rejects flow collections.
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
clips:
  idle: anims/idle.anim.yaml
  wave: anims/wave.anim.yaml
  carry: anims/carry.anim.yaml
masks:
  arm_only:
    joints:
      - arm
";

fn clip(name: &str, looping: bool, target: &str, to: [f64; 3]) -> String {
    let end: String = to.iter().map(|v| format!("          - {v:?}\n")).collect();
    format!(
        "format: 1\nkind: animation\nname: {name}\nduration: 1.0\nloop: {looping}\ntracks:\n  {target}:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 1.0\n        v:\n{end}"
    )
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-anim-graph-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "models/humanoid.assembly.yaml", ASSEMBLY);
    write(
        &dir,
        "models/parts/cell.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    write(
        &dir,
        "models/anims/idle.anim.yaml",
        &clip("idle", true, "head", [0.0, 10.0, 0.0]),
    );
    write(
        &dir,
        "models/anims/wave.anim.yaml",
        &clip("wave", false, "arm", [0.0, 0.0, 90.0]),
    );
    write(
        &dir,
        "models/anims/carry.anim.yaml",
        &clip("carry", true, "arm", [90.0, 0.0, 0.0]),
    );
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

const SCENE: &str = "format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Mesh:
        shape:
          assembly: models/humanoid.assembly.yaml
      Animation:
        clip: idle
      AnimationLayers:
        layers:
          - name: carry
            mask: arm_only
      AnimGraphState:
        state: idle
";

const SCENE_NO_LAYERS: &str = "format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Mesh:
        shape:
          assembly: models/humanoid.assembly.yaml
      Animation:
        clip: idle
      AnimGraphState:
        state: idle
";

fn engine_at(root: &Path, scene: &str, with_graph: bool) -> Engine {
    let mut builder = Engine::builder().project_root(root);
    if with_graph {
        builder = builder
            .system("animation_events", animation_events)
            .query::<(&Animation, &Mesh)>()
            .system("graph_system", graph_system)
            .query::<(&mut Animation,)>()
            .query::<(&mut AnimationLayers,)>()
            .query::<(&mut AnimGraphState,)>();
    }
    let mut engine = builder.build().expect("the engine builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn arm_at(engine: &Engine, tick: u64) -> Mat4 {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the fixture world extracts")
    .meshes[ARM]
        .model
}

fn run(engine: &mut Engine, ticks: u64) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn log(engine: &Engine) -> Vec<(u64, Pose, bool)> {
    engine
        .world()
        .resource::<Log>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

#[test]
fn transitions_land_on_the_ticks_the_inputs_ask_and_one_shots_leave_on_end() {
    let root = fixture_project("transitions");
    let mut engine = engine_at(&root, SCENE, true);
    run(&mut engine, 130);
    let log = log(&engine);
    let order: Vec<(u64, Pose)> = log.iter().map(|(f, p, _)| (*f, *p)).collect();
    assert_eq!(
        order,
        [
            (10, Pose::Carry),
            (40, Pose::Idle),
            (50, Pose::Wave),
            (110, Pose::Idle),
        ],
        "transition log: {log:?}"
    );
    assert!(
        !log[2].2 && log[3].2,
        "wave leaves exactly on its `end` tick: {log:?}"
    );
    assert_eq!(log[3].0 - log[2].0, 60, "the one-second clip");
    let state = engine
        .world_mut()
        .query_ids::<(&AnimGraphState,)>()
        .next()
        .unwrap()
        .1
        .0
        .clone();
    assert_eq!(
        (state.state.as_str(), state.previous.as_str()),
        ("idle", "wave")
    );
    assert_eq!(state.entered, FrameIndex(110));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_layer_is_enabled_on_entry_faded_out_on_leaving_and_drives_a_part() {
    let root = fixture_project("layer");
    let mut engine = engine_at(&root, SCENE, true);
    let rest = arm_at(&engine, 5);
    run(&mut engine, 30);
    let layers = |engine: &mut Engine| {
        engine
            .world_mut()
            .query_ids::<(&AnimationLayers,)>()
            .next()
            .unwrap()
            .1
            .0
            .layers[0]
            .clone()
    };
    let on = layers(&mut engine);
    assert_eq!((on.clip.as_str(), on.weight, on.fade), ("carry", 1.0, 0.2));
    assert!(
        !arm_at(&engine, 30).abs_diff_eq(rest, 1e-5),
        "the carry layer moves the arm off its rest pose"
    );
    run(&mut engine, 20);
    let off = layers(&mut engine);
    assert_eq!(
        (off.weight, off.fade),
        (0.0, 0.2),
        "leaving fades the layer to 0"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hash_traces_are_identical_at_1_2_8_threads() {
    let root = fixture_project("threads");
    let traces: Vec<Vec<u64>> = [1, 2, 8]
        .into_iter()
        .map(|n| {
            ludion_core::par::with_threads(n, || {
                let mut engine = engine_at(&root, SCENE, true);
                (0..130)
                    .map(|_| {
                        engine.advance_live();
                        engine.world_hash().unwrap().as_u64()
                    })
                    .collect()
            })
        })
        .collect();
    assert_eq!(traces[0], traces[1], "1 vs 2 threads");
    assert_eq!(traces[0], traces[2], "1 vs 8 threads");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_layer_is_l4290_and_writes_nothing() {
    let root = fixture_project("missing");
    let mut engine = engine_at(&root, SCENE_NO_LAYERS, false);
    let (entity, _) = engine
        .world_mut()
        .query_ids::<(&AnimGraphState,)>()
        .next()
        .unwrap();
    let before =
        serde_json::to_string(engine.world().component::<Animation>(entity).unwrap()).unwrap();
    let ctx = SystemCtx::new(FrameIndex(20), TickRate::DEFAULT);
    let err = GRAPH
        .step(engine.world_mut(), entity, &inputs_at(20), &ctx)
        .expect_err("entering `carry` lists a layer the entity lacks");
    assert_eq!(err.code.to_string(), "L4290");
    let after =
        serde_json::to_string(engine.world().component::<Animation>(entity).unwrap()).unwrap();
    assert_eq!(before, after);
    assert_eq!(GRAPH.current(engine.world(), entity).unwrap(), Pose::Idle);
    let _ = std::fs::remove_dir_all(&root);
}
