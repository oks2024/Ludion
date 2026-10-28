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
    Egress, EgressChannels, Engine, EngineBuilder, FollowerMotion, Name, PathFollower, PathMode,
    PathOrient, Transform, path_follower_motion,
};
use ludion_physics::{ExitCause, PhysicsWorld, SensorEvent, SensorPhase};
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

// --- Motion readout and teleports (`path_follower_motion`) ---------------------------------------

/// A four-metre square, closed: its wrap is seamless.
const RING: &str = "\
format: 1
kind: path
name: ring
interpolation: linear
closed: true
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 4.0
  - - 0.0
    - 0.0
    - 4.0
";

/// A ten-metre vertical run, open.
const RISE: &str = "\
format: 1
kind: path
name: rise
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 0.0
    - 10.0
    - 0.0
";

type Tick = (u64, Option<FollowerMotion>, [f32; 3]);

/// `(frame, the follower's motion, its translation)` as a game system saw it each tick.
#[derive(Default)]
struct Moves(Vec<Tick>);

fn record_motion(world: &mut World, ctx: &SystemCtx) {
    let follower = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .expect("the scene has a follower");
    let motion = path_follower_motion(world, follower);
    let translation = world
        .get::<Transform>(follower)
        .unwrap()
        .unwrap()
        .translation;
    if world.resource::<Moves>().is_none() {
        world.insert_resource(Moves::default());
    }
    world
        .resource_mut::<Moves>()
        .unwrap()
        .0
        .push((ctx.frame.as_u64(), motion, translation));
}

fn extra_paths(tag: &str) -> PathBuf {
    let root = fixture_root(tag);
    std::fs::write(root.join("ring.path.yaml"), RING).unwrap();
    std::fs::write(root.join("rise.path.yaml"), RISE).unwrap();
    root
}

/// The follower's recorded ticks of `scene` over `ticks`, under `builder` plus `record_motion`.
fn moves(
    tag: &str,
    builder: impl FnOnce(EngineBuilder) -> EngineBuilder,
    scene: &str,
    ticks: u64,
) -> Vec<Tick> {
    let root = extra_paths(tag);
    let mut engine = builder(game(&root).system("record_motion", record_motion))
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    advance(&mut engine, ticks);
    engine.world().resource::<Moves>().expect("ran").0.clone()
}

#[test]
fn motion_delta_equals_the_translation_written() {
    let ticks = moves(
        "delta",
        |b| b,
        &scene("lift.path.yaml", METRE_PER_TICK, "once", 0.0),
        6,
    );
    let mut old = [3.0, 0.0, 0.0];
    for (frame, motion, translation) in &ticks {
        let motion = motion.expect("a posed follower reports its motion");
        let expected = [
            translation[0] - old[0],
            translation[1] - old[1],
            translation[2] - old[2],
        ];
        assert_eq!(
            motion.delta.map(f32::to_bits),
            expected.map(f32::to_bits),
            "frame {frame}"
        );
        old = *translation;
    }
    // The authored (3, 0, 0) is off the path: the first tick snaps to the start, honestly a jump.
    assert!(ticks[0].1.unwrap().teleported);
    let travelling = ticks[2].1.unwrap();
    assert!(!travelling.teleported, "{travelling:?}");
    assert!(
        (travelling.velocity[0] - METRE_PER_TICK).abs() < 0.01,
        "{travelling:?}"
    );
    assert!((travelling.s - 3.0).abs() < 1e-3, "{travelling:?}");
}

#[test]
fn ping_pong_direction_flips_at_the_end() {
    let ticks = moves(
        "direction",
        |b| b,
        &scene("lift.path.yaml", METRE_PER_TICK, "ping_pong", 0.0),
        25,
    );
    let direction = |tick: usize| ticks[tick - 1].1.unwrap().direction;
    assert_eq!(direction(5), 1.0);
    assert_eq!(direction(15), -1.0, "the return leg runs towards the start");
    assert_eq!(direction(25), 1.0);
    let backwards = moves(
        "direction-back",
        |b| b,
        &scene("lift.path.yaml", -METRE_PER_TICK, "once", 8.0),
        2,
    );
    assert_eq!(backwards[0].1.unwrap().direction, -1.0);
}

