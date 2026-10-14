//! Runtime proof for `PathFollower` (`Plan/character-controller.md` decision 10): the engine's
//! `path_follow` system poses a follower on gameplay time before any game system runs, in all three
//! modes, degrades an unloadable path to one `L1215`, and installs itself only into a world that
//! holds a follower (`L1216` for one spawned later). Everything goes through the public surface — a
//! scene document, `EngineBuilder`, the ordinary sim step — over a path file written to a temp root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::time::GameClock;
use ludion_ecs::{SemanticValidate, SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    Egress, EgressChannels, Engine, EngineBuilder, Name, PathFollower, PathMode, Transform,
};
use ludion_protocol::command::{SimStepParams, StreamSubscribeParams, SubscribeMode};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};

/// A straight, ten-metre path along +x: arc length equals x, so a pose reads as its phase.
const LIFT: &str = "\
format: 1
kind: path
name: lift
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 10.0
    - 0.0
    - 0.0
";

/// One metre per tick at the default tick rate.
const METRE_PER_TICK: f32 = 60.0;

/// What the `rider` system saw of the platform each tick.
#[derive(Default)]
struct Seen(Vec<(u64, f32, f32)>);

/// Reads the platform after `path_follow` has run: `(frame, translation.x, phase)`.
fn rider(world: &mut World, ctx: &SystemCtx) {
    let platform = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .expect("the scene has a follower");
    let x = world
        .get::<Transform>(platform)
        .unwrap()
        .unwrap()
        .translation[0];
    let phase = world.get::<PathFollower>(platform).unwrap().unwrap().phase;
    if world.resource::<Seen>().is_none() {
        world.insert_resource(Seen::default());
    }
    world
        .resource_mut::<Seen>()
        .unwrap()
        .0
        .push((ctx.frame.as_u64(), x, phase));
}

/// A game system that moves the follower's `phase` itself, at frame 2.
fn drive(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let platform = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .unwrap();
    world
        .get_mut::<PathFollower>(platform)
        .unwrap()
        .unwrap()
        .phase = 4.0;
}

