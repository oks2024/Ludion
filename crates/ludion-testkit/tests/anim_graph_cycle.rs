//! A six-state `AnimGraph` port of `common::CYCLE` (`Plan/animation-layers.md` decisions 50-53): driven
//! side by side with `common::cycle_builder` on the gallery's `character-cycle` scene, the graph writes
//! the identical `Animation` on every tick, and so the identical switch log. `CYCLE` itself, its trace
//! and its replay stay untouched: the hashed `AnimGraphState` would move them (an R14 re-record item).

mod common;

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AnimEdge, AnimGate, AnimGraph, AnimGraphState, AnimState, Animation, AnimationEvents, Engine,
    EngineBuilder, Mesh, PrefabInstance, Sprite, animation_events,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

const TICKS: usize = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pose {
    Idle,
    RunA,
    Jump,
    RunB,
    Spin,
    RunC,
}

const fn state(
    id: Pose,
    name: &'static str,
    plays: &'static str,
    one_shot: bool,
) -> AnimState<Pose> {
    AnimState {
        id,
        name,
        plays,
        layers: &[],
        one_shot,
    }
}

const fn edge(from: Pose, to: Pose, gate: AnimGate, fade: f32) -> AnimEdge<Pose, ()> {
    AnimEdge {
        from: Some(from),
        to,
        when: |_| true,
        fade,
        gate,
    }
}

/// `common::CYCLE` as states and edges.
static CYCLE_GRAPH: AnimGraph<Pose, ()> = AnimGraph {
    states: &[
        state(Pose::Idle, "idle", "idle", false),
        state(Pose::RunA, "run_a", "run", false),
        state(Pose::Jump, "jump", "jump", true),
        state(Pose::RunB, "run_b", "run", false),
        state(Pose::Spin, "spin", "spin", true),
        state(Pose::RunC, "run_c", "run", false),
    ],
    edges: &[
        edge(
            Pose::Idle,
            Pose::RunA,
            AnimGate::MinTicks(common::IDLE_TICKS),
            common::RUN_IN_FADE,
        ),
        edge(
            Pose::RunA,
            Pose::Jump,
            AnimGate::MinTicks(common::RUN_TICKS),
            0.1,
        ),
        edge(Pose::Jump, Pose::RunB, AnimGate::OnEvent("end"), 0.15),
        edge(
            Pose::RunB,
            Pose::Spin,
            AnimGate::MinTicks(common::RUN_TICKS),
            0.0,
        ),
        edge(Pose::Spin, Pose::RunC, AnimGate::OnEvent("end"), 0.15),
        edge(
            Pose::RunC,
            Pose::Idle,
            AnimGate::MinTicks(common::RUN_TICKS),
            0.3,
        ),
    ],
};

fn graph_cycle(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<EntityId> = world
        .query_ids::<(&Animation,)>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        if world.component::<AnimGraphState>(entity).is_some() {
            CYCLE_GRAPH
                .step(world, entity, &(), ctx)
                .expect("the graph steps");
        }
    }
}

/// `common::cycle_builder` with `graph_cycle` in place of `cycle_character`.
fn graph_builder() -> EngineBuilder {
    Engine::builder()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("graph_cycle", graph_cycle)
        .query::<(&mut Animation,)>()
        .query::<(&mut AnimGraphState,)>()
}

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn load(builder: EngineBuilder, graph: bool) -> Engine {
    let mut engine = builder
        .project_root(gallery_root())
        .build()
        .expect("the engine builds");
    engine
        .load_inline_scene(
            &std::fs::read_to_string(gallery_root().join("character-cycle.yaml"))
                .expect("the committed scene"),
        )
        .expect("the committed scene loads");
    if graph {
        let (mannequin, _) = engine
            .world_mut()
            .query_ids::<(&Animation,)>()
            .next()
            .expect("the mannequin plays a clip");
        engine
            .world_mut()
            .insert(
                mannequin,
                AnimGraphState {
                    state: "idle".into(),
                    ..AnimGraphState::default()
                },
            )
            .expect("the state inserts");
    }
    engine
}

fn animation(engine: &mut Engine) -> Animation {
    engine
        .world_mut()
        .query_ids::<(&Animation,)>()
        .next()
        .map(|(_, (animation,))| animation.clone())
        .expect("the mannequin plays a clip")
}

#[test]
fn the_graph_writes_the_identical_animation_every_tick() {
    let mut table = load(common::cycle_builder(), false);
    let mut graph = load(graph_builder(), true);
    for tick in 0..TICKS {
        table.advance_live();
        graph.advance_live();
        assert_eq!(animation(&mut table), animation(&mut graph), "tick {tick}");
    }
}

#[test]
fn the_graph_switches_in_the_cycle_order_and_ends_in_idle() {
    let mut engine = load(graph_builder(), true);
    let mut log: Vec<(String, String, bool)> = Vec::new();
    let mut previous = String::from("idle");
    for _ in 0..TICKS {
        engine.advance_live();
        let now = animation(&mut engine);
        if now.clip != previous {
            let (mannequin, _) = engine
                .world_mut()
                .query_ids::<(&Animation,)>()
                .next()
                .expect("the mannequin plays a clip");
            let on_end = engine
                .world()
                .resource::<AnimationEvents>()
                .is_some_and(|events| events.fired(mannequin, &previous, "end"));
            log.push((now.clip.clone(), now.from.clone(), on_end));
            previous = now.clip;
        }
    }
    let order: Vec<(&str, &str, bool)> = log
        .iter()
        .map(|(clip, from, on_end)| (clip.as_str(), from.as_str(), *on_end))
        .collect();
    assert_eq!(
        order,
        [
            ("run", "idle", false),
            ("jump", "run", false),
            ("run", "jump", true),
            ("spin", "", false),
            ("run", "spin", true),
            ("idle", "run", false),
        ],
        "switch log: {log:#?}"
    );
    let (mannequin, _) = engine
        .world_mut()
        .query_ids::<(&Animation,)>()
        .next()
        .expect("the mannequin plays a clip");
    assert_eq!(
        CYCLE_GRAPH.current(engine.world(), mannequin).unwrap(),
        Pose::Idle
    );
    let held = engine
        .world()
        .component::<AnimGraphState>(mannequin)
        .unwrap();
    assert_eq!(held.previous, "run_c");
}

#[test]
fn a_mid_cycle_snapshot_restore_reproduces_the_remaining_run() {
    const AT: usize = 200;
    let mut engine = load(graph_builder(), true);
    for _ in 0..AT {
        engine.advance_live();
    }
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut want = Vec::new();
    for _ in AT..TICKS {
        engine.advance_live();
        want.push(animation(&mut engine));
    }
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    let mut got = Vec::new();
    for _ in AT..TICKS {
        engine.advance_live();
        got.push(animation(&mut engine));
    }
    assert_eq!(got, want);
}
