//! Runtime proof for patrols (`Plan/nav.md` decision 13): `patrol_update` walks a guard through a
//! `kind: patrol` document as nav legs, counts stops on gameplay time, hands the guard to the game
//! while paused, skips an unreachable stop, degrades a bad setup to one diagnostic, and resumes after a
//! restore. Everything goes through the public surface over documents written to a temp content root.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, NavGoal, PatrolFollower, PatrolPhase, PatrolState, Transform,
    TransformHierarchy, nav_steer, nav_update, patrol_motion, patrol_update,
};
use ludion_protocol::Command;
use ludion_protocol::command::EngineRestoreParams;
use ludion_spatial::spatial3d;

use crate::nav::{agent, block, config, ground, mover, scene, seq, snapshot_of};

const A: [f32; 3] = [4.0, 0.0, 4.0];
const B: [f32; 3] = [10.0, 0.0, 4.0];
const C: [f32; 3] = [10.0, 0.0, 10.0];

/// One stop of a patrol document: its position and any extra point keys (already indented 4 spaces).
type Stop<'a> = ([f32; 3], &'a str);

fn patrol_doc(mode: &str, stops: &[Stop]) -> String {
    let mut out = format!("format: 1\nkind: patrol\nmode: {mode}\npoints:\n");
    for (at, extra) in stops {
        out += "  - at:\n";
        out += &seq(*at, 6);
        out += extra;
    }
    out
}

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-patrol-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    dir
}

fn write(root: &Path, name: &str, text: &str) {
    std::fs::write(root.join(name), text).unwrap();
}

/// The nav-and-patrol schedule: `nav_update` → `patrol_update` → `nav_steer` → the mover.
fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .nav(config())
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("nav_update", nav_update)
        .system("patrol_update", patrol_update)
        .system("nav_steer", nav_steer)
        .system("mover", mover)
}

/// What `log` saw of the first guard each tick, after every other system ran.
#[derive(Default)]
struct Log(Vec<PatrolState>);

fn log(world: &mut World, _ctx: &SystemCtx) {
    let state = world
        .query_ids::<(&PatrolState,)>()
        .next()
        .map(|(_, (state,))| *state);
    if world.resource::<Log>().is_none() {
        world.insert_resource(Log::default());
    }
    if let Some(state) = state {
        world.resource_mut::<Log>().unwrap().0.push(state);
    }
}

fn guard(n: usize, at: [f32; 3], follower: &str) -> String {
    let fields = "        neighbor_layers: 0\n        speed: 6.0\n";
    agent(n, at, fields) + &format!("      PatrolFollower:\n{follower}")
}

fn follower(route: &str) -> String {
    format!("        route: {route}\n")
}

fn engine_with(builder: EngineBuilder, yaml: &str) -> Engine {
    let mut engine = builder.system("log", log).build().expect("boots");
    engine.load_inline_scene(yaml).expect("the scene loads");
    engine
}

fn samples(engine: &mut Engine, ticks: usize) -> Vec<PatrolState> {
    for _ in 0..ticks {
        engine.advance_live();
    }
    engine
        .world()
        .resource::<Log>()
        .map_or_else(Vec::new, |log| log.0.clone())
}

/// The point of each tick that began a stop.
fn arrivals(samples: &[PatrolState]) -> Vec<u32> {
    samples
        .iter()
        .filter(|s| s.arrived)
        .map(|s| s.point)
        .collect()
}

fn run(root: &Path, route: &str, start: [f32; 3], ticks: usize) -> Vec<PatrolState> {
    let yaml = scene(&[ground()]) + &guard(0, start, &follower(route));
    samples(&mut engine_with(game(root), &yaml), ticks)
}

#[test]
fn a_loop_visits_its_stops_in_order_and_repeats_bit_for_bit() {
    let root = fixture_root("loop");
    let wait = "    wait: 0.2\n";
    write(
        &root,
        "loop.patrol.yaml",
        &patrol_doc("loop", &[(A, wait), (B, wait), (C, wait)]),
    );
    let first = run(&root, "loop.patrol.yaml", A, 700);
    let order = arrivals(&first);
    assert!(order.len() >= 6, "{order:?}");
    assert_eq!(order[..6], [0, 1, 2, 0, 1, 2]);
    assert_eq!(
        first,
        run(&root, "loop.patrol.yaml", A, 700),
        "the run is repeatable"
    );
    // Facing +x (A -> B) is a yaw of -90 degrees with -Z forward.
    let at_b = first.iter().find(|s| s.arrived && s.point == 1).unwrap();
    assert!((at_b.look_deg.unwrap() + 90.0).abs() < 1e-3, "{at_b:?}");
}