/// Freezes the game for frames 3..=5 (the ticks after them do not advance gameplay time).
fn freeze(world: &mut World, ctx: &SystemCtx) {
    if (3..=5).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

/// Spawns a follower at frame 1, into a world that held none at load.
fn spawn_follower(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let id = world.spawn();
    world
        .insert(id, Name("late".to_string()))
        .expect("the entity is alive");
    world
        .insert(
            id,
            Transform {
                translation: [3.0, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            id,
            PathFollower {
                path: "lift.path.yaml".to_string(),
                speed: METRE_PER_TICK,
                ..PathFollower::default()
            },
        )
        .unwrap();
}

fn scene(path: &str, speed: f32, mode: &str, phase: f32) -> String {
    format!(
        "\
format: 1
entities:
  - name: platform
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      PathFollower:
        path: {path}
        speed: {speed}
        mode: {mode}
        phase: {phase}
"
    )
}

const EMPTY_SCENE: &str = "\
format: 1
entities:
  - name: camera
    components:
      Transform: {}
";

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-path-follow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    std::fs::write(dir.join("lift.path.yaml"), LIFT).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root)
}

fn advance(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("the sim steps");
}

/// The platform's `(x, phase)` after each of `ticks` ticks of a scene under `builder`.
fn run(
    tag: &str,
    builder: impl FnOnce(EngineBuilder) -> EngineBuilder,
    scene: &str,
    ticks: u64,
) -> Vec<(f32, f32)> {
    let root = fixture_root(tag);
    let mut engine = builder(game(&root)).build().expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    advance(&mut engine, ticks);
    engine
        .world()
        .resource::<Seen>()
        .expect("the rider ran")
        .0
        .iter()
        .map(|&(_, x, phase)| (x, phase))
        .collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn the_rider_sees_this_ticks_pose_not_last_ticks() {
    let seen = run(
        "rider",
        |b| b.system("rider", rider),
        &scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0),
        6,
    );
    assert_eq!(seen.len(), 6);
    for (n, &(x, phase)) in seen.iter().enumerate() {
        let tick = (n + 1) as f32;
        assert!(close(phase, tick), "tick {n}: phase {phase}");
        // The pose is the path sampled at *this* tick's phase, so the first tick already moved.
        assert!(close(x, phase), "tick {n}: pose {x} vs phase {phase}");
    }
    for pair in seen.windows(2) {
        assert!(close(pair[1].0 - pair[0].0, 1.0), "{pair:?}");
    }
}

#[test]
fn loop_wraps_ping_pong_reverses_and_once_stops() {
    let ticks = 4;
    let looped = run(
        "loop",
        |b| b.system("rider", rider),
        &scene("lift.path.yaml", METRE_PER_TICK, "loop", 8.5),
        ticks,
    );
    let xs: Vec<f32> = looped.iter().map(|s| s.0).collect();
    for (got, want) in xs.iter().zip([9.5, 0.5, 1.5, 2.5]) {
        assert!(close(*got, want), "loop {xs:?}");
    }

    let ping = run(
        "ping",
        |b| b.system("rider", rider),
        &scene("lift.path.yaml", METRE_PER_TICK, "ping_pong", 8.5),
        ticks,
    );
    let xs: Vec<f32> = ping.iter().map(|s| s.0).collect();
    for (got, want) in xs.iter().zip([9.5, 9.5, 8.5, 7.5]) {
        assert!(close(*got, want), "ping_pong {xs:?}");
    }
    assert!(
        close(ping[1].1, 10.5),
        "the phase keeps counting past L: {ping:?}"
    );

    let once = run(
        "once",
        |b| b.system("rider", rider),
        &scene("lift.path.yaml", METRE_PER_TICK, "once", 8.5),
        ticks,
    );
    let xs: Vec<f32> = once.iter().map(|s| s.0).collect();
    for (got, want) in xs.iter().zip([9.5, 10.0, 10.0, 10.0]) {
        assert!(close(*got, want), "once {xs:?}");
    }
}

#[test]
fn a_negative_speed_runs_backwards() {
    let seen = run(
        "reverse",
        |b| b.system("rider", rider),
        &scene("lift.path.yaml", -METRE_PER_TICK, "once", 2.5),
        4,
    );
    let xs: Vec<f32> = seen.iter().map(|s| s.0).collect();
    for (got, want) in xs.iter().zip([1.5, 0.5, 0.0, 0.0]) {
        assert!(close(*got, want), "{xs:?}");
    }
}

#[test]
fn a_gameplay_hold_freezes_phase_and_pose() {
    let seen = run(
        "hold",
        |b| b.system("freeze", freeze).system("rider", rider),
        &scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0),
        9,
    );
    // Frames 3..=5 are held, so frames 4, 5 and 6 do not advance gameplay time.
    assert!(seen[3].0 > seen[2].0, "frame 3 still advances: {seen:?}");
    for frame in 4..=6 {
        assert_eq!(seen[frame], seen[3], "frame {frame} is held: {seen:?}");
    }
    assert!(seen[7].0 > seen[6].0, "the platform resumes: {seen:?}");
}

#[test]
fn a_game_driven_phase_poses_on_the_next_tick() {
    let seen = run(
        "drive",
        |b| b.system("drive", drive).system("rider", rider),
        &scene("lift.path.yaml", 0.0, "once", 1.0),
        5,
    );
    assert!(
        close(seen[2].0, 1.0),
        "frame 2 still shows the old pose: {seen:?}"
    );
    assert!(
        close(seen[3].0, 4.0),
        "frame 3 poses the written phase: {seen:?}"
    );
    assert!(close(seen[4].0, 4.0), "speed 0 holds it: {seen:?}");
}

/// The `log`-channel lines a live session emits over `ticks` ticks.
fn log_lines(mut engine: Engine, ticks: u64) -> Vec<LogEvent> {
    engine
        .execute(Command::SimPause)
        .expect("the engine pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
    let mut lines = Vec::new();
    let mut wait = Duration::from_secs(5);
    while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
        if let Event::Log(log) = event.message.event {
            lines.push(log);
        }
        wait = Duration::from_millis(300);
    }
    shutdown.signal();
    let _ = handle.join();
    lines
}

fn x_of(engine: &Engine, name: &str) -> f32 {
    let world = engine.world();
    let id = world
        .entities()
        .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == name))
        .expect("the entity exists");
    world.get::<Transform>(id).unwrap().unwrap().translation[0]
}