/// Writes the follower's `phase` a quarter metre down on frames 1..=4, then leaves it alone.
fn slide_down(world: &mut World, ctx: &SystemCtx) {
    if !(1..=4).contains(&ctx.frame.as_u64()) {
        return;
    }
    let follower = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .unwrap();
    world
        .get_mut::<PathFollower>(follower)
        .unwrap()
        .unwrap()
        .phase -= 0.25;
}

#[test]
fn a_game_driven_phase_sets_direction_from_its_change() {
    let root = extra_paths("driven-direction");
    let mut engine = game(&root)
        .system("record_motion", record_motion)
        .system("slide_down", slide_down)
        .build()
        .expect("the game builds");
    let scene = scene("lift.path.yaml", 0.0, "once", 8.0).replace(
        "        mode: once\n",
        "        mode: once\n        orient: yaw\n",
    );
    engine.load_inline_scene(&scene).expect("the scene loads");
    advance(&mut engine, 8);
    let ticks = engine.world().resource::<Moves>().expect("ran").0.clone();
    let direction = |tick: usize| ticks[tick].1.unwrap().direction;
    assert_eq!(direction(0), 1.0, "no previous tick: the default");
    assert!((1..5).any(|tick| direction(tick) == -1.0), "{ticks:?}");
    assert_eq!(direction(ticks.len() - 1), -1.0, "held: keeps the last");
    let follower = engine
        .world()
        .entities()
        .find(|&id| engine.world().has::<PathFollower>(id))
        .unwrap();
    let yaw = engine
        .world()
        .get::<Transform>(follower)
        .unwrap()
        .unwrap()
        .rotation[1];
    assert!(close(yaw, 90.0), "faces the start of the path: {yaw}");
}

#[test]
fn a_held_tick_has_zero_velocity() {
    let ticks = moves(
        "held",
        |b| b.system("freeze", freeze),
        &scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0),
        9,
    );
    let held: Vec<_> = ticks
        .iter()
        .skip(1)
        .filter(|(_, motion, _)| motion.unwrap().delta == [0.0; 3])
        .collect();
    assert!(!held.is_empty(), "the freeze held some ticks: {ticks:?}");
    for (frame, motion, _) in held {
        assert_eq!(motion.unwrap().velocity, [0.0; 3], "frame {frame}");
    }
}

/// A kinematic lift on the vertical `rise` path running backwards over the top of its path, with a
/// crate resting on it: the wrap carries the platform ten metres up through the crate in one tick.
const LIFT_AND_CRATE: &str = "format: 1
physics:
  space: free
entities:
  - name: lift
    components:
      Transform: {}
      PathFollower:
        path: rise.path.yaml
        speed: -0.6
        mode: loop
        phase: 0.5
      Collider:
        shape:
          box:
            half_extents:
              - 3.0
              - 0.25
              - 3.0
        body: kinematic
  - name: crate
    components:
      Transform:
        translation:
          - 0.0
          - 1.3
          - 0.0
      Velocity: {}
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          rigid:
            mass: 1.0
            sleep_after_ticks: 100000
";

#[derive(Default)]
struct CrateSpeeds(Vec<(u64, f32)>);

fn watch_crate(world: &mut World, ctx: &SystemCtx) {
    let id = world
        .entities()
        .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == "crate"))
        .unwrap();
    let v = world
        .get::<ludion_spatial::Velocity>(id)
        .unwrap()
        .unwrap()
        .linear;
    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if world.resource::<CrateSpeeds>().is_none() {
        world.insert_resource(CrateSpeeds::default());
    }
    world
        .resource_mut::<CrateSpeeds>()
        .unwrap()
        .0
        .push((ctx.frame.as_u64(), speed));
}