#[test]
fn ping_pong_reverses_and_once_ends_done() {
    let root = fixture_root("modes");
    let stops: [Stop; 3] = [(A, ""), (B, ""), (C, "")];
    write(&root, "pp.patrol.yaml", &patrol_doc("ping_pong", &stops));
    write(&root, "once.patrol.yaml", &patrol_doc("once", &stops));
    let pp = run(&root, "pp.patrol.yaml", A, 700);
    assert_eq!(arrivals(&pp)[..7], [0, 1, 2, 1, 0, 1, 2]);
    let once = run(&root, "once.patrol.yaml", A, 400);
    assert_eq!(arrivals(&once), [0, 1, 2]);
    let last = once.last().unwrap();
    assert_eq!(
        (last.phase, last.point, last.look_deg),
        (PatrolPhase::Done, 2, None)
    );
}

#[test]
fn a_wait_holds_about_one_second_and_looks_turn_the_facing() {
    let root = fixture_root("wait");
    write(
        &root,
        "wait.patrol.yaml",
        &patrol_doc("loop", &[(A, "    wait: 1.0\n"), (B, "")]),
    );
    let waited = run(&root, "wait.patrol.yaml", A, 200);
    let first = waited
        .iter()
        .position(|s| s.phase == PatrolPhase::Stopped)
        .unwrap();
    let held = waited[first..]
        .iter()
        .take_while(|s| s.phase == PatrolPhase::Stopped)
        .count();
    assert!(
        (59..=62).contains(&held),
        "a 1 s wait at 60 Hz held {held} ticks"
    );

    let looks = "    face_deg: 10.0\n    wait: 1.5\n    look:\n      - yaw_deg: 90.0\n        hold: 0.5\n      - yaw_deg: -90.0\n        hold: 0.5\n";
    write(
        &root,
        "post.patrol.yaml",
        &patrol_doc("loop", &[(A, looks)]),
    );
    let post = run(&root, "post.patrol.yaml", A, 120);
    let mut facings: Vec<f32> = Vec::new();
    for look in post.iter().filter_map(|s| s.look_deg) {
        if facings.last() != Some(&look) {
            facings.push(look);
        }
    }
    assert_eq!(facings[..3], [100.0, -80.0, 10.0], "{facings:?}");
}

