//! `common::cycle_builder` (the six-state `AnimGraph`, `Plan/animation-layers.md` decisions 50-53)
//! against the hand-written table it replaced, kept here as an oracle: both load the gallery's
//! `character-cycle` scene and write the identical `Animation` on every tick, and so the identical
//! switch log. The table ignores the scene's `AnimGraphState`; the graph is what the hash sees.

mod common;

use std::path::PathBuf;

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AnimGraphState, Animation, AnimationEvents, Engine, EngineBuilder, Mesh, PrefabInstance,
    Sprite, animation_events, crossfade,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

use common::{CYCLE_GRAPH, IDLE_TICKS, Pose, RUN_IN_FADE, RUN_TICKS};

const TICKS: usize = 420;

/// What ends a clip's turn in the oracle table.
enum Leave {
    /// This many ticks after the clip started.
    After(u64),
    /// The clip's implicit `end` event (a one-shot finishing).
    OnEnd,
}

/// `(clip, entered from, leaves on, next clip, fade seconds)`; a `None` origin matches any.
/// Every entry into `run` is a crossfade, and `crossfade` keeps `Animation.from` after the fade
/// ends, so `run` reads its predecessor there and the table needs no state of its own.
const CYCLE: [(&str, Option<&str>, Leave, &str, f32); 6] = [
    ("idle", None, Leave::After(IDLE_TICKS), "run", RUN_IN_FADE),
    ("run", Some("idle"), Leave::After(RUN_TICKS), "jump", 0.1),
    ("run", Some("jump"), Leave::After(RUN_TICKS), "spin", 0.0),
    ("run", Some("spin"), Leave::After(RUN_TICKS), "idle", 0.3),
    ("jump", None, Leave::OnEnd, "run", 0.15),
    ("spin", None, Leave::OnEnd, "run", 0.15),
];

/// Advances every `Animation` along [`CYCLE`], writing only on a switch.
fn table_cycle(world: &mut World, ctx: &SystemCtx) {
    let events = world
        .resource::<AnimationEvents>()
        .cloned()
        .unwrap_or_default();
    for (entity, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        let elapsed = ctx
            .game_frame
            .as_u64()
            .saturating_sub(animation.started.as_u64());
        let ended = events.fired(entity, &animation.clip, "end");
        let next = CYCLE
            .iter()
            .find(|(clip, from, _, _, _)| {
                *clip == animation.clip && from.is_none_or(|from| from == animation.from)
            })
            .and_then(|(_, _, leave, next, fade)| {
                let due = match leave {
                    Leave::After(ticks) => elapsed >= *ticks,
                    Leave::OnEnd => ended,
                };
                due.then_some((*next, *fade))
            });
        if let Some((clip, fade)) = next {
            crossfade(animation, clip, fade, ctx.game_frame);
        }
    }
}

/// `common::cycle_builder` with `table_cycle` in place of the graph.
fn table_builder() -> EngineBuilder {
    Engine::builder()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("table_cycle", table_cycle)
        .query::<(&mut Animation,)>()
}

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn load(builder: EngineBuilder) -> Engine {
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
    let mut table = load(table_builder());
    let mut graph = load(common::cycle_builder());
    for tick in 0..TICKS {
        table.advance_live();
        graph.advance_live();
        assert_eq!(animation(&mut table), animation(&mut graph), "tick {tick}");
    }
}

#[test]
fn the_graph_switches_in_the_cycle_order_and_ends_in_idle() {
    let mut engine = load(common::cycle_builder());
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
    let mut engine = load(common::cycle_builder());
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