#[test]
fn an_unloadable_path_latches_l1215_once_and_the_entity_stays_put() {
    let root = fixture_root("missing");
    let mut engine = game(&root).build().expect("the game builds");
    engine
        .load_inline_scene(&scene("nowhere.path.yaml", METRE_PER_TICK, "loop", 0.0))
        .expect("the scene loads");
    let lines = log_lines(engine, 20);
    let reported: Vec<&LogEvent> = lines.iter().filter(|l| code_is(l, "L1215")).collect();
    assert_eq!(
        reported.len(),
        1,
        "once per entity over 20 ticks: {lines:?}"
    );
    assert!(
        reported[0].message.contains("nowhere.path.yaml"),
        "{}",
        reported[0].message
    );
    let diagnostic = reported[0].diagnostic.as_ref().expect("structured");
    assert!(!diagnostic.message.contains("L1"), "{}", diagnostic.message);
    let related: Vec<&str> = diagnostic.related.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(related, ["L1101"], "the loader's read failure rides along");

    let root = fixture_root("invalid");
    let text = "format: 1\nkind: path\npoints:\n";
    std::fs::write(root.join("bad.path.yaml"), text).unwrap();
    let oracle = ludion_scene::check_path(text).expect_err("the document is invalid");
    let mut engine = game(&root).build().unwrap();
    engine
        .load_inline_scene(&scene("bad.path.yaml", METRE_PER_TICK, "loop", 0.0))
        .unwrap();
    let lines = log_lines(engine, 5);
    let reported: Vec<&LogEvent> = lines.iter().filter(|l| code_is(l, "L1215")).collect();
    assert_eq!(reported.len(), 1, "{lines:?}");
    let diagnostic = reported[0].diagnostic.as_ref().expect("structured");
    assert!(!diagnostic.message.contains("L1"), "{}", diagnostic.message);
    assert_eq!(
        diagnostic.related, oracle,
        "the validator's diagnostics, whole, as related"
    );

    let mut engine = game(&root).build().unwrap();
    engine
        .load_inline_scene(&scene("bad.path.yaml", METRE_PER_TICK, "loop", 0.0))
        .unwrap();
    advance(&mut engine, 5);
    assert_eq!(x_of(&engine, "platform"), 3.0, "the entity did not move");
}

#[test]
fn a_follower_spawned_at_runtime_latches_l1216_once_and_does_not_move() {
    let root = fixture_root("late");
    let mut engine = game(&root)
        .system("spawn_follower", spawn_follower)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(EMPTY_SCENE)
        .expect("the scene loads");
    advance(&mut engine, 5);
    assert_eq!(x_of(&engine, "late"), 3.0, "nothing moves it");

    let root = fixture_root("late-live");
    let mut engine = game(&root)
        .system("spawn_follower", spawn_follower)
        .build()
        .unwrap();
    engine.load_inline_scene(EMPTY_SCENE).unwrap();
    let lines = log_lines(engine, 20);
    let reported: Vec<&LogEvent> = lines.iter().filter(|l| code_is(l, "L1216")).collect();
    assert_eq!(reported.len(), 1, "once per entity: {lines:?}");
    assert!(
        reported[0].message.contains("PathFollower"),
        "{}",
        reported[0].message
    );
}