#[test]
fn an_open_loop_wrap_is_a_teleport_and_launches_no_rider() {
    let ticks = moves(
        "wrap",
        |b| b,
        &scene("lift.path.yaml", METRE_PER_TICK, "loop", 0.0),
        15,
    );
    let wraps: Vec<_> = ticks
        .iter()
        .skip(1)
        .filter(|(_, motion, _)| motion.unwrap().teleported)
        .collect();
    assert_eq!(wraps.len(), 1, "one wrap in fifteen metres: {wraps:?}");
    assert_eq!(wraps[0].1.unwrap().velocity, [0.0; 3]);

    let root = extra_paths("wrap-rider");
    let mut engine = game(&root)
        .system("record_motion", record_motion)
        .system("watch_crate", watch_crate)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(LIFT_AND_CRATE)
        .expect("the scene loads");
    advance(&mut engine, 70);
    let world = engine.world();
    let wrap = world
        .resource::<Moves>()
        .unwrap()
        .0
        .iter()
        .skip(1)
        .find(|(_, motion, _)| motion.unwrap().teleported)
        .expect("the lift wraps")
        .0;
    let peak = world
        .resource::<CrateSpeeds>()
        .unwrap()
        .0
        .iter()
        .filter(|(frame, _)| (wrap + 1..=wrap + 4).contains(frame))
        .map(|(_, speed)| *speed)
        .fold(0.0_f32, f32::max);
    assert!(
        peak < 2.0,
        "the crate is left behind, not launched: {peak} m/s"
    );
}

#[test]
fn a_closed_loop_wrap_is_not_a_teleport() {
    let ticks = moves(
        "ring",
        |b| b,
        &scene("ring.path.yaml", METRE_PER_TICK, "loop", 0.0),
        30,
    );
    // Tick 1 poses the off-path authored transform; from then on a 16 m ring wraps seamlessly.
    for (frame, motion, _) in ticks.iter().skip(1) {
        assert!(!motion.unwrap().teleported, "frame {frame}");
    }
}

#[test]
fn a_game_written_phase_jump_is_a_teleport() {
    let ticks = moves(
        "jump",
        |b| b.system("drive", drive),
        &scene("lift.path.yaml", 0.0, "once", 1.0),
        6,
    );
    let jumps: Vec<_> = ticks
        .iter()
        .skip(1)
        .filter(|(_, motion, _)| motion.unwrap().teleported)
        .map(|(frame, ..)| *frame)
        .collect();
    assert_eq!(jumps, [3], "only the tick that poses the written phase");
}

#[test]
fn a_world_without_followers_reports_no_motion() {
    let root = extra_paths("none");
    let mut engine = game(&root).build().expect("the game builds");
    engine.load_inline_scene(EMPTY_SCENE).expect("loads");
    advance(&mut engine, 3);
    let world = engine.world();
    assert!(
        world
            .entities()
            .all(|id| path_follower_motion(world, id).is_none())
    );
}

/// A one-follower scene on `path` with `orient` (`None`: the key is omitted) and an authored rotation.
fn oriented_scene(path: &str, speed: f32, mode: &str, phase: f32, orient: Option<&str>) -> String {
    let orient = orient.map_or(String::new(), |o| format!("        orient: {o}\n"));
    format!(
        "\
format: 1
entities:
  - name: platform
    components:
      Transform:
        rotation:
          - 0.0
          - 45.0
          - 0.0
      PathFollower:
        path: {path}
        speed: {speed}
        mode: {mode}
        phase: {phase}
{orient}"
    )
}

/// An engine on `scene` over the `LIFT` root plus `extra` `(file, text)` paths, `ticks` in.
fn oriented_engine(tag: &str, extra: &[(&str, &str)], scene: &str, ticks: u64) -> Engine {
    let root = fixture_root(tag);
    for (file, text) in extra {
        std::fs::write(root.join(file), text).unwrap();
    }
    let mut engine = game(&root).build().expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    advance(&mut engine, ticks);
    engine
}