/// Holds gameplay time for frames 30 through 50.
fn freeze(world: &mut World, ctx: &SystemCtx) {
    if (30..=50).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

#[test]
fn a_gameplay_hold_freezes_the_stop() {
    let root = fixture_root("hold");
    write(
        &root,
        "post.patrol.yaml",
        &patrol_doc("loop", &[(A, "    wait: 2.0\n")]),
    );
    let yaml = scene(&[ground()]) + &guard(0, A, &follower("post.patrol.yaml"));
    let held = samples(
        &mut engine_with(game(&root).system("freeze", freeze), &yaml),
        90,
    );
    let frozen = |samples: &[PatrolState]| {
        samples
            .windows(2)
            .filter(|w| w[0].phase == PatrolPhase::Stopped && w[0].stop_left == w[1].stop_left)
            .count()
    };
    assert!(
        frozen(&held) >= 15,
        "the hold froze stop_left: {}",
        frozen(&held)
    );
    assert_eq!(frozen(&run(&root, "post.patrol.yaml", A, 90)), 0);
}

/// Un-pauses the guard at frame 12.
fn unpause(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 12 {
        let id = world.query_ids::<(&PatrolFollower,)>().next().unwrap().0;
        world.get_mut::<PatrolFollower>(id).unwrap().unwrap().paused = false;
    }
}

#[test]
fn pausing_leaves_the_goal_alone_and_resume_picks_the_stop() {
    let root = fixture_root("pause");
    write(
        &root,
        "loop.patrol.yaml",
        &patrol_doc("loop", &[(A, ""), (B, ""), (C, "")]),
    );
    let goal = [20.0, 0.0, 20.0];
    for (resume, expected) in [("nearest", 2), ("next", 1), ("last", 0)] {
        let extra = format!(
            "{}        paused: true\n        resume: {resume}\n      NavGoal:\n        at:\n{}",
            follower("loop.patrol.yaml"),
            seq(goal, 10)
        );
        let yaml = scene(&[ground()])
            + &agent(
                0,
                [9.5, 0.0, 9.0],
                "        neighbor_layers: 0\n        speed: 6.0\n",
            )
            + &format!("      PatrolFollower:\n{extra}");
        let mut engine = engine_with(game(&root).system("unpause", unpause), &yaml);
        for _ in 0..10 {
            engine.advance_live();
        }
        let world = engine.world();
        let id = world
            .entities()
            .find(|&id| world.has::<NavGoal>(id))
            .unwrap();
        assert_eq!(
            world.get::<NavGoal>(id).unwrap().unwrap().at,
            goal,
            "{resume}"
        );
        assert_eq!(
            world.get::<PatrolState>(id).unwrap().unwrap().phase,
            PatrolPhase::Paused
        );
        for _ in 0..4 {
            engine.advance_live();
        }
        let world = engine.world();
        let state = world.get::<PatrolState>(id).unwrap().unwrap();
        assert_eq!(state.point, expected, "resume: {resume}");
    }
}

fn reported(engine: &Engine, which: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(which)
        .map(str::to_string)
        .collect()
}

#[test]
fn an_unreachable_stop_is_skipped_once_and_the_patrol_goes_on() {
    let root = fixture_root("unreachable");
    let (a, ledge, b) = ([4.0, 0.0, 12.0], [12.0, 1.0, 12.0], [4.0, 0.0, 20.0]);
    write(
        &root,
        "ledge.patrol.yaml",
        &patrol_doc("loop", &[(a, ""), (ledge, ""), (b, "")]),
    );
    let blocks = [ground(), block([12.0, 0.5, 12.0], [2.0, 0.5, 12.0])];
    let yaml = scene(&blocks) + &guard(0, a, &follower("ledge.patrol.yaml"));
    let mut engine = engine_with(game(&root), &yaml);
    let seen = samples(&mut engine, 600);
    let order = arrivals(&seen);
    assert!(order.len() >= 4, "{order:?}");
    assert!(
        order.iter().all(|&p| p != 1),
        "never stops on the ledge: {order:?}"
    );
    assert_eq!(order[..4], [0, 2, 0, 2]);
    let keys = reported(&engine, code::PATROL_POINT_UNREACHABLE);
    assert_eq!(keys.len(), 1, "once per entity and point: {keys:?}");
    assert!(keys[0].ends_with("/points/1"), "{keys:?}");
}

#[test]
fn a_bad_setup_reports_one_diagnostic_and_leaves_the_guard_alone() {
    let root = fixture_root("bad");
    let mut engine = engine_with(
        game(&root),
        &(scene(&[ground()]) + &guard(0, A, &follower("nowhere.patrol.yaml"))),
    );
    let seen = samples(&mut engine, 20);
    assert!(
        seen.is_empty(),
        "no PatrolState for a route that did not load"
    );
    let keys = reported(&engine, code::PATROL_ROUTE_UNLOADED);
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(keys[0].ends_with("/PatrolFollower/route"), "{keys:?}");

    write(&root, "post.patrol.yaml", &patrol_doc("loop", &[(A, "")]));
    let no_agent = format!(
        "  - name: g\n    components:\n      Transform:\n        translation:\n{}      PatrolFollower:\n{}",
        seq(A, 10),
        follower("post.patrol.yaml")
    );
    let mut engine = engine_with(game(&root), &(scene(&[ground()]) + &no_agent));
    assert!(samples(&mut engine, 20).is_empty());
    assert_eq!(reported(&engine, code::PATROL_WITHOUT_AGENT).len(), 1);
}

#[test]
fn a_scene_without_a_follower_gets_no_patrol_state() {
    let root = fixture_root("none");
    let plain = agent(0, A, "        neighbor_layers: 0\n        speed: 6.0\n");
    let mut engine = engine_with(game(&root), &(scene(&[ground()]) + &plain));
    assert!(samples(&mut engine, 20).is_empty());
    let world = engine.world();
    assert!(
        world
            .entities()
            .all(|id| patrol_motion(world, id).is_none())
    );
}

#[test]
fn the_motion_reads_the_stops_tag() {
    let root = fixture_root("tag");
    write(
        &root,
        "post.patrol.yaml",
        &patrol_doc("loop", &[(A, "    wait: 5.0\n    tag: door\n")]),
    );
    let yaml = scene(&[ground()]) + &guard(0, A, &follower("post.patrol.yaml"));
    let mut engine = engine_with(game(&root), &yaml);
    samples(&mut engine, 30);
    let world = engine.world();
    let id = world
        .entities()
        .find(|&id| world.has::<PatrolFollower>(id))
        .unwrap();
    let motion = patrol_motion(world, id).expect("a patrolling guard");
    assert_eq!((motion.point, motion.phase), (0, PatrolPhase::Stopped));
    assert_eq!(motion.tag.as_deref(), Some("door"));
}

fn restore_engine(root: &Path) -> Engine {
    let yaml = scene(&[ground()]) + &guard(0, A, &follower("loop.patrol.yaml"));
    engine_with(game(root), &yaml)
}

#[test]
fn a_restore_mid_wait_rejoins_the_uninterrupted_trace() {
    let root = fixture_root("restore");
    write(
        &root,
        "loop.patrol.yaml",
        &patrol_doc("loop", &[(A, "    wait: 1.0\n"), (B, "    wait: 0.5\n")]),
    );
    let whole = crate::nav::trace(&mut restore_engine(&root), 250);

    let mut first = restore_engine(&root);
    crate::nav::trace(&mut first, 30);
    let mid = first
        .world()
        .resource::<Log>()
        .unwrap()
        .0
        .last()
        .copied()
        .unwrap();
    assert_eq!(
        mid.phase,
        PatrolPhase::Stopped,
        "the snapshot lands mid-wait: {mid:?}"
    );
    let snapshot = snapshot_of(&mut first);

    let mut resumed = restore_engine(&root);
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let tail = crate::nav::trace(&mut resumed, 220);
    assert_eq!(
        tail,
        whole[30..],
        "the restored patrol matches from tick 31"
    );
}