fn describe_systems(engine: &mut Engine) -> Vec<serde_json::Value> {
    match engine
        .execute(Command::EngineDescribe)
        .expect("describes")
        .body
    {
        ResponseBody::EngineDescription { description } => {
            description["systems"].as_array().unwrap().clone()
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn describe_lists_path_follow_first_only_when_a_follower_is_present() {
    let root = fixture_root("describe");
    let mut engine = game(&root)
        .system("rider", rider)
        .build()
        .expect("the game builds");

    engine.load_inline_scene(EMPTY_SCENE).unwrap();
    let systems = describe_systems(&mut engine);
    assert_eq!(
        systems.len(),
        1,
        "no follower, no engine system: {systems:?}"
    );
    assert_eq!(systems[0]["name"], "rider");
    assert_eq!(systems[0]["order"], 0);

    // An inert follower is enough to install it.
    engine
        .load_inline_scene(
            "format: 1\nentities:\n  - name: p\n    components:\n      Transform: {}\n      PathFollower: {}\n",
        )
        .unwrap();
    let systems = describe_systems(&mut engine);
    assert_eq!(systems.len(), 2, "{systems:?}");
    assert_eq!(systems[0]["name"], "path_follow");
    assert_eq!(systems[0]["order"], 0);
    assert!(systems[0]["registered_at"].is_null());
    assert_eq!(systems[0]["fn_path"], "ludion_engine::path_follow");
    assert_eq!(systems[1]["name"], "rider");
    assert_eq!(systems[1]["order"], 1);
    assert_eq!(x_of(&engine, "p"), 0.0);
    advance(&mut engine, 3);
}

#[test]
fn a_scene_without_a_follower_runs_no_path_system() {
    // The world hash of such a scene is whatever the game systems make it: the engine adds nothing.
    let root = fixture_root("nofollower");
    let mut engine = game(&root).build().unwrap();
    engine.load_inline_scene(EMPTY_SCENE).unwrap();
    assert!(describe_systems(&mut engine).is_empty());
    advance(&mut engine, 3);
    assert!(describe_systems(&mut engine).is_empty());
}

#[test]
fn the_mode_names_are_snake_case() {
    for (text, mode) in [
        ("loop", PathMode::Loop),
        ("ping_pong", PathMode::PingPong),
        ("once", PathMode::Once),
    ] {
        let follower: PathFollower =
            serde_json::from_value(serde_json::json!({ "mode": text })).unwrap();
        assert_eq!(follower.mode, mode);
    }
    let default: PathFollower = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(default, PathFollower::default());
    assert_eq!(default.speed, 1.0);
}

/// Slides the `carrier` along +y one metre a tick.
fn carry(world: &mut World, _ctx: &SystemCtx) {
    let carrier = world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == "carrier")
        })
        .expect("the scene has a carrier");
    world
        .get_mut::<Transform>(carrier)
        .unwrap()
        .unwrap()
        .translation[1] += 1.0;
}

#[test]
fn a_parented_follower_under_a_moving_parent_passes_the_query_drift_gate() {
    let scene = "\
format: 1
entities:
  - name: carrier
    components:
      Transform: {}
  - name: platform
    components:
      Parent:
        index: 0
        generation: 0
      Transform: {}
      PathFollower:
        path: lift.path.yaml
        speed: 1.0
        mode: loop
        phase: 0.0
";
    let root = fixture_root("drift");
    let mut engine = game(&root)
        .system("carry", carry)
        .query::<(&mut Transform,)>()
        .query::<(&Name,)>()
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    let report = ludion_engine::check_query_drift_with(&mut engine, |e| {
        e.execute(Command::SimStep(SimStepParams {
            ticks: 2,
            hash: false,
        }))
        .map(|_| ())
    })
    .unwrap();
    assert!(report.is_clean(), "{report}");
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}

/// A game system that corrupts the follower's `speed` after the first tick, as Rust code can.
fn poison_speed(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    let platform = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .unwrap();
    world
        .get_mut::<PathFollower>(platform)
        .unwrap()
        .unwrap()
        .speed = f32::NAN;
}

#[test]
fn a_speed_or_phase_past_f32_range_fails_validation_with_l1269() {
    for field in ["speed", "phase"] {
        let value = serde_json::json!({ field: 1e39 });
        let found = PathFollower::validate_semantics(&value);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code.as_str(), "L1269");
        assert_eq!(found[0].path.as_deref(), Some(format!("/{field}").as_str()));
    }
    assert!(PathFollower::validate_semantics(&serde_json::json!({ "speed": -2.5 })).is_empty());
}

#[test]
fn a_non_finite_speed_built_in_rust_holds_the_follower_and_reports_l1269_once() {
    let root = fixture_root("nan");
    let mut engine = game(&root)
        .system("poison_speed", poison_speed)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(&scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0))
        .expect("the scene loads");
    advance(&mut engine, 1);
    let phase_of = |engine: &Engine| {
        let world = engine.world();
        let id = world
            .entities()
            .find(|&id| world.has::<PathFollower>(id))
            .unwrap();
        world.get::<PathFollower>(id).unwrap().unwrap().phase
    };
    let (x, phase) = (x_of(&engine, "platform"), phase_of(&engine));
    let lines = log_lines(engine, 5);
    assert_eq!(
        lines.iter().filter(|l| code_is(l, "L1269")).count(),
        1,
        "once per entity over two or more ticks: {lines:?}"
    );

    let mut engine = game(&root)
        .system("poison_speed", poison_speed)
        .build()
        .unwrap();
    engine
        .load_inline_scene(&scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0))
        .unwrap();
    advance(&mut engine, 6);
    assert_eq!(x_of(&engine, "platform").to_bits(), x.to_bits());
    assert_eq!(phase_of(&engine).to_bits(), phase.to_bits());
}