fn rotation_of(engine: &Engine, name: &str) -> [f32; 3] {
    let world = engine.world();
    let id = world
        .entities()
        .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == name))
        .expect("the entity exists");
    world.get::<Transform>(id).unwrap().unwrap().rotation
}

fn near(got: [f32; 3], want: [f32; 3]) -> bool {
    got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-4)
}

#[test]
fn orient_yaw_faces_travel_and_flips_on_ping_pong() {
    let out = oriented_engine(
        "yaw-out",
        &[],
        &oriented_scene(
            "lift.path.yaml",
            METRE_PER_TICK,
            "ping_pong",
            0.0,
            Some("yaw"),
        ),
        2,
    );
    let rotation = rotation_of(&out, "platform");
    assert!(near(rotation, [0.0, -90.0, 0.0]), "{rotation:?}");
    let back = oriented_engine(
        "yaw-back",
        &[],
        &oriented_scene(
            "lift.path.yaml",
            METRE_PER_TICK,
            "ping_pong",
            0.0,
            Some("yaw"),
        ),
        12,
    );
    let rotation = rotation_of(&back, "platform");
    assert!(near(rotation, [0.0, 90.0, 0.0]), "{rotation:?}");
    let reversed = oriented_engine(
        "yaw-neg",
        &[],
        &oriented_scene("lift.path.yaml", -METRE_PER_TICK, "loop", 5.0, Some("yaw")),
        2,
    );
    let rotation = rotation_of(&reversed, "platform");
    assert!(near(rotation, [0.0, 90.0, 0.0]), "{rotation:?}");
}

#[test]
fn orient_yaw_keeps_the_last_yaw_on_a_vertical_stretch() {
    const CLIMB: &str = "format: 1
kind: path
name: climb
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 0.0
    - 0.0
    - -4.0
  - - 0.0
    - 4.0
    - -4.0
";
    let scene = oriented_scene("climb.path.yaml", METRE_PER_TICK, "once", 0.0, Some("yaw"));
    let extra = [("climb.path.yaml", CLIMB)];
    let flat = oriented_engine("climb-flat", &extra, &scene, 3);
    assert_eq!(rotation_of(&flat, "platform"), [0.0, 0.0, 0.0]);
    let rising = oriented_engine("climb-up", &extra, &scene, 7);
    let world = rising.world();
    let id = world
        .entities()
        .find(|&id| world.has::<PathFollower>(id))
        .unwrap();
    assert!(world.get::<Transform>(id).unwrap().unwrap().translation[1] > 1.0);
    assert_eq!(
        rotation_of(&rising, "platform").map(f32::to_bits),
        rotation_of(&flat, "platform").map(f32::to_bits)
    );
}

#[test]
fn orient_frame_faces_travel() {
    const SLOPE: &str = "format: 1
kind: path
name: slope
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 10.0
    - 10.0
    - 0.0
";
    let scene = oriented_scene(
        "slope.path.yaml",
        METRE_PER_TICK,
        "ping_pong",
        0.0,
        Some("frame"),
    );
    let extra = [("slope.path.yaml", SLOPE)];
    let t = std::f32::consts::FRAC_1_SQRT_2;
    for (ticks, sign) in [(3, 1.0), (16, -1.0)] {
        let engine = oriented_engine("frame", &extra, &scene, ticks);
        let quat = ludion_core::math::euler_xyz_degrees_to_quat(rotation_of(&engine, "platform"));
        let forward = ludion_core::math::quat_rotate(quat, [0.0, 0.0, -1.0]);
        let dot = sign * (forward[0] * t + forward[1] * t);
        assert!(dot > 1.0 - 1e-5, "{ticks} ticks: {forward:?}");
        let right = ludion_core::math::quat_rotate(quat, [1.0, 0.0, 0.0]);
        assert!(right[1].abs() < 1e-5, "no roll: {right:?}");
    }
}

#[test]
fn orient_none_leaves_rotation_and_hash_unchanged() {
    let mut omitted = oriented_engine(
        "none-a",
        &[],
        &oriented_scene("lift.path.yaml", 2.0, "loop", 0.0, None),
        10,
    );
    let mut none = oriented_engine(
        "none-b",
        &[],
        &oriented_scene("lift.path.yaml", 2.0, "loop", 0.0, Some("none")),
        10,
    );
    for engine in [&omitted, &none] {
        assert_eq!(
            rotation_of(engine, "platform").map(f32::to_bits),
            [0.0f32, 45.0, 0.0].map(f32::to_bits)
        );
    }
    assert_eq!(omitted.world_hash(), none.world_hash());
    let value = serde_json::to_value(PathFollower::default()).unwrap();
    assert!(value.get("orient").is_none(), "{value}");
}

#[test]
fn orient_is_written_in_the_parents_frame() {
    let scene = "\
format: 1
entities:
  - name: carrier
    components:
      Transform:
        rotation:
          - 0.0
          - 90.0
          - 0.0
  - name: platform
    components:
      Parent:
        index: 0
        generation: 0
      Transform: {}
      PathFollower:
        path: lift.path.yaml
        speed: 60.0
        mode: loop
        orient: yaw
";
    let engine = oriented_engine("parented", &[], scene, 2);
    let rotation = rotation_of(&engine, "platform");
    assert!(near(rotation, [0.0, -90.0, 0.0]), "{rotation:?}");
}

#[test]
fn the_orient_names_are_snake_case() {
    for (text, orient) in [
        ("none", PathOrient::None),
        ("yaw", PathOrient::Yaw),
        ("frame", PathOrient::Frame),
    ] {
        let follower: PathFollower =
            serde_json::from_value(serde_json::json!({ "orient": text })).unwrap();
        assert_eq!(follower.orient, orient);
    }
}

/// A kinematic sensor box riding `lift` at half a metre per tick, past a static capsule at x = 5.
const SENSOR_RIDE: &str = "format: 1
physics:
  space: free
entities:
  - name: probe
    components:
      Transform: {}
      PathFollower:
        path: lift.path.yaml
        speed: 30.0
        mode: once
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body: kinematic
        sensor: true
  - name: post
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
      Collider:
        shape:
          capsule:
            half_height: 0.5
            radius: 0.25
        body: static
";

#[derive(Default)]
struct SensorLog(Vec<(u64, SensorEvent)>);

fn watch_sensors(world: &mut World, ctx: &SystemCtx) {
    let events = world
        .resource::<PhysicsWorld>()
        .map(|physics| physics.sensor_events().to_vec())
        .unwrap_or_default();
    if world.resource::<SensorLog>().is_none() {
        world.insert_resource(SensorLog::default());
    }
    let log = world.resource_mut::<SensorLog>().unwrap();
    log.0
        .extend(events.into_iter().map(|event| (ctx.frame.as_u64(), event)));
}

#[test]
fn a_kinematic_path_follower_sensor_enters_then_exits_a_static_body() {
    let root = fixture_root("sensor-ride");
    let mut engine = game(&root)
        .system("watch_sensors", watch_sensors)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(SENSOR_RIDE)
        .expect("the scene loads");
    advance(&mut engine, 30);
    let world = engine.world();
    let named = |wanted: &str| {
        world
            .entities()
            .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == wanted))
            .expect("the scene names it")
    };
    let (probe, post) = (named("probe"), named("post"));
    let log = &world.resource::<SensorLog>().expect("the watcher ran").0;
    let phases: Vec<_> = log.iter().map(|(_, event)| *event).collect();
    assert_eq!(
        phases,
        vec![
            SensorEvent {
                sensor: probe,
                other: post,
                phase: SensorPhase::Entered,
            },
            SensorEvent {
                sensor: probe,
                other: post,
                phase: SensorPhase::Exited {
                    cause: ExitCause::Separated,
                },
            },
        ],
        "{log:?}"
    );
    assert!(log[0].0 < log[1].0);
}
